"use strict";

/**
 * This module acts as a client for the Communication-Server Manager,
 * used by the Auth Server for communication with AndruavAuth.
 */

const c_s2s_auth = require('./js_s2s_auth.js');

const CONST_S2S_WS_RETRY_TIME = 2000;
let m_ws;
let Me;

function fn_onOpen_Handler() {
    console.log(`${global.Colors.BSuccess}[OK] Connection established with AuthServer${global.Colors.Reset}`);

    // S2S auth is always enabled - wait for the AuthServer challenge and answer it
    // before announcing ourselves. fn_updateAuthServer (which sends our INFO card) is
    // therefore deferred until the handshake completes (see fn_onMessage_Handler).
    return;
}

/**
 * Called when the websocket is closed. 
 * The websocket is NEVER closed on purpose, indicating a disconnect.
 * A retry function is called with a rate of CONST_S2S_WS_RETRY_TIME.
 */
function fn_onClose_Handler() {
    console.log(`${global.Colors.Error}ATTENTION!! Connection closed with AuthServer${global.Colors.Reset}`);

    const c_url = `wss://${global.m_serverconfig.m_configuration.s2s_ws_target_ip}:${global.m_serverconfig.m_configuration.s2s_ws_target_port}`;
    setTimeout(() => fn_startWebSocketListener(c_url), CONST_S2S_WS_RETRY_TIME);
}

function fn_onError_Handler(err) {
    console.error(`${global.Colors.Error}ATTENTION!! Connection error with AuthServer: ${err}${global.Colors.Reset}`);
}

/**
 * Handles incoming messages from AndruavAuth.
 * @param {*} data 
 */
function fn_onMessage_Handler(data) {
    // Answer the AuthServer S2S challenge by signing the nonce with our private key.
    // Once answered, announce ourselves (fn_updateAuthServer sends the INFO card).
    const c_env = c_s2s_auth.fn_parseEnvelope(data);
    if (c_env != null) {
        if (c_env.s2s_auth === c_s2s_auth.CONST_S2S_AUTH_CHALLENGE) {
            try {
                m_ws.send(c_s2s_auth.fn_buildResponse(c_env.nonce, global.m_serverconfig.m_configuration.server_id));
                if (Me.fn_updateAuthServer) {
                    Me.fn_updateAuthServer();
                }
            }
            catch (ex) {
                console.error(`${global.Colors.Error}ATTENTION!! S2S handshake failed (check s2s_my_private_key): ${ex}${global.Colors.Reset}`);
                m_ws.close();
            }
        }
        return;
    }

    if (Me.fn_onMessageReceived) {
        Me.fn_onMessageReceived(data);
    }
}

function fn_startWebSocketListener(p_url) {
    const WebSocket = require('ws');
    m_ws = new WebSocket(p_url);
    m_ws.on('open', fn_onOpen_Handler);
    m_ws.on('close', fn_onClose_Handler);
    m_ws.on('message', fn_onMessage_Handler);
    m_ws.on('error', fn_onError_Handler);
}

/**
 * Starts the communication server manager client module.
 */
function fn_startServer() {
    Me = this;
    const c_url = `wss://${global.m_serverconfig.m_configuration.s2s_ws_target_ip}:${global.m_serverconfig.m_configuration.s2s_ws_target_port}`;

    console.log(`${global.Colors.BSuccess}[OK] Comm Server Manager Client connecting to ${c_url} to reach AndruavAuth${global.Colors.Reset}`);
    fn_startWebSocketListener(c_url);
}

function fn_sendMessage(p_message) {
    if (m_ws) {
        m_ws.send(p_message);
    }
}

// Expose message handlers
exports.fn_onMessageReceived = undefined;
exports.fn_updateAuthServer = undefined;

module.exports = {
    fn_startServer,
    fn_sendMessage
};