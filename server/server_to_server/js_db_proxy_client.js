"use strict";

/**
 * DBProxyClient
 *
 * Connects the comm server OUT to a storage (DB) server endpoint configured
 * in server.config (storage_server_host, storage_server_port) and forwards
 * LoadTasks/SaveTasks/DeleteTasks/DisableTasks requests, translating them into
 * the storage server's {mt, ms, rid} wire protocol (see droneengage_storage_server/src/messageHandlers.js).
 *
 * Reuses the same S2S Ed25519 challenge-response handshake already used to trust
 * AUTH (see js_s2s_auth.js) - the comm server signs the storage server's challenge
 * with its existing s2s_my_private_key/server_id, which the storage server must
 * have configured under s2s_trusted_server_keys.
 *
 * Singleton: a comm server talks to a single active storage server at a time.
 * The endpoint is read from config on startup via fn_initialize().
 */

const WebSocket = require('ws');
const c_CONSTANTS = require("../../js_constants.js");
const { v4: uuidv4 } = require('uuid');
const c_s2s_auth = require('../js_s2s_auth.js');

const CONST_REQUEST_TIMEOUT_MS = 10000;
const CONST_RETRY_TIME_MS = 5000;
const CONST_HEARTBEAT_INTERVAL_MS = 30000; // 30 seconds
const CONST_MAX_RECONNECT_DELAY_MS = 60000; // Max 60 seconds
const CONST_INITIAL_RECONNECT_DELAY_MS = 1000; // Start with 1 second

// Connection states
const CONST_CONNECTION_STATE = {
    DISCONNECTED: 'disconnected',
    CONNECTING: 'connecting',
    CONNECTED: 'connected',
    UNHEALTHY: 'unhealthy'
};

let m_ws = null;
let m_authenticated = false;
let m_currentEndpoint = null; // { host, port, server_id }
let m_reconnectTimer = null;
let m_heartbeatTimer = null;
let m_sendToAuthCallback = null; // Callback to send messages to AUTH
let m_pendingRequests = {}; // rid -> { resolve, reject, timeout }
let m_reconnectAttempts = 0; // Track reconnect attempts for exponential backoff
let m_connectionState = CONST_CONNECTION_STATE.DISCONNECTED; // Current connection state
let m_s2sAuthTimer = null; // Timer for S2S auth challenge timeout

function fn_clearPending(p_error) {
    const c_keys = Object.keys(m_pendingRequests);
    for (let i = 0; i < c_keys.length; ++i) {
        const c_req = m_pendingRequests[c_keys[i]];
        clearTimeout(c_req.timeout);
        c_req.reject(p_error || new Error('DBProxyClient connection closed'));
    }
    m_pendingRequests = {};
}

function fn_onOpen() {
    console.log(`${global.Colors.BSuccess}[OK] DBProxyClient connected to storage server${global.Colors.Reset}`);
    m_reconnectAttempts = 0; // Reset reconnect attempts on successful connection
    m_connectionState = CONST_CONNECTION_STATE.CONNECTING;
    fn_sendStorageStatus('connecting');

    // If storage server doesn't send S2S challenge within 2 seconds, assume no auth required
    if (m_s2sAuthTimer) {
        clearTimeout(m_s2sAuthTimer);
    }
    m_s2sAuthTimer = setTimeout(() => {
        if (!m_authenticated && m_ws && m_ws.readyState === WebSocket.OPEN) {
            console.log(`${global.Colors.FgYellow}[INFO] DBProxyClient: No S2S challenge received, assuming no auth required${global.Colors.Reset}`);
            m_authenticated = true;
            m_connectionState = CONST_CONNECTION_STATE.CONNECTED;
            fn_sendStorageStatus('connected');
            fn_startHeartbeat();
        }
    }, 2000);
}

