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
 *   udp_relay            : [{host, port, id}]  absent or [] -> built-in js_udp_proxy (unchanged)
 *   udp_relay_select     : "hash" (default) | "first_healthy" | "least_loaded"  (wiki §7)
 *   udp_relay_fallback   : "none" | "builtin"  when no relay is reachable (default none)
 *   udp_relay_pending_s  : hold an open request this long while relays reconnect (default 30)
 *   udp_relay_failover_s : a relay down this long has its units moved to another relay
 *                          (default 120 = the relay's default owner_grace_s)
 *
 * Every configured relay gets its own connection. A unit's relay is chosen
 * when it first asks (hash = rendezvous hash of account/unit over the healthy
 * relays, so a unit lands on the same relay every time and removing a relay
 * moves only its units; first_healthy = first connected in list order;
 * least_loaded = lowest `pairs` from the relays' stats) and remembered: it
 * stays there while that relay is healthy, and when the relay recovers after
 * a failover nothing moves back (no flapping).
 *
 * The link uses the same S2S handshake as the storage link (js_db_proxy_client.js):
 * Ed25519 (s2s_my_private_key/server_id) or shared-secret HMAC, depending on
 * the challenge 'method'; fail closed if no auth_success arrives in time.
 * Envelope {mt, ms, rid}; replies echo rid; pushes (closed/changed) have none.
 *
 * Units that want a proxy are remembered, so:
 *   - every (re)auth sends `sync` with the units on that relay (it keeps their pairs),
 *   - pairs the relay no longer has (relay restart) are re-opened and pushed
 *     to the unit as an unsolicited 9008 en:true (de_mavlink only re-requests
 *     on reconnect),
 *   - a request made while the relay was down is opened when it comes back.
 *
 * GCS-side pinning (relay gcs_peer_policy): `open` carries gcs_ips, the IPs
 * of the GCS sockets in the unit's account+group; a GCS joining that group
 * is added with peer_allow; and GCSes manage the pin with 9021
 * (CONST_TYPE_AndruavSystem_UdpProxyPeer) {tg_unit, act: allow|reset|query}.
 */

const crypto = require('crypto');
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
const CONST_DEFAULT_FAILOVER_S = 120;

const CONST_SELECT_HASH = 'hash';
const CONST_SELECT_FIRST_HEALTHY = 'first_healthy';
const CONST_SELECT_LEAST_LOADED = 'least_loaded';

const CONST_FALLBACK_NONE = 'none';
const CONST_FALLBACK_BUILTIN = 'builtin';

// unit states
const CONST_UNIT_RELAY = 'relay';       // has (or had) a pair on entry.relay
const CONST_UNIT_PENDING = 'pending';   // waiting for a relay to come up
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

let m_relays = [];                  // CUdpRelayConnection, in udp_relay order
let m_units = new Map();            // unitKey -> {account, group, unit, req, cap, state, relay, timer, waiting, opening}
let m_failoverTimers = new Map();   // relay id -> timer started when its link was lost
let m_relayLoads = new Map();       // relay id -> pairs (from stats, least_loaded)
let m_sendToAuthCallback = null;
let m_heartbeatTimer = null;
let m_heartbeatMs = CONST_HEARTBEAT_INTERVAL_MS;


/**
 * True when udp_relay is configured (non-empty array): 9008 goes through the
 * relays instead of the built-in proxy.
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

function fn_failoverMs() {
    const c_s = Number(fn_cfg().udp_relay_failover_s);
    return ((Number.isFinite(c_s) && c_s >= 0) ? c_s : CONST_DEFAULT_FAILOVER_S) * 1000;
}

function fn_selectMode() {
    const c_mode = fn_cfg().udp_relay_select;
    return [CONST_SELECT_FIRST_HEALTHY, CONST_SELECT_LEAST_LOADED].includes(c_mode) ? c_mode : CONST_SELECT_HASH;
}


/**
 * Picks a relay id for (account, unit) among p_ids (the healthy relays, in
 * config order). Pure, so the choice is the same on every comm server and
 * across restarts.
 *   hash          rendezvous (highest random weight) hash: highest
 *                 sha256(id, account, unit); removing a relay only moves
 *                 the units that were on it
 *   first_healthy the first id
 *   least_loaded  the lowest p_loads[id] (unknown = 0), ties in config order
 * @returns {string|null}
 */
function fn_selectRelayId(p_account, p_unit, p_ids, p_mode, p_loads) {
    if (!Array.isArray(p_ids) || p_ids.length === 0) return null;
    if (p_mode === CONST_SELECT_FIRST_HEALTHY) return p_ids[0];
    if (p_mode === CONST_SELECT_LEAST_LOADED) {
        let v_best = p_ids[0];
        let v_bestLoad = Infinity;
        for (const c_id of p_ids) {
            const c_load = Number((p_loads || {})[c_id]) || 0;
            if (c_load < v_bestLoad) {
                v_best = c_id;
                v_bestLoad = c_load;
            }
        }
        return v_best;
    }
    let v_best = null;
    let v_bestScore = '';
    for (const c_id of p_ids) {
        const c_score = crypto.createHash('sha256').update(`${c_id}\n${p_account}\n${p_unit}`).digest('hex');
        if (v_best === null || c_score > v_bestScore) {
            v_best = c_id;
            v_bestScore = c_score;
        }
    }
    return v_best;
}


function fn_healthyRelays() {
    return m_relays.filter(function (p_relay) { return p_relay.fn_isConnected(); });
}


/**
 * The relay a unit should go to now, or null when none is healthy.
 */
function fn_selectRelay(p_account, p_unit) {
    const c_healthy = fn_healthyRelays();
    const c_loads = {};
    m_relayLoads.forEach(function (p_load, p_id) { c_loads[p_id] = p_load; });
    const c_id = fn_selectRelayId(p_account, p_unit, c_healthy.map(function (p_r) { return p_r.m_endpoint.id; }), fn_selectMode(), c_loads);
    return c_healthy.find(function (p_r) { return p_r.m_endpoint.id === c_id; }) || null;
}


/**
 * Connects to every configured relay. No-op without udp_relay.
 * @param {object} [p_opts] {heartbeatMs} (tests)
 */
function fn_initialize(p_opts) {
    if (!fn_isEnabled() || m_relays.length > 0) return;
    const c_opts = p_opts || {};
    m_heartbeatMs = c_opts.heartbeatMs || CONST_HEARTBEAT_INTERVAL_MS;

    const c_seen = new Set();
    for (const c_entry of fn_cfg().udp_relay) {
        if (c_entry == null || c_entry.host == null || c_entry.port == null) continue;
        const c_id = String(c_entry.id || `${c_entry.host}:${c_entry.port}`);
        if (c_seen.has(c_id)) {
            fn_log(`duplicate relay id ${c_id} ignored`);
            continue;
        }
        c_seen.add(c_id);
        const c_relay = new CUdpRelayConnection({ host: c_entry.host, port: c_entry.port, id: c_id, ssl: c_entry.ssl });
        c_relay.on('authenticated', function () { fn_onRelayAuthenticated(c_relay); });
        c_relay.on('closed', function () { fn_onRelayLost(c_relay); });
        c_relay.on('push', function (p_msg) { fn_onRelayPush(c_relay, p_msg); });
        c_relay.on('state', function () { fn_sendRelayStatus(c_relay); });
        m_relays.push(c_relay);
        fn_log(`relay ${c_id} at ${c_entry.host}:${c_entry.port}`);
    }
    fn_log(`${m_relays.length} relay(s), select ${fn_selectMode()}, fallback ${fn_fallbackMode()}, pending ${fn_pendingMs() / 1000}s, failover ${fn_failoverMs() / 1000}s`);
    m_relays.forEach(function (p_relay) { p_relay.fn_start(); });

    m_heartbeatTimer = setInterval(fn_heartbeat, m_heartbeatMs);
    m_heartbeatTimer.unref();
}


/**
 * Stops every relay link and forgets all state (tests, shutdown).
 */
function fn_shutdown() {
    clearInterval(m_heartbeatTimer);
    m_heartbeatTimer = null;
    m_units.forEach(function (p_entry) { clearTimeout(p_entry.timer); });
    m_units = new Map();
    m_failoverTimers.forEach(function (p_timer) { clearTimeout(p_timer); });
    m_failoverTimers = new Map();
    m_relayLoads = new Map();
    const c_relays = m_relays;
    m_relays = [];
    c_relays.forEach(function (p_relay) {
        p_relay.removeAllListeners();
        p_relay.fn_stop();
    });
}


/**
 * True when at least one relay is connected and authenticated.
 */
function fn_isConnected() {
    return m_relays.some(function (p_relay) { return p_relay.fn_isConnected(); });
}


/**
 * Sets the callback used to send the `j` UDP_RELAY_STATUS command to AUTH.
 */
function fn_setSendToAuthCallback(p_callback) {
    m_sendToAuthCallback = p_callback;
    fn_sendStatus();
}


function fn_countRelayUnits(p_relay) {
    let v_count = 0;
    m_units.forEach(function (p_entry) {
        if (p_entry.state === CONST_UNIT_RELAY && p_entry.relay === p_relay) v_count += 1;
    });
    return v_count;
}


/**
 * `j` UDP_RELAY_STATUS to AUTH for one relay: on connect, on every state
 * change and every 30 s. Same shape as `d` STORAGE_STATUS plus relay_id and
 * pairs (the units this comm server holds on that relay).
 */
function fn_sendRelayStatus(p_relay) {
    if (m_sendToAuthCallback === null) return;
    const c_cmd = {
        c: c_CONSTANTS.CONST_CS_CMD_UDP_RELAY_STATUS,
        d: {
            relay_id: p_relay.m_endpoint.id,
            host: p_relay.m_endpoint.host,
            port: p_relay.m_endpoint.port,
            status: p_relay.m_state,
            pairs: fn_countRelayUnits(p_relay),
            error: p_relay.m_lastError,
            timestamp: Math.floor(Date.now() / 1000)
        }
    };
    try {
        m_sendToAuthCallback(JSON.stringify(c_cmd));
    } catch (ex) {
        fn_log(`failed to send relay status to AUTH: ${ex}`);
    }
}


function fn_sendStatus() {
    m_relays.forEach(fn_sendRelayStatus);
}


/**
 * Every heartbeat: `j` per relay, and (least_loaded) each relay's load.
 */
function fn_heartbeat() {
    fn_sendStatus();
    if (fn_selectMode() === CONST_SELECT_LEAST_LOADED) {
        fn_healthyRelays().forEach(fn_refreshLoad);
    }
}


function fn_refreshLoad(p_relay) {
    p_relay.fn_request('stats', {}).then(function (p_rep) {
        if (p_rep.success === true && p_rep.ms && Number.isFinite(p_rep.ms.pairs)) {
            m_relayLoads.set(p_relay.m_endpoint.id, p_rep.ms.pairs);
        }
    }).catch(function () { /* next heartbeat */ });
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
 * Unsolicited 9008 to a connected unit (relay recovery, failover, changed,
 * closed). Skipped if the unit is gone or now belongs to another account.
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


/**
 * IPs of the GCS sockets in the same account+group as the unit socket.
 */
function fn_gcsIpsOfGroup(p_unitWs) {
    const c_ips = new Set();
    const c_group = (p_unitWs != null) ? p_unitWs.m__group : null;
    if (c_group == null || c_group.m_units == null) return [];
    for (const c_name in c_group.m_units) {
        const c_ws = c_group.m_units[c_name];
        if (c_ws && c_ws.m_loginRequest
            && c_ws.m_loginRequest.m_actorType === c_CONSTANTS.CONST_ACTOR_TYPE_GCS
            && (typeof c_ws.m_remoteIp === 'string') && c_ws.m_remoteIp.length > 0) {
            c_ips.add(c_ws.m_remoteIp);
        }
    }
    return Array.from(c_ips);
}


/**
 * The connected unit socket for an entry, or null (gone, or another account).
 */
function fn_unitSocket(p_account, p_unit) {
    const c_ws = c_activeSenders.getActiveSender(p_unit);
    if (c_ws == null || c_ws.m_loginRequest == null) return null;
    if (String(c_ws.m_loginRequest.m_accountID) !== String(p_account)) return null;
    return c_ws;
}


function fn_openParams(p_entry) {
    const c_ms = {
        account: p_entry.account,
        group: p_entry.group,
        unit: p_entry.unit,
        req: p_entry.req
    };
    if (p_entry.cap !== undefined) c_ms.cap = p_entry.cap;
    const c_gcsIps = fn_gcsIpsOfGroup(fn_unitSocket(p_entry.account, p_entry.unit));
    if (c_gcsIps.length > 0) c_ms.gcs_ips = c_gcsIps;
    return c_ms;
}


/**
 * Opens p_entry's pair on p_relay, which then holds the unit.
 * @returns Promise of the unit payload ({en, socket1, socket2, ...}).
 */
async function fn_openOnRelay(p_entry, p_relay) {
    p_entry.opening = true;
    let c_rep;
    try {
        c_rep = await p_relay.fn_request('open', fn_openParams(p_entry));
    } finally {
        p_entry.opening = false;
    }
    if (c_rep.success !== true || !c_rep.ms || c_rep.ms.en !== true) {
        fn_log(`relay ${p_relay.m_endpoint.id} refused open for '${p_entry.unit}': ${c_rep.error || 'en:false'}`);
        return fn_disabledPayload();
    }
    p_entry.state = CONST_UNIT_RELAY;
    p_entry.relay = p_relay;
    fn_sendRelayStatus(p_relay);
    return fn_unitPayload(c_rep.ms);
}


/**
 * The relay for a new request: the unit's current relay while it is
 * healthy (a re-request never moves a unit), otherwise a fresh selection.
 */
function fn_relayFor(p_old, p_account, p_unit) {
    if (p_old !== undefined && p_old.relay != null && p_old.relay.fn_isConnected()) return p_old.relay;
    return fn_selectRelay(p_account, p_unit);
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
        if (c_old !== undefined && c_old.relay != null && c_old.relay.fn_isConnected()) {
            try { await c_old.relay.fn_request('close', { account: c_id.account, unit: c_id.unit }); }
            catch (ex) { fn_log(`relay close for '${c_id.unit}' failed: ${ex.message}`); }
            fn_sendRelayStatus(c_old.relay);
        }
        fn_replyToUnit(p_ws, v_jmsg, fn_disabledPayload());
        return;
    }

    if (c_old !== undefined) clearTimeout(c_old.timer);
    const c_relay = fn_relayFor(c_old, c_id.account, c_id.unit);
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
        relay: (c_old !== undefined) ? c_old.relay : null,
        timer: null,
        waiting: null,      // {v_jmsg, p_ws} while a request is held
        opening: false      // an open request is in flight
    };
    m_units.set(c_key, c_entry);

    if (c_relay !== null) {
        let v_ms;
        try {
            v_ms = await fn_openOnRelay(c_entry, c_relay);
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
 * No relay reachable: hold the request for udp_relay_pending_s. If a relay
 * authenticates in time the held request is answered normally; otherwise
 * fallback none -> en:false now and an unsolicited en:true once a relay is
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
            fn_log(`no relay available for '${p_entry.unit}', falling back to the built-in proxy`);
            p_entry.state = CONST_UNIT_BUILTIN;
            p_entry.relay = null;
            fn_builtin(v_jmsg, p_ws);
            return;
        }
        fn_log(`no relay available for '${p_entry.unit}', replying en:false (will push en:true when a relay is back)`);
        fn_replyToUnit(p_ws, v_jmsg, fn_disabledPayload());
    }, fn_pendingMs());
}


