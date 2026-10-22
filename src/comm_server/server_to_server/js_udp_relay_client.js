"use strict";

/**
 * UdpRelayClient
 *
 * Connects the comm server OUT to a standalone UDP relay server
 * (droneengage_udp_relay_server, design: wiki/UdpRelayServer.md) and acts as a
 * broker for 9008 (CONST_TYPE_AndruavSystem_UdpProxy) requests: instead of
 * binding UDP sockets in this process, it asks the relay for a port pair and
 * replies to the unit with the relay's sockets in today's 9008 shape.
 *
 * Config (server.config):
 *   udp_relay           : [{host, port, id}]  absent or [] -> built-in js_udp_proxy (unchanged)
 *   udp_relay_fallback  : "none" | "builtin"  when the relay is unreachable (default none)
 *   udp_relay_pending_s : hold an open request this long while the relay reconnects (default 30)
 * Only the first relay is used here; selection over several is TASK-57.
 *
 * The link uses the same S2S handshake as the storage link (js_db_proxy_client.js):
 * Ed25519 (s2s_my_private_key/server_id) or shared-secret HMAC, depending on
 * the challenge 'method'; fail closed if no auth_success arrives in time.
 * Envelope {mt, ms, rid}; replies echo rid; pushes (closed/changed) have none.
 *
 * Units that want a proxy are remembered, so:
 *   - every (re)auth sends `sync` with them (the relay keeps their pairs),
 *   - pairs the relay no longer has (relay restart) are re-opened and pushed
 *     to the unit as an unsolicited 9008 en:true (de_mavlink only re-requests
 *     on reconnect),
 *   - a request made while the relay was down is opened when it comes back.
 */

const WebSocket = require('ws');
const { v4: uuidv4 } = require('uuid');
const c_CONSTANTS = require("../../js_constants.js");
const c_s2s_auth = require('../js_s2s_auth.js');
const c_activeSenders = require("../chat_server/js_andruav_active_senders.js");

const CONST_REQUEST_TIMEOUT_MS = 10000;
const CONST_HEARTBEAT_INTERVAL_MS = 30000;
const CONST_MAX_RECONNECT_DELAY_MS = 60000;
const CONST_INITIAL_RECONNECT_DELAY_MS = 1000;
const CONST_DEFAULT_PENDING_S = 30;

const CONST_FALLBACK_NONE = 'none';
const CONST_FALLBACK_BUILTIN = 'builtin';

// unit states
const CONST_UNIT_RELAY = 'relay';       // has (or had) a pair on the relay
const CONST_UNIT_PENDING = 'pending';   // waiting for the relay to come up
const CONST_UNIT_BUILTIN = 'builtin';   // served by js_udp_proxy (fallback builtin)

// Same short state codes as the storage link (js_constants.js).
const CONST_STATE = {
    DISCONNECTED: c_CONSTANTS.CONST_STORAGE_STATE_DISCONNECTED,
    CONNECTING:   c_CONSTANTS.CONST_STORAGE_STATE_CONNECTING,
    CONNECTED:    c_CONSTANTS.CONST_STORAGE_STATE_CONNECTED,
    ERROR:        c_CONSTANTS.CONST_STORAGE_STATE_ERROR
};

const CONST_ZERO_SOCKET = { address: '0.0.0.0', port: 0 };


function fn_log(p_text) {
    const c_colors = global.Colors || {};
    console.log((c_colors.FgYellow || '') + '[UdpRelay] ' + p_text + (c_colors.Reset || ''));
}

function fn_cfg() {
    return global.m_serverconfig.m_configuration;
}

function fn_unitKey(p_account, p_unit) {
    return String(p_account) + '\u0000' + String(p_unit);
}


/**
 * One S2S connection to one relay.
 * Events: 'authenticated', 'closed', 'push' (envelope), 'state' (code, error).
 */
class CUdpRelayConnection extends require('events') {

    constructor(p_endpoint) {
        super();
        this.m_endpoint = p_endpoint;
        this.m_ws = null;
        this.m_authenticated = false;
        this.m_state = CONST_STATE.DISCONNECTED;
        this.m_lastError = null;
        this.m_pending = new Map();        // rid -> {resolve, reject, timeout}
        this.m_reconnectAttempts = 0;
        this.m_reconnectTimer = null;
        this.m_authTimer = null;
        this.m_stopped = false;
    }

    fn_start() {
        this.m_stopped = false;
        this.fn_connect();
    }