function fn_onMessage(data) {
    const c_env = c_s2s_auth.fn_parseEnvelope(data);
    if (c_env != null) {
        if ((c_env.s2s_auth === c_s2s_auth.CONST_S2S_AUTH_CHALLENGE) && (m_ws != null)) {
            // Clear the timeout since we received a challenge
            if (m_s2sAuthTimer) {
                clearTimeout(m_s2sAuthTimer);
                m_s2sAuthTimer = null;
            }
            try {
                m_ws.send(c_s2s_auth.fn_buildResponse(c_env.nonce, global.m_serverconfig.m_configuration.server_id));
            }
            catch (ex) {
                console.error(`${global.Colors.Error}ATTENTION!! DBProxyClient S2S handshake failed: ${ex}${global.Colors.Reset}`);
                m_ws.close();
            }
        }
        return;
    }

    let v_msg;
    try {
        v_msg = JSON.parse(data);
    }
    catch (ex) {
        console.error(`err: DBProxyClient could not parse message: ${ex}`);
        return;
    }

    if (v_msg.type === 'auth_success') {
        // Clear the timeout since auth succeeded
        if (m_s2sAuthTimer) {
            clearTimeout(m_s2sAuthTimer);
            m_s2sAuthTimer = null;
        }
        m_authenticated = true;
        m_connectionState = CONST_CONNECTION_STATE.CONNECTED;
        console.log(`${global.Colors.BSuccess}[OK] DBProxyClient authenticated with storage server${global.Colors.Reset}`);
        fn_sendStorageStatus('connected');
        fn_startHeartbeat();
        return;
    }

    if (v_msg.type === 'error') {
        console.error(`${global.Colors.Error}DBProxyClient received error from storage server: ${v_msg.error}${global.Colors.Reset}`);
        return;
    }

    // Response to a forwarded task request: correlate using rid.
    if ((v_msg.rid != null) && (m_pendingRequests.hasOwnProperty(v_msg.rid))) {
        const c_req = m_pendingRequests[v_msg.rid];
        clearTimeout(c_req.timeout);
        delete m_pendingRequests[v_msg.rid];
        c_req.resolve(v_msg);
    }
}

function fn_onClose() {
    console.log(`${global.Colors.Error}ATTENTION!! DBProxyClient disconnected from storage server${global.Colors.Reset}`);
    m_authenticated = false;
    m_ws = null;
    m_connectionState = CONST_CONNECTION_STATE.DISCONNECTED;
    fn_stopHeartbeat();
    fn_sendStorageStatus('disconnected', 'Connection closed');
    fn_clearPending(new Error('DBProxyClient disconnected'));

    // Clear S2S auth timer
    if (m_s2sAuthTimer) {
        clearTimeout(m_s2sAuthTimer);
        m_s2sAuthTimer = null;
    }

    if (m_currentEndpoint != null) {
        // Calculate exponential backoff delay with jitter to avoid synchronized reconnect storms
        m_reconnectAttempts++;
        const c_baseDelay = Math.min(
            CONST_INITIAL_RECONNECT_DELAY_MS * Math.pow(2, m_reconnectAttempts - 1),
            CONST_MAX_RECONNECT_DELAY_MS
        );
        // Add jitter: +/- 25% of base delay
        const c_jitter = c_baseDelay * 0.25 * (Math.random() * 2 - 1);
        const c_delay = Math.floor(c_baseDelay + c_jitter);
        console.log(`${global.Colors.FgYellow}[INFO] DBProxyClient reconnecting in ${c_delay}ms (attempt ${m_reconnectAttempts})${global.Colors.Reset}`);
        m_reconnectTimer = setTimeout(() => fn_connectInternal(), c_delay);
    }
}

function fn_onError(err) {
    console.error(`${global.Colors.Error}DBProxyClient WebSocket error: ${err}${global.Colors.Reset}`);
    m_connectionState = CONST_CONNECTION_STATE.UNHEALTHY;
    fn_sendStorageStatus('error', err.message);
    // Close the socket to trigger reconnect logic
    if (m_ws != null) {
        try { m_ws.close(); } catch (ex) { /* ignore */ }
    }
}

function fn_connectInternal() {
    if (m_currentEndpoint == null) return;

    if (m_reconnectTimer != null) {
        clearTimeout(m_reconnectTimer);
        m_reconnectTimer = null;
    }

    m_connectionState = CONST_CONNECTION_STATE.CONNECTING;
    
    // Storage server always uses SSL (wss://)
    const c_protocol = 'wss';
    const c_url = `${c_protocol}://${m_currentEndpoint.host}:${m_currentEndpoint.port}`;
    console.log(`${global.Colors.FgYellow}DBProxyClient connecting to storage server at ${c_url}${global.Colors.Reset}`);

    const c_wsOptions = {
        rejectUnauthorized: false // Allow self-signed certificates (same as AUTH connection)
    };

    m_ws = new WebSocket(c_url, c_wsOptions);
    m_ws.on('open', fn_onOpen);
    m_ws.on('message', fn_onMessage);
    m_ws.on('close', fn_onClose);
    m_ws.on('error', fn_onError);
    m_ws.on('ping', () => {
        if (m_ws != null && m_ws.readyState === m_ws.OPEN) {
            try { m_ws.pong(); } catch (ex) { /* ignore */ }
        }
    });
}

/**
 * Initialize DBProxyClient by reading storage server endpoint from config.
 * Should be called during comm server startup.
 */