/**
 * Unit websocket closed: tell its relay the unit is offline (its pair idles
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
    if (c_entry.state === CONST_UNIT_RELAY && c_entry.relay != null && c_entry.relay.fn_isConnected()) {
        c_entry.relay.fn_request('unit_offline', { account: c_id.account, unit: c_id.unit }).catch(function (ex) {
            fn_log(`relay unit_offline for '${c_id.unit}' failed: ${ex.message}`);
        });
        fn_sendRelayStatus(c_entry.relay);
    }
}


/**
 * (Re)opens p_entry on a selected relay (failover, recovery) and tells the
 * unit: the held request is answered, otherwise an unsolicited 9008.
 * Without a healthy relay the unit becomes pending until one authenticates.
 */
async function fn_reopenElsewhere(p_key, p_entry, p_relay) {
    const c_relay = p_relay || fn_selectRelay(p_entry.account, p_entry.unit);
    if (c_relay === null) {
        p_entry.state = CONST_UNIT_PENDING;
        p_entry.relay = null;
        return;
    }
    const c_waiting = p_entry.waiting;
    clearTimeout(p_entry.timer);
    p_entry.timer = null;
    p_entry.waiting = null;
    let v_ms;
    try {
        v_ms = await fn_openOnRelay(p_entry, c_relay);
    } catch (ex) {
        fn_log(`relay re-open for '${p_entry.unit}' on ${c_relay.m_endpoint.id} failed: ${ex.message}`);
        p_entry.state = CONST_UNIT_PENDING;
        p_entry.relay = null;
        return;
    }
    if (m_units.get(p_key) !== p_entry) return;
    if (v_ms.en !== true) m_units.delete(p_key);
    if (c_waiting !== null) {
        fn_replyToUnit(c_waiting.p_ws, c_waiting.v_jmsg, v_ms);
    } else if (v_ms.en === true) {
        fn_pushToUnit(p_entry.account, p_entry.unit, v_ms);
    }
}