    fn_stop() {
        this.m_stopped = true;
        clearTimeout(this.m_reconnectTimer);
        clearTimeout(this.m_authTimer);
        if (this.m_ws !== null) {
            try { this.m_ws.terminate(); } catch (e) { /* ignore */ }
        }
        this.fn_onClose();
    }

    fn_isConnected() {
        return (this.m_ws !== null) && (this.m_ws.readyState === WebSocket.OPEN) && (this.m_authenticated === true);
    }

    fn_setState(p_state, p_error) {
        const c_changed = (p_state !== this.m_state);
        this.m_state = p_state;
        this.m_lastError = p_error || null;
        if (c_changed) this.emit('state', p_state, this.m_lastError);
    }

    fn_connect() {
        if (this.m_stopped) return;
        clearTimeout(this.m_reconnectTimer);
        this.m_reconnectTimer = null;
        const c_proto = (this.m_endpoint.ssl === false) ? 'ws' : 'wss';
        const c_url = `${c_proto}://${this.m_endpoint.host}:${this.m_endpoint.port}`;
        this.fn_setState(CONST_STATE.CONNECTING);
        // self-signed certificates are accepted, same as the AUTH and storage links
        const c_ws = new WebSocket(c_url, { rejectUnauthorized: false });
        this.m_ws = c_ws;
        c_ws.on('open', () => this.fn_onOpen());
        c_ws.on('message', (p_data) => this.fn_onMessage(p_data));
        c_ws.on('close', () => { if (this.m_ws === c_ws) this.fn_onClose(); });
        c_ws.on('error', (p_err) => {
            this.fn_setState(CONST_STATE.ERROR, p_err.message);
            try { c_ws.close(); } catch (e) { /* ignore */ }
        });
    }

    fn_onOpen() {
        this.m_reconnectAttempts = 0;
        clearTimeout(this.m_authTimer);
        // fail closed: a relay that does not finish the handshake is dropped
        this.m_authTimer = setTimeout(() => {
            if (!this.m_authenticated && this.m_ws !== null) {
                fn_log(`relay ${this.m_endpoint.id}: S2S auth not completed within ${c_s2s_auth.CONST_S2S_AUTH_HANDSHAKE_TIMEOUT}ms — closing (fail closed)`);
                try { this.m_ws.close(); } catch (e) { /* ignore */ }
            }
        }, c_s2s_auth.CONST_S2S_AUTH_HANDSHAKE_TIMEOUT);
    }

    fn_onMessage(p_data) {
        const c_env = c_s2s_auth.fn_parseEnvelope(p_data);
        if (c_env != null) {
            if (c_env.s2s_auth === c_s2s_auth.CONST_S2S_AUTH_CHALLENGE && this.m_ws !== null) {
                try {
                    const c_id = fn_cfg().server_id;
                    this.m_ws.send((c_env.method === c_s2s_auth.CONST_S2S_AUTH_METHOD_HMAC)
                        ? c_s2s_auth.fn_buildHmacResponse(c_env.nonce, c_id)
                        : c_s2s_auth.fn_buildResponse(c_env.nonce, c_id));
                } catch (ex) {
                    fn_log(`relay ${this.m_endpoint.id}: S2S handshake failed: ${ex}`);
                    this.fn_setState(CONST_STATE.ERROR, String(ex.message || ex));
                    try { this.m_ws.close(); } catch (e) { /* ignore */ }
                }
            }
            return;
        }

        let v_msg;
        try {
            v_msg = JSON.parse(p_data);
        } catch (ex) {
            return;
        }
        if (v_msg === null || typeof v_msg !== 'object') return;

        if (v_msg.type === 'auth_success') {
            clearTimeout(this.m_authTimer);
            this.m_authenticated = true;
            fn_log(`authenticated with relay ${this.m_endpoint.id} (${this.m_endpoint.host}:${this.m_endpoint.port})`);
            this.fn_setState(CONST_STATE.CONNECTED);
            this.emit('authenticated');
            return;
        }
        if (v_msg.type === 'error') {
            fn_log(`relay ${this.m_endpoint.id} error: ${v_msg.error}`);
            return;
        }
        if (v_msg.rid != null && this.m_pending.has(v_msg.rid)) {
            const c_req = this.m_pending.get(v_msg.rid);
            this.m_pending.delete(v_msg.rid);
            clearTimeout(c_req.timeout);
            c_req.resolve(v_msg);
            return;
        }
        if (v_msg.rid == null && v_msg.mt != null) this.emit('push', v_msg);
    }