function fn_initialize() {
    // Check if storage server is enabled
    if (global.m_serverconfig.m_configuration.enable_storage_server !== true) {
        console.log(`${global.Colors.FgYellow}[INFO] DBProxyClient: storage server disabled in config${global.Colors.Reset}`);
        return;
    }

    const c_host = global.m_serverconfig.m_configuration.storage_server_host;
    const c_port = global.m_serverconfig.m_configuration.storage_server_port;
    
    if (c_host && c_port) {
        console.log(`${global.Colors.FgYellow}[INFO] DBProxyClient configured for storage server at ${c_host}:${c_port}${global.Colors.Reset}`);
        fn_connect(c_host, c_port, 'configured-storage-server');
    } else {
        console.log(`${global.Colors.Error}[ATTENTION!!] DBProxyClient: storage_server_host or storage_server_port not configured${global.Colors.Reset}`);
    }
}

/**
 * Set callback for sending messages to AUTH server.
 * This is used to report storage connection status.
 * @param {function} p_callback - Function that takes a message string to send to AUTH
 */
function fn_setSendToAuthCallback(p_callback) {
    m_sendToAuthCallback = p_callback;
}

/**
 * Send storage status message to AUTH
 * @param {string} p_status - 'connected', 'disconnected', or 'error'
 * @param {string} p_error - Optional error message
 */
function fn_sendStorageStatus(p_status, p_error = null) {
    if (!m_sendToAuthCallback) {
        return; // No callback set, can't send to AUTH
    }

    const c_cmd = {
        c: c_CONSTANTS.CONST_CS_CMD_STORAGE_STATUS,
        d: {
            status: p_status,
            storage_host: m_currentEndpoint ? m_currentEndpoint.host : null,
            storage_port: m_currentEndpoint ? m_currentEndpoint.port : null,
            timestamp: Math.floor(Date.now() / 1000),
            error: p_error
        }
    };

    try {
        m_sendToAuthCallback(JSON.stringify(c_cmd));
    } catch (ex) {
        console.error(`${global.Colors.Error}DBProxyClient: Failed to send storage status to AUTH: ${ex}${global.Colors.Reset}`);
    }
}

/**
 * Start periodic heartbeat to AUTH
 */
function fn_startHeartbeat() {
    if (m_heartbeatTimer) {
        clearInterval(m_heartbeatTimer);
    }

    m_heartbeatTimer = setInterval(() => {
        const c_status = m_authenticated ? 'connected' : 'disconnected';
        fn_sendStorageStatus(c_status);
    }, CONST_HEARTBEAT_INTERVAL_MS);
}

/**
 * Stop periodic heartbeat
 */
function fn_stopHeartbeat() {
    if (m_heartbeatTimer) {
        clearInterval(m_heartbeatTimer);
        m_heartbeatTimer = null;
    }
}

/**
 * (Re)connect to a storage server endpoint.
 * @param {string} p_host
 * @param {number} p_port
 * @param {string} p_serverId informational, id of the storage server (for logging).
 */
function fn_connect(p_host, p_port, p_serverId) {
    if ((m_currentEndpoint != null) && (m_currentEndpoint.host === p_host) && (m_currentEndpoint.port === p_port)) {
        // Already connected/connecting to this endpoint.
        return;
    }

    if (m_ws != null) {
        try { m_ws.close(); } catch (ex) { /* ignore */ }
        m_ws = null;
    }

    m_authenticated = false;
    m_currentEndpoint = { host: p_host, port: p_port, server_id: p_serverId };
    fn_connectInternal();
}

function fn_isConnected() {
    return (m_ws != null) && (m_ws.readyState === WebSocket.OPEN) && (m_authenticated === true);
}

/**
 * Get the current connection state
 * @returns {string} One of: 'disconnected', 'connecting', 'connected', 'unhealthy'
 */
function fn_getConnectionState() {
    return m_connectionState;
}

/**
 * Forwards a task request (LoadTasks/SaveTasks/DeleteTasks/DisableTasks) to the
 * storage server and returns a Promise resolving with the parsed response envelope
 * ({mt, ms, success, error?}).
 * @param {number} p_mt AndruavSystem message type (9001-9004).
 * @param {object} p_ms payload (unitId, tasks/taskIds, ...).
 */
function fn_sendRequest(p_mt, p_ms) {
    return new Promise((resolve, reject) => {
        if (!fn_isConnected()) {
            reject(new Error('DBProxyClient is not connected to a storage server'));
            return;
        }

        const c_rid = uuidv4();
        const c_timeout = setTimeout(() => {
            delete m_pendingRequests[c_rid];
            reject(new Error('DBProxyClient request timed out'));
        }, CONST_REQUEST_TIMEOUT_MS);

        m_pendingRequests[c_rid] = { resolve, reject, timeout: c_timeout };

        try {
            m_ws.send(JSON.stringify({ mt: p_mt, ms: p_ms, rid: c_rid }));
        }
        catch (ex) {
            clearTimeout(c_timeout);
            delete m_pendingRequests[c_rid];
            reject(ex);
        }
    });
}

module.exports = {
    fn_initialize,
    fn_connect,
    fn_isConnected,
    fn_getConnectionState,
    fn_sendRequest,
    fn_setSendToAuthCallback
};