/**
 * On every (re)auth of a relay: sync the units it holds, re-open the ones it
 * no longer has (relay restart) on it, and place every pending unit. Units
 * that failed over to another relay stay there (no flapping).
 */
async function fn_onRelayAuthenticated(p_relay) {
    clearTimeout(m_failoverTimers.get(p_relay.m_endpoint.id));
    m_failoverTimers.delete(p_relay.m_endpoint.id);
    if (fn_selectMode() === CONST_SELECT_LEAST_LOADED) fn_refreshLoad(p_relay);

    // Only units known now are handled here; a request arriving while the
    // sync is in flight opens its own pair.
    const c_relayUnits = [];
    const c_candidates = [];
    m_units.forEach(function (p_entry, p_key) {
        if (p_entry.state === CONST_UNIT_RELAY && p_entry.relay === p_relay) {
            c_candidates.push({ key: p_key, entry: p_entry, wasRelay: true });
            c_relayUnits.push({ account: p_entry.account, unit: p_entry.unit });
        } else if (p_entry.state === CONST_UNIT_PENDING) {
            c_candidates.push({ key: p_key, entry: p_entry, wasRelay: false });
        }
    });

    let v_kept = new Set();
    try {
        const c_rep = await p_relay.fn_request('sync', { units: c_relayUnits });
        if (c_rep.success === true && c_rep.ms && Array.isArray(c_rep.ms.kept)) {
            v_kept = new Set(c_rep.ms.kept.map(function (p_u) { return fn_unitKey(p_u.account, p_u.unit); }));
        }
    } catch (ex) {
        fn_log(`relay ${p_relay.m_endpoint.id} sync failed: ${ex.message}`);
        return;
    }

    for (const { key: c_key, entry: c_entry, wasRelay: c_wasRelay } of c_candidates) {
        if (m_units.get(c_key) !== c_entry || c_entry.opening) continue;
        if (c_wasRelay) {
            // still there, or moved/re-requested meanwhile
            if (c_entry.state !== CONST_UNIT_RELAY || c_entry.relay !== p_relay || v_kept.has(c_key)) continue;
            if (!p_relay.fn_isConnected()) return;
            await fn_reopenElsewhere(c_key, c_entry, p_relay);
        } else {
            if (c_entry.state !== CONST_UNIT_PENDING) continue;
            await fn_reopenElsewhere(c_key, c_entry, null);
        }
    }
    fn_sendRelayStatus(p_relay);
}