    fn_onClose() {
        const c_wasAuthenticated = this.m_authenticated;
        this.m_authenticated = false;
        this.m_ws = null;
        clearTimeout(this.m_authTimer);
        this.m_pending.forEach((p_req) => {
            clearTimeout(p_req.timeout);
            p_req.reject(new Error('relay connection closed'));
        });
        this.m_pending.clear();
        if (this.m_state !== CONST_STATE.ERROR || this.m_stopped) this.fn_setState(CONST_STATE.DISCONNECTED, this.m_lastError);
        if (c_wasAuthenticated) this.emit('closed');
        if (this.m_stopped || this.m_reconnectTimer !== null) return;

        // exponential backoff 1 s -> 60 s with +/-25 % jitter
        this.m_reconnectAttempts += 1;
        const c_base = Math.min(CONST_INITIAL_RECONNECT_DELAY_MS * Math.pow(2, this.m_reconnectAttempts - 1), CONST_MAX_RECONNECT_DELAY_MS);
        const c_delay = Math.floor(c_base + c_base * 0.25 * (Math.random() * 2 - 1));
        this.m_reconnectTimer = setTimeout(() => this.fn_connect(), c_delay);
        this.m_reconnectTimer.unref();
    }

    /**
     * @returns Promise of the reply envelope {mt, ms, success, error?}
     */
    fn_request(p_mt, p_ms) {
        return new Promise((resolve, reject) => {
            if (!this.fn_isConnected()) {
                reject(new Error('relay not connected'));
                return;
            }
            const c_rid = uuidv4();
            const c_timeout = setTimeout(() => {
                this.m_pending.delete(c_rid);
                reject(new Error('relay request timed out'));
            }, CONST_REQUEST_TIMEOUT_MS);
            this.m_pending.set(c_rid, { resolve, reject, timeout: c_timeout });
            try {
                this.m_ws.send(JSON.stringify({ mt: p_mt, ms: p_ms, rid: c_rid }));
            } catch (ex) {
                clearTimeout(c_timeout);
                this.m_pending.delete(c_rid);
                reject(ex);
            }
        });
    }
}


// ----------------------------------------------------------------------
// broker
// ----------------------------------------------------------------------

let m_relay = null;                 // CUdpRelayConnection
let m_units = new Map();            // unitKey -> {account, group, unit, req, cap, state, timer, waiting}
let m_sendToAuthCallback = null;
let m_heartbeatTimer = null;
let m_heartbeatMs = CONST_HEARTBEAT_INTERVAL_MS;


/**
 * True when udp_relay is configured (non-empty array): 9008 goes through the
 * relay instead of the built-in proxy.
 */
function fn_isEnabled() {
    const c_relays = fn_cfg().udp_relay;
    return Array.isArray(c_relays) && c_relays.length > 0;
}

function fn_fallbackMode() {
    return (fn_cfg().udp_relay_fallback === CONST_FALLBACK_BUILTIN) ? CONST_FALLBACK_BUILTIN : CONST_FALLBACK_NONE;
}

function fn_pendingMs() {
    const c_s = Number(fn_cfg().udp_relay_pending_s);
    return ((Number.isFinite(c_s) && c_s >= 0) ? c_s : CONST_DEFAULT_PENDING_S) * 1000;
}


/**
 * Connects to the first configured relay. No-op without udp_relay.
 * @param {object} [p_opts] {heartbeatMs} (tests)
 */
function fn_initialize(p_opts) {
    if (!fn_isEnabled() || m_relay !== null) return;
    const c_opts = p_opts || {};
    m_heartbeatMs = c_opts.heartbeatMs || CONST_HEARTBEAT_INTERVAL_MS;

    const c_entry = fn_cfg().udp_relay[0];
    m_relay = new CUdpRelayConnection({
        host: c_entry.host,
        port: c_entry.port,
        id: c_entry.id || `${c_entry.host}:${c_entry.port}`,
        ssl: c_entry.ssl
    });
    m_relay.on('authenticated', fn_onRelayAuthenticated);
    m_relay.on('push', fn_onRelayPush);
    m_relay.on('state', function () { fn_sendStatus(); });
    fn_log(`using relay ${m_relay.m_endpoint.id} at ${c_entry.host}:${c_entry.port} (fallback ${fn_fallbackMode()}, pending ${fn_pendingMs() / 1000}s)`);
    m_relay.fn_start();

    m_heartbeatTimer = setInterval(fn_sendStatus, m_heartbeatMs);
    m_heartbeatTimer.unref();
}


/**
 * Stops the relay link and forgets all state (tests, shutdown).
 */
function fn_shutdown() {
    clearInterval(m_heartbeatTimer);
    m_heartbeatTimer = null;
    m_units.forEach(function (p_entry) { clearTimeout(p_entry.timer); });
    m_units = new Map();
    if (m_relay !== null) {
        const c_relay = m_relay;
        m_relay = null;
        c_relay.removeAllListeners();
        c_relay.fn_stop();
    }
}


function fn_isConnected() {
    return m_relay !== null && m_relay.fn_isConnected();
}


/**
 * Sets the callback used to send the `j` UDP_RELAY_STATUS command to AUTH.
 */
function fn_setSendToAuthCallback(p_callback) {
    m_sendToAuthCallback = p_callback;
    fn_sendStatus();
}


function fn_countRelayUnits() {
    let v_count = 0;
    m_units.forEach(function (p_entry) { if (p_entry.state === CONST_UNIT_RELAY) v_count += 1; });
    return v_count;
}


/**
 * `j` UDP_RELAY_STATUS to AUTH: on connect, on every state change and every
 * 30 s. Same shape as `d` STORAGE_STATUS plus relay_id and pairs (the units
 * this comm server holds relay pairs for).
 */
function fn_sendStatus() {
    if (m_sendToAuthCallback === null || m_relay === null) return;
    const c_cmd = {
        c: c_CONSTANTS.CONST_CS_CMD_UDP_RELAY_STATUS,
        d: {
            relay_id: m_relay.m_endpoint.id,
            host: m_relay.m_endpoint.host,
            port: m_relay.m_endpoint.port,
            status: m_relay.m_state,
            pairs: fn_countRelayUnits(),
            error: m_relay.m_lastError,
            timestamp: Math.floor(Date.now() / 1000)
        }
    };
    try {
        m_sendToAuthCallback(JSON.stringify(c_cmd));
    } catch (ex) {
        fn_log(`failed to send relay status to AUTH: ${ex}`);
    }
}


// ----------------------------------------------------------------------
// unit side
// ----------------------------------------------------------------------

function fn_unitIdentity(p_ws) {
    const c_login = p_ws.m_loginRequest || {};
    return {
        account: c_login.m_accountID,
        group: c_login.m_groupID,
        unit: p_ws.name
    };
}


/**
 * Builds the 9008 reply payload for the unit from a relay open reply:
 * today's {en, socket1, socket2} plus any extra relay fields (e.g. tk),
 * without relay-internal ids.
 */
function fn_unitPayload(p_relayMs) {
    const c_ms = Object.assign({}, p_relayMs);
    delete c_ms.relay_id;
    delete c_ms.pair_id;
    delete c_ms.error;
    c_ms.en = (p_relayMs.en === true);
    if (c_ms.en !== true) {
        c_ms.socket1 = Object.assign({}, CONST_ZERO_SOCKET);
        c_ms.socket2 = Object.assign({}, CONST_ZERO_SOCKET);
    }
    return c_ms;
}


function fn_disabledPayload() {
    return {
        en: false,
        socket1: Object.assign({}, CONST_ZERO_SOCKET),
        socket2: Object.assign({}, CONST_ZERO_SOCKET)
    };
}


/**
 * Sends a 9008 reply on p_ws, reusing the request object like the built-in path.
 */
function fn_replyToUnit(p_ws, v_jmsg, p_ms) {
    v_jmsg.ms = p_ms;
    v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
    v_jmsg.tg = p_ws.name;
    v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
    try {
        p_ws.send(JSON.stringify(v_jmsg));
    } catch (ex) {
        fn_log(`could not reply to unit '${p_ws.name}': ${ex}`);
    }
}


/**
 * Unsolicited 9008 to a connected unit (relay recovery, changed, closed).
 * Skipped if the unit is gone or now belongs to another account.
 */