/**
 * A relay's link is lost: if it is still down after udp_relay_failover_s
 * (its owner grace), its units are opened on another relay.
 */
function fn_onRelayLost(p_relay) {
    const c_id = p_relay.m_endpoint.id;
    clearTimeout(m_failoverTimers.get(c_id));
    const c_timer = setTimeout(function () {
        m_failoverTimers.delete(c_id);
        if (p_relay.fn_isConnected()) return;
        fn_failover(p_relay).catch(function (ex) { fn_log(`failover from ${c_id} failed: ${ex.message}`); });
    }, fn_failoverMs());
    c_timer.unref();
    m_failoverTimers.set(c_id, c_timer);
}


async function fn_failover(p_relay) {
    const c_moving = [];
    m_units.forEach(function (p_entry, p_key) {
        if (p_entry.state === CONST_UNIT_RELAY && p_entry.relay === p_relay) c_moving.push([p_key, p_entry]);
    });
    if (c_moving.length === 0) return;
    fn_log(`relay ${p_relay.m_endpoint.id} down past failover: moving ${c_moving.length} unit(s)`);
    for (const [c_key, c_entry] of c_moving) {
        if (m_units.get(c_key) !== c_entry || c_entry.relay !== p_relay || c_entry.opening) continue;
        await fn_reopenElsewhere(c_key, c_entry, null);
    }
    fn_sendStatus();
}