function fn_pushToUnit(p_account, p_unit, p_ms) {
    const c_ws = c_activeSenders.getActiveSender(p_unit);
    if (c_ws == null || c_ws.readyState !== 1) return false;
    if (c_ws.m_loginRequest == null || String(c_ws.m_loginRequest.m_accountID) !== String(p_account)) return false;
    const c_msg = {};
    c_msg[c_CONSTANTS.CONST_WS_MESSAGE_ID] = c_CONSTANTS.CONST_TYPE_AndruavSystem_UdpProxy;
    c_msg[c_CONSTANTS.CONST_WS_MSG_ROUTING] = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
    c_msg[c_CONSTANTS.CONST_WS_TARGET_ID] = p_unit;
    c_msg[c_CONSTANTS.CONST_WS_SENDER_ID] = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
    c_msg[c_CONSTANTS.CONST_WS_PAYLOAD] = p_ms;
    try {
        c_ws.send(JSON.stringify(c_msg));
        return true;
    } catch (ex) {
        fn_log(`could not push 9008 to unit '${p_unit}': ${ex}`);
        return false;
    }
}


function fn_openParams(p_entry) {
    const c_ms = {
        account: p_entry.account,
        group: p_entry.group,
        unit: p_entry.unit,
        req: p_entry.req
    };
    if (p_entry.cap !== undefined) c_ms.cap = p_entry.cap;
    return c_ms;
}


/**
 * Opens p_entry's pair on the relay.
 * @returns Promise of the unit payload ({en, socket1, socket2, ...}).
 */
async function fn_openOnRelay(p_entry) {
    p_entry.opening = true;
    let c_rep;
    try {
        c_rep = await m_relay.fn_request('open', fn_openParams(p_entry));
    } finally {
        p_entry.opening = false;
    }
    if (c_rep.success !== true || !c_rep.ms || c_rep.ms.en !== true) {
        fn_log(`relay refused open for '${p_entry.unit}': ${c_rep.error || 'en:false'}`);
        return fn_disabledPayload();
    }
    p_entry.state = CONST_UNIT_RELAY;
    fn_sendStatus();
    return fn_unitPayload(c_rep.ms);
}


/**
 * 9008 handler when udp_relay is configured. Called by
 * js_chat_system_commands.js after the actor check.
 * @param {object} v_jmsg the 9008 request (reused for the reply)
 * @param {WebSocket} p_ws the unit
 * @param {function} fn_builtin (v_jmsg, p_ws) -> today's built-in handling, for fallback builtin
 */
async function fn_handleUnitRequest(v_jmsg, p_ws, fn_builtin) {
    const c_id = fn_unitIdentity(p_ws);
    const c_key = fn_unitKey(c_id.account, c_id.unit);
    const c_req = v_jmsg.ms || {};
    const c_old = m_units.get(c_key);

    if (c_req.en !== true) {
        // en:false -> close, wherever the unit is served
        if (c_old !== undefined) clearTimeout(c_old.timer);
        m_units.delete(c_key);
        if (c_old !== undefined && c_old.state === CONST_UNIT_BUILTIN) {
            fn_builtin(v_jmsg, p_ws);
            return;
        }
        if (fn_isConnected()) {
            try { await m_relay.fn_request('close', { account: c_id.account, unit: c_id.unit }); }
            catch (ex) { fn_log(`relay close for '${c_id.unit}' failed: ${ex.message}`); }
        }
        fn_sendStatus();
        fn_replyToUnit(p_ws, v_jmsg, fn_disabledPayload());
        return;
    }

    if (c_old !== undefined) clearTimeout(c_old.timer);
    const c_entry = {
        account: c_id.account,
        group: c_id.group,
        unit: c_id.unit,
        req: {
            p1: (c_req.socket1 && c_req.socket1.port) || 0,
            p2: (c_req.socket2 && c_req.socket2.port) || 0
        },
        cap: c_req.cap,
        state: CONST_UNIT_PENDING,
        timer: null,
        waiting: null,      // {v_jmsg, p_ws} while a request is held
        opening: false      // an open request is in flight
    };
    m_units.set(c_key, c_entry);

    if (fn_isConnected()) {
        let v_ms;
        try {
            v_ms = await fn_openOnRelay(c_entry);
        } catch (ex) {
            // link dropped mid-request: hold it like a request made while down
            fn_log(`relay open for '${c_id.unit}' failed: ${ex.message}`);
            fn_holdRequest(c_entry, v_jmsg, p_ws, fn_builtin);
            return;
        }
        if (m_units.get(c_key) === c_entry && v_ms.en !== true) m_units.delete(c_key);
        fn_replyToUnit(p_ws, v_jmsg, v_ms);
        return;
    }
    fn_holdRequest(c_entry, v_jmsg, p_ws, fn_builtin);
}


/**
 * Relay unreachable: hold the request for udp_relay_pending_s. If the relay
 * authenticates in time the held request is answered normally; otherwise
 * fallback none -> en:false now and an unsolicited en:true once the relay is
 * back; fallback builtin -> today's built-in path.
 */
function fn_holdRequest(p_entry, v_jmsg, p_ws, fn_builtin) {
    p_entry.state = CONST_UNIT_PENDING;
    p_entry.waiting = { v_jmsg: v_jmsg, p_ws: p_ws };
    p_entry.timer = setTimeout(function () {
        p_entry.timer = null;
        p_entry.waiting = null;
        const c_key = fn_unitKey(p_entry.account, p_entry.unit);
        if (m_units.get(c_key) !== p_entry) return;
        if (fn_fallbackMode() === CONST_FALLBACK_BUILTIN) {
            fn_log(`relay unavailable for '${p_entry.unit}', falling back to the built-in proxy`);
            p_entry.state = CONST_UNIT_BUILTIN;
            fn_builtin(v_jmsg, p_ws);
            return;
        }
        fn_log(`relay unavailable for '${p_entry.unit}', replying en:false (will push en:true when the relay is back)`);
        fn_replyToUnit(p_ws, v_jmsg, fn_disabledPayload());
    }, fn_pendingMs());
}


/**
 * Unit websocket closed: tell the relay the unit is offline (its pair idles
 * out there) instead of closing it, and forget the unit.
 */
function fn_onUnitDisconnected(p_ws) {
    if (p_ws == null || p_ws.m_loginRequest == null) return;
    const c_id = fn_unitIdentity(p_ws);
    const c_key = fn_unitKey(c_id.account, c_id.unit);
    const c_entry = m_units.get(c_key);
    if (c_entry === undefined) return;
    clearTimeout(c_entry.timer);
    m_units.delete(c_key);
    if (c_entry.state === CONST_UNIT_RELAY && fn_isConnected()) {
        m_relay.fn_request('unit_offline', { account: c_id.account, unit: c_id.unit }).catch(function (ex) {
            fn_log(`relay unit_offline for '${c_id.unit}' failed: ${ex.message}`);
        });
    }
    fn_sendStatus();
}


/**
 * On every (re)auth: sync the units we hold pairs for, then open whatever
 * the relay no longer has (relay restart) and everything that was waiting.
 */
async function fn_onRelayAuthenticated() {
    // Only units known now are handled here; a request arriving while the
    // sync is in flight opens its own pair.
    const c_relayUnits = [];
    const c_candidates = [];
    m_units.forEach(function (p_entry, p_key) {
        if (p_entry.state === CONST_UNIT_BUILTIN) return;
        c_candidates.push({ key: p_key, entry: p_entry, wasRelay: p_entry.state === CONST_UNIT_RELAY });
        if (p_entry.state === CONST_UNIT_RELAY) c_relayUnits.push({ account: p_entry.account, unit: p_entry.unit });
    });

    let v_kept = new Set();
    try {
        const c_rep = await m_relay.fn_request('sync', { units: c_relayUnits });
        if (c_rep.success === true && c_rep.ms && Array.isArray(c_rep.ms.kept)) {
            v_kept = new Set(c_rep.ms.kept.map(function (p_u) { return fn_unitKey(p_u.account, p_u.unit); }));
        }
    } catch (ex) {
        fn_log(`relay sync failed: ${ex.message}`);
        return;
    }

    for (const { key: c_key, entry: c_entry, wasRelay: c_wasRelay } of c_candidates) {
        if (m_units.get(c_key) !== c_entry || c_entry.opening) continue;
        if (c_entry.state === CONST_UNIT_BUILTIN) continue;
        // still on the relay, or opened by its own request meanwhile
        if (c_entry.state === CONST_UNIT_RELAY && (v_kept.has(c_key) || !c_wasRelay)) continue;
        if (!fn_isConnected()) return;

        const c_waiting = c_entry.waiting;
        clearTimeout(c_entry.timer);
        c_entry.timer = null;
        c_entry.waiting = null;
        let v_ms;
        try {
            v_ms = await fn_openOnRelay(c_entry);
        } catch (ex) {
            fn_log(`relay re-open for '${c_entry.unit}' failed: ${ex.message}`);
            continue;
        }
        if (m_units.get(c_key) !== c_entry) continue;
        if (v_ms.en !== true) m_units.delete(c_key);
        if (c_waiting !== null) {
            fn_replyToUnit(c_waiting.p_ws, c_waiting.v_jmsg, v_ms);
        } else if (v_ms.en === true) {
            // the unit got en:false (or a pair the relay lost): tell it now
            fn_pushToUnit(c_entry.account, c_entry.unit, v_ms);
        }
    }
    fn_sendStatus();
}