/**
 * Pushes from a relay, for pairs this comm server owns on that relay:
 *   closed owner_lost -> the relay gave up on us: move the unit (failover)
 *   closed otherwise  -> today's notifyUnitProxyClosed 9008 en:false to the unit
 *   changed           -> unsolicited 9008 en:true with the new sockets
 */
function fn_onRelayPush(p_relay, p_msg) {
    const c_ms = p_msg.ms || {};
    if (c_ms.owner !== fn_cfg().server_id) return;
    const c_key = fn_unitKey(c_ms.account, c_ms.unit);
    const c_entry = m_units.get(c_key);
    // a unit that moved to another relay ignores its old relay's pushes
    if (c_entry === undefined || c_entry.relay !== p_relay) return;

    if (p_msg.mt === 'closed') {
        // Only pairs a unit still wants: a close the unit asked for (en:false)
        // already got its reply.
        if (c_entry.state !== CONST_UNIT_RELAY || c_entry.opening) return;
        if (c_ms.reason === 'owner_lost') {
            fn_reopenElsewhere(c_key, c_entry, null).catch(function (ex) { fn_log(`re-open after owner_lost failed: ${ex.message}`); });
            return;
        }
        m_units.delete(c_key);
        if (fn_pushToUnit(c_ms.account, c_ms.unit, fn_disabledPayload())) {
            fn_log(`notified unit '${c_ms.unit}' that its relay pair was closed (${c_ms.reason})`);
        }
        fn_sendRelayStatus(p_relay);
        return;
    }
    if (p_msg.mt === 'changed') {
        fn_pushToUnit(c_ms.account, c_ms.unit, fn_unitPayload({ en: true, socket1: c_ms.socket1, socket2: c_ms.socket2 }));
    }
}