/**
 * Relay pushes for pairs this comm server owns:
 *   closed  -> today's notifyUnitProxyClosed 9008 en:false to the unit
 *   changed -> unsolicited 9008 en:true with the new sockets
 */
function fn_onRelayPush(p_msg) {
    const c_ms = p_msg.ms || {};
    if (c_ms.owner !== fn_cfg().server_id) return;
    const c_key = fn_unitKey(c_ms.account, c_ms.unit);

    if (p_msg.mt === 'closed') {
        // Only pairs a unit still wants: a close the unit asked for (en:false)
        // already got its reply.
        const c_entry = m_units.get(c_key);
        if (c_entry === undefined || c_entry.state !== CONST_UNIT_RELAY || c_entry.opening) return;
        m_units.delete(c_key);
        if (fn_pushToUnit(c_ms.account, c_ms.unit, fn_disabledPayload())) {
            fn_log(`notified unit '${c_ms.unit}' that its relay pair was closed (${c_ms.reason})`);
        }
        fn_sendStatus();
        return;
    }
    if (p_msg.mt === 'changed') {
        fn_pushToUnit(c_ms.account, c_ms.unit, fn_unitPayload({ en: true, socket1: c_ms.socket1, socket2: c_ms.socket2 }));
    }
}


// ----------------------------------------------------------------------
// AUTH dashboard (e / g)
// ----------------------------------------------------------------------

/**
 * Relay pairs owned by this comm server, in fn_getActiveProxiesList's row
 * shape plus account/group/owner/relay_id/online/rx/tx. [] when the relay is
 * not configured or not reachable.
 */
async function fn_getRelayProxiesList() {
    if (!fn_isConnected()) return [];
    let v_rep;
    try {
        v_rep = await m_relay.fn_request('list', {});
    } catch (ex) {
        fn_log(`relay list failed: ${ex.message}`);
        return [];
    }
    if (v_rep.success !== true || !v_rep.ms || !Array.isArray(v_rep.ms.pairs)) return [];
    const c_me = fn_cfg().server_id;
    return v_rep.ms.pairs.filter(function (p_row) { return p_row.owner === c_me; }).map(function (p_row) {
        return {
            name: p_row.unit,
            account: p_row.account,
            group: p_row.group,
            owner: p_row.owner,
            relay_id: v_rep.ms.relay_id,
            socket1: { address: p_row.socket1.address, port: p_row.socket1.port },
            socket2: { address: p_row.socket2.address, port: p_row.socket2.port },
            ready: p_row.ready,
            blocked: p_row.blocked,
            created: p_row.created,
            last_access: p_row.last_access,
            online: p_row.online,
            rx: p_row.rx,
            tx: p_row.tx
        };
    });
}


/**
 * Blocks/unblocks the relay pairs this comm server owns for unit p_name
 * (optionally only in account p_account).
 * @returns Promise<boolean> applied to at least one pair
 */
async function fn_setRelayProxyBlocked(p_name, p_blocked, p_account) {
    const c_rows = (await fn_getRelayProxiesList()).filter(function (p_row) {
        return p_row.name === p_name && (p_account == null || String(p_row.account) === String(p_account));
    });
    let v_applied = false;
    for (const c_row of c_rows) {
        try {
            const c_rep = await m_relay.fn_request('block', { account: c_row.account, unit: c_row.name, blocked: p_blocked === true });
            if (c_rep.success === true && c_rep.ms && c_rep.ms.applied === true) v_applied = true;
        } catch (ex) {
            fn_log(`relay block for '${p_name}' failed: ${ex.message}`);
        }
    }
    return v_applied;
}


module.exports = {
    CUdpRelayConnection,
    fn_isEnabled,
    fn_initialize,
    fn_shutdown,
    fn_isConnected,
    fn_setSendToAuthCallback,
    fn_sendStatus,
    fn_handleUnitRequest,
    fn_onUnitDisconnected,
    fn_getRelayProxiesList,
    fn_setRelayProxyBlocked
};