// ----------------------------------------------------------------------
// GCS-side pinning
// ----------------------------------------------------------------------

/**
 * A GCS joined a group: allow its IP on the relay pairs of that group's units.
 */
function fn_onGcsJoined(p_gcsWs) {
    if (p_gcsWs == null || p_gcsWs.m_loginRequest == null) return;
    const c_ip = p_gcsWs.m_remoteIp;
    if ((typeof c_ip !== 'string') || c_ip.length === 0) return;
    const c_login = p_gcsWs.m_loginRequest;
    m_units.forEach(function (p_entry) {
        if (p_entry.state !== CONST_UNIT_RELAY || p_entry.relay == null || !p_entry.relay.fn_isConnected()) return;
        if (String(p_entry.account) !== String(c_login.m_accountID) || String(p_entry.group) !== String(c_login.m_groupID)) return;
        p_entry.relay.fn_request('peer_allow', { account: p_entry.account, unit: p_entry.unit, ips: [c_ip] }).catch(function (ex) {
            fn_log(`relay peer_allow for '${p_entry.unit}' failed: ${ex.message}`);
        });
    });
}


/**
 * The relay's pinned state of one pair, or null when it has no such pair.
 */
async function fn_getRelayPinned(p_relay, p_account, p_unit) {
    const c_rep = await p_relay.fn_request('list', { account: p_account });
    if (c_rep.success !== true || !c_rep.ms || !Array.isArray(c_rep.ms.pairs)) return null;
    const c_row = c_rep.ms.pairs.find(function (p_row) { return String(p_row.unit) === String(p_unit); });
    return (c_row === undefined) ? null : (c_row.pinned || null);
}


/**
 * 9021 UdpProxyPeer from a GCS: ms {tg_unit, act: 'allow'|'reset'|'query'}.
 * The GCS must be in tg_unit's account+group; allow/reset also need
 * PERMISSION_ALLOW_GCS_FULL_CONTROL (what de_mavlink requires for its
 * UDP-proxy commands). allow -> peer_allow with this GCS's IP, reset ->
 * peer_reset side gcs, on the relay holding the unit. Replies {tg_unit,
 * pinned} (plus applied for allow/reset), or {tg_unit, error}. Without a
 * relay pair (no udp_relay, or the built-in fallback) the unit is on the
 * built-in proxy: mode 'open'.
 */
async function fn_handleGcsPeerRequest(v_jmsg, p_ws) {
    const c_req = (v_jmsg.ms !== null && typeof v_jmsg.ms === 'object') ? v_jmsg.ms : {};
    const c_tgUnit = c_req.tg_unit;
    const c_act = c_req.act;
    const fn_reply = function (p_ms) {
        fn_replyToUnit(p_ws, v_jmsg, Object.assign({ tg_unit: c_tgUnit }, p_ms));
    };

    const c_login = p_ws.m_loginRequest;
    if (c_login == null || c_login.m_actorType !== c_CONSTANTS.CONST_ACTOR_TYPE_GCS) return;   // GCS only, like 9008 is drone only
    if ((typeof c_tgUnit !== 'string') || !['allow', 'reset', 'query'].includes(c_act)) {
        fn_reply({ error: 'bad_request' });
        return;
    }

    const c_unitWs = c_activeSenders.getActiveSender(c_tgUnit);
    if (c_unitWs == null || c_unitWs.m_loginRequest == null
        || String(c_unitWs.m_loginRequest.m_accountID) !== String(c_login.m_accountID)
        || String(c_unitWs.m_loginRequest.m_groupID) !== String(c_login.m_groupID)) {
        fn_reply({ error: 'not_allowed' });
        return;
    }
    const c_full = c_CONSTANTS.CONST_PERMISSION_ALLOW_GCS_FULL_CONTROL;
    if (c_act !== 'query' && ((Number(c_login.m_prm) & c_full) !== c_full)) {
        fn_reply({ error: 'permission_denied' });
        return;
    }

    const c_account = c_unitWs.m_loginRequest.m_accountID;
    const c_entry = m_units.get(fn_unitKey(c_account, c_tgUnit));
    if (!fn_isEnabled() || c_entry === undefined || c_entry.state === CONST_UNIT_BUILTIN) {
        fn_reply({ pinned: { gcs: { mode: 'open' } } });
        return;
    }
    const c_relay = c_entry.relay;
    if (c_relay == null || !c_relay.fn_isConnected()) {
        fn_reply({ error: 'relay_unavailable' });
        return;
    }

    try {
        let v_applied;
        if (c_act === 'allow') {
            if (typeof p_ws.m_remoteIp !== 'string' || p_ws.m_remoteIp.length === 0) {
                fn_reply({ error: 'no_ip' });
                return;
            }
            const c_rep = await c_relay.fn_request('peer_allow', { account: c_account, unit: c_tgUnit, ips: [p_ws.m_remoteIp] });
            v_applied = (c_rep.success === true && c_rep.ms && c_rep.ms.applied === true);
        } else if (c_act === 'reset') {
            const c_rep = await c_relay.fn_request('peer_reset', { account: c_account, unit: c_tgUnit, side: 'gcs' });
            v_applied = (c_rep.success === true && c_rep.ms && c_rep.ms.applied === true);
        }
        const c_pinned = await fn_getRelayPinned(c_relay, c_account, c_tgUnit);
        const c_ms = { pinned: c_pinned };
        if (v_applied !== undefined) c_ms.applied = v_applied;
        fn_reply(c_ms);
    } catch (ex) {
        fn_log(`9021 ${c_act} for '${c_tgUnit}' failed: ${ex.message}`);
        fn_reply({ error: 'relay_unavailable' });
    }
}


// ----------------------------------------------------------------------
// AUTH dashboard (e / g)
// ----------------------------------------------------------------------

/**
 * Pairs this comm server owns on p_relay, in fn_getActiveProxiesList's row
 * shape plus account/group/owner/relay_id/online/rx/tx.
 */
async function fn_listRelay(p_relay) {
    let v_rep;
    try {
        v_rep = await p_relay.fn_request('list', {});
    } catch (ex) {
        fn_log(`relay ${p_relay.m_endpoint.id} list failed: ${ex.message}`);
        return [];
    }
    if (v_rep.success !== true || !v_rep.ms || !Array.isArray(v_rep.ms.pairs)) return [];
    const c_me = fn_cfg().server_id;
    return v_rep.ms.pairs.filter(function (p_row) { return p_row.owner === c_me; }).map(function (p_row) {
        const c_row = {
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
            tx: p_row.tx,
            m_relay: p_relay
        };
        // forward the GCS/unit pin state so the AUTH dashboard can show it
        if (p_row.pinned != null) c_row.pinned = p_row.pinned;
        return c_row;
    });
}


/**
 * Relay pairs owned by this comm server on every reachable relay. [] when no
 * relay is configured or reachable.
 */
async function fn_getRelayProxiesList() {
    const c_lists = await Promise.all(fn_healthyRelays().map(fn_listRelay));
    return [].concat(...c_lists).map(function (p_row) {
        const c_row = Object.assign({}, p_row);
        delete c_row.m_relay;
        return c_row;
    });
}


/**
 * Blocks/unblocks the relay pairs this comm server owns for unit p_name
 * (optionally only in account p_account), on whichever relays hold them.
 * @returns Promise<boolean> applied to at least one pair
 */
async function fn_setRelayProxyBlocked(p_name, p_blocked, p_account) {
    const c_lists = await Promise.all(fn_healthyRelays().map(fn_listRelay));
    const c_rows = [].concat(...c_lists).filter(function (p_row) {
        return p_row.name === p_name && (p_account == null || String(p_row.account) === String(p_account));
    });
    let v_applied = false;
    for (const c_row of c_rows) {
        try {
            const c_rep = await c_row.m_relay.fn_request('block', { account: c_row.account, unit: c_row.name, blocked: p_blocked === true });
            if (c_rep.success === true && c_rep.ms && c_rep.ms.applied === true) v_applied = true;
        } catch (ex) {
            fn_log(`relay block for '${p_name}' failed: ${ex.message}`);
        }
    }
    return v_applied;
}


/**
 * Each relay's link state ({id, status, connected}), for tests and diagnostics.
 */
function fn_getRelayStates() {
    return m_relays.map(function (p_relay) {
        return { id: p_relay.m_endpoint.id, status: p_relay.m_state, connected: p_relay.fn_isConnected() };
    });
}


/**
 * Where each wanted unit is ({account, unit, state, relay_id}), for tests and diagnostics.
 */
function fn_getUnitPlacement() {
    const c_rows = [];
    m_units.forEach(function (p_entry) {
        c_rows.push({
            account: p_entry.account,
            unit: p_entry.unit,
            state: p_entry.state,
            relay_id: (p_entry.relay != null) ? p_entry.relay.m_endpoint.id : null
        });
    });
    return c_rows;
}


module.exports = {
    CUdpRelayConnection,
    fn_isEnabled,
    fn_initialize,
    fn_shutdown,
    fn_isConnected,
    fn_selectRelayId,
    fn_getUnitPlacement,
    fn_getRelayStates,
    fn_setSendToAuthCallback,
    fn_sendStatus,
    fn_handleUnitRequest,
    fn_onUnitDisconnected,
    fn_onGcsJoined,
    fn_handleGcsPeerRequest,
    fn_getRelayProxiesList,
    fn_setRelayProxyBlocked,
    fn_listRelay
};
