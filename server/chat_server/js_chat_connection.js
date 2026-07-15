"use strict";

/**
 * Connection module.
 * Handles the WebSocket connection lifecycle: key validation, connection
 * acceptance (local and auth-server flows), socket event wiring, and the
 * HTTPS/WebSocket server bootstrap.
 * Extracted from js_andruav_chat_server.js (behavior-preserving).
 */

const c_CONSTANTS = require("../../js_constants.js");
const hlp_strings = require("../../helpers/hlp_strings.js");

const c_ChatAccountRooms = require("./js_andruav_chat_account_rooms.js");
const c_CommServerManagerClient = require("../js_comm_server_manager_client.js");
const c_andruav_comm_server = require("../js_andruav_comm_server.js");
const c_andruav_active_senders = require("./js_andruav_active_senders.js");
const c_routing = require("./js_chat_routing.js");


function getHeaderParams(url) {
    let regex = /[?&]([^=#]+)=([^&#]*)/g,
        params = {},
        match;


    while (match = regex.exec(url)) {
        params[match[1]] = match[2];
    }

    return params;
}


function _acceptConnection(c_onb, p_ws) {
    const c_status = {};
    c_status.m_TTX = 0;
    c_status.m_BTX = 0;

    p_ws.m_loginRequest = c_onb;
    p_ws.m_status = c_status;

    c_andruav_active_senders.addActiveSenderIDList(p_ws.m_loginRequest.m_senderID, p_ws);
    c_ChatAccountRooms.fn_del_member_fromAccountByName(p_ws.m_loginRequest, true);
    c_ChatAccountRooms.fn_add_member_to_AccountGroup(p_ws);

    send_ok_message(p_ws);

}


/**
 * Accepts a local connection that directly connects to local comm server.
 * @param {*} c_params
 * @param {*} p_ws
 */
function acceptLocalConnection(c_params, p_ws) {
    const sender_id = c_params[c_CONSTANTS.CONST_CS_SENDER_ID.toString()];
    const c_onb = {};
    c_onb.m_senderID = sender_id;
    c_onb.m_accountID = global.m_serverconfig.m_configuration.local_server_account_id == null ? '1' : global.m_serverconfig.m_configuration.local_server_account_id;
    c_onb.m_groupID = '1';
    c_onb.m_requestID = sender_id;  // !Change it to a random number
    c_onb.m_actorType = c_params.at == null ? 'a' : c_params.at;  // use suggested actor type.
    c_onb.m_prm = 0xffffffff;
    c_onb.m_creationDate = Date.now();


    // Seal the object to prevent adding or removing properties, but allow modifying existing properties
    Object.seal(c_onb);

    _acceptConnection(c_onb, p_ws);


}


/**
 * Accepts a connection from AuthServer after validating the key.
 * @param {*} v_loginTempKey
 * @param {*} c_params
 * @param {*} p_ws
 */
function acceptConnection(v_loginTempKey, c_params, p_ws) {
    try {
        const c_loginRequest = c_andruav_comm_server.getLogin(v_loginTempKey);
        if (c_loginRequest != null) {
            const c_onb = {};
            c_onb.m_senderID = c_params[c_CONSTANTS.CONST_CS_SENDER_ID.toString()]; // !THIS IS WRONG. SenderID should be as a SYS message to allow encryption
            c_onb.m_accountID = c_loginRequest[c_CONSTANTS.CONST_CS_ACCOUNT_ID.toString()];
            c_onb.m_groupID = c_loginRequest[c_CONSTANTS.CONST_CS_GROUP_ID.toString()];
            c_onb.m_requestID = c_loginRequest[c_CONSTANTS.CONST_CS_REQUEST_ID.toString()];
            c_onb.m_actorType = c_loginRequest[c_CONSTANTS.CONST_ACTOR_TYPE.toString()];
            c_onb.m_prm = c_loginRequest[c_CONSTANTS.CONST_PERMISSION2.toString()];
            c_onb.m_creationDate = Date.now();


            Object.seal(c_onb);

            // delete from waiting list.
            c_andruav_comm_server.deleteLogin(v_loginTempKey);

            _acceptConnection(c_onb, p_ws);

            c_CommServerManagerClient.fn_updateAuthServer(); // give feedback to AUTH server
        }
        else {
            p_ws.m_loginRequest = null;
            p_ws.close();
        }
    }
    catch (ex) {
        // exception handling to be removed.
        console.log("TEMP DEBUG EXCEPTION ... Ex:" + JSON.stringify(ex));
        if (global.m_logger) global.m_logger.Error('Party WS Error', 'fn_onConnect_Handler', null, ex);
    }
}


/**
 * MAIN Function called by WebSocket when a client "party" connects.
 * @param {Socket for Each Connection} p_ws
 * @param {request parameters} p_req
 */
function fn_onConnect_Handler(p_ws, p_req) {
    const c_WS = p_ws;
    const c_params = getHeaderParams(p_req.url);
    let v_loginTempKey;

    if (global.m_logger) global.m_logger.Info('WS Created from Party', 'fn_onConnect_Handler', null, c_params);


    /**
     * Make sure that connection has a key and in valid format and in the waiting list "m_waitingAccounts".
     * @param {*} p_params
     */
    function fn_validateKey(p_params) {

        const c_PARAM_LENGTH = 200;
        if (p_params == null) {
            if (global.m_logger) global.m_logger.Warn('Party tried to login using no credentials', 'fn_validateKey', null, p_params);
            return false;
        }

        if (c_params.hasOwnProperty(c_CONSTANTS.CONST_CS_LOGIN_TEMP_KEY.toString()) === true) {
            /*
            * Agent is connecting using  CONST_CS_LOGIN_TEMP_KEY that is originally generated by AndruavServer
            * and sent back to AndruavAuth.
            */
            v_loginTempKey = p_params[c_CONSTANTS.CONST_CS_LOGIN_TEMP_KEY.toString()].toString();
            if ((hlp_strings.fn_isAlphanumeric(v_loginTempKey) !== true) || (v_loginTempKey.length > c_PARAM_LENGTH)) {
                c_WS.close();
                if (global.m_logger) global.m_logger.Warn('Party tried to login using bad credentials', 'fn_validateKey', null, p_params);
                return false;
            }
        }


        if (!c_andruav_comm_server.isLoginExist(v_loginTempKey)) {
            // UNAUTHERIZED LOGIN or REPATED Login with same Key .. close the connection.
            console.log("debug INVALID v_loginTempKey .." + v_loginTempKey)

            c_WS.close();
            if (global.m_logger) global.m_logger.Warn('Party failed to login', 'fn_validateKey', null, p_params);
            return false;
        }
        else {

            console.log("debug valid v_loginTempKey .." + v_loginTempKey)
            if (global.m_logger) global.m_logger.Info('Party successfully login', 'fn_validateKey', null, p_params);

            return true;
        }


    }


    function fn_validateKeyLocal(p_params) {
        //TODO: you can use a fixed RequestID in the config.
        // or Master Drone can send an KEY to followers to use it when connect.
        // A system message can set this KEY in  comm_srver.
        v_loginTempKey = p_params[c_CONSTANTS.CONST_CS_LOGIN_TEMP_KEY.toString()].toString();

        return true;
    }


    function fn_onWsMessage(p_msg, v_isBinary) {
        c_routing.fn_parseMessage(this, p_msg, v_isBinary);
        p_msg = null;
    }
    
    function fn_onWsClose(p_code) {
        // this function can be called during key validation.
        // also this function is called when terminated a socket when a senderID kicks out an older unit with same senderID.
        // in this case m__terminated = true other wise the new senderID will also kick itself.
        //console.log ("debug ... fn_onWsClose code: " + p_code + " of key " + v_loginTempKey);
        if ((this.hasOwnProperty('m__terminated') == false) || (this.m__terminated == false)) {
            c_ChatAccountRooms.fn_del_member_fromAccountByName(p_ws.m_loginRequest, true);
        }

        // remove from active senderIDs list and notify auth server.
        if (p_ws.m_loginRequest != null) {
            c_andruav_active_senders.deleteActiveSenderIDList(p_ws.m_loginRequest.m_senderID);
            
            // Send logout notification to auth server
            const c_logout_msg = {
                'c': c_CONSTANTS.CONST_CS_CMD_LOGOUT_REQUEST,
                'd': {}
            };
            c_logout_msg.d[c_CONSTANTS.CONST_CS_SENDER_ID.toString()] = p_ws.m_loginRequest.m_senderID;
            c_CommServerManagerClient.fn_sendMessage(JSON.stringify(c_logout_msg));
            
            if (c_CommServerManagerClient.fn_updateAuthServer) {
                c_CommServerManagerClient.fn_updateAuthServer();
            }
        }
    }


    function fn_onWsError(p_err) {
        console.log("debug ... fn_onWsError err: " + p_err);
        if (global.m_logger) global.m_logger.Error('Party WS Error', 'fn_onWsError', null, p_err);
    }

    function fn_onWsUpgrade(r) {

    }

    function fn_onWsHeaders(r) {

    }


    p_ws.on('message', fn_onWsMessage.bind(p_ws));
    p_ws.on('close', fn_onWsClose.bind(p_ws));
    p_ws.on('error', fn_onWsError.bind(p_ws));
    p_ws.on('upgrade', fn_onWsUpgrade.bind(p_ws));
    p_ws.on('headers', fn_onWsHeaders.bind(p_ws));

    if (global.m_serverconfig.m_configuration.local_server_enabled === true) {
        // Local Server is enabled. No need to wait for AndruavAuth to connect.
        // We assume the connection setup is straightforward and generate our local data.
        fn_validateKeyLocal(c_params);

        if (v_loginTempKey != null) {
            // OK THIS IS A VALID LOGIN... Lets' get him in the right chat room and send a welcome reply.
            acceptLocalConnection(c_params, p_ws);
        }
        else {
            //delete m_waitingAccounts [v_loginTempKey];  v_loginTempKey is already null.
            p_ws.m_loginRequest = null;
            p_ws.close();
        }

    }
    else {

        fn_validateKey(c_params);

        if (v_loginTempKey != null) {
            // OK THIS IS A VALID LOGIN... Lets' get him in the right chat room and send a welcome reply.

            acceptConnection(v_loginTempKey, c_params, p_ws);
        }
        else {
            //delete m_waitingAccounts [v_loginTempKey];  v_loginTempKey is already null.
            p_ws.m_loginRequest = null;
            p_ws.close();
        }
    }

}


// Send OK Message to Newly Connected Socket.
function send_ok_message(p_ws) {

    const v_jmsg = {
        'ty': 's',
        'mt': c_CONSTANTS.CONST_TYPE_AndruavSystem_ConnectedCommServer,
        'ms': { s: 'OK:connected:tcp:' + p_ws._socket.remoteAddress + ':' + p_ws._socket.remotePort }
    };

    p_ws.send(JSON.stringify(v_jmsg));
}


function fn_startChatServer() {
    const v_express = require('express');
    const v_fs = require('fs');
    const v_path = require('path');
    const v_WebSocketServer = require('ws').Server;
    const c_https = require('https');

    // HTTPS server options
    const v_keyPath = v_path.isAbsolute(global.m_serverconfig.m_configuration.ssl_key_file.toString()) ? global.m_serverconfig.m_configuration.ssl_key_file.toString() : v_path.join(__dirname, '../../', global.m_serverconfig.m_configuration.ssl_key_file.toString());
    const v_certPath = v_path.isAbsolute(global.m_serverconfig.m_configuration.ssl_cert_file.toString()) ? global.m_serverconfig.m_configuration.ssl_cert_file.toString() : v_path.join(__dirname, '../../', global.m_serverconfig.m_configuration.ssl_cert_file.toString());
    
    let v_keyFile, v_certFile;
    let v_hasError = false;
    try {
        v_keyFile = v_fs.readFileSync(v_keyPath);
    } catch (e) {
        console.log(global.Colors.Error + "FATAL ERROR: Cannot read SSL key file: " + global.Colors.Error + v_keyPath + global.Colors.Reset);
        v_hasError = true;
    }
    
    try {
        v_certFile = v_fs.readFileSync(v_certPath);
    } catch (e) {
        console.log(global.Colors.Error + "FATAL ERROR: Cannot read SSL cert file: " + global.Colors.Error + v_certPath + global.Colors.Reset);
        v_hasError = true;
    }

    if (v_hasError == true)
    {
        process.exit(1);
    }
    
    const options = {
        key: v_keyFile,
        cert: v_certFile
    };

    // Create HTTPS server with Express
    const app = new v_express();
    const wserver = c_https.createServer(options, app);

    // Start HTTPS server
    wserver.listen(
        global.m_serverconfig.m_configuration.server_port,
        global.m_serverconfig.m_configuration.server_ip,
        () => {
            console.log(`HTTPS server started on ${global.m_serverconfig.m_configuration.server_ip}:${global.m_serverconfig.m_configuration.server_port}`);
        }
    );

    // Initialize WebSocket server with compression
    let v_wss;
    if (global.m_serverconfig.m_configuration.ws_compression === true) {
        v_wss = new v_WebSocketServer({
            server: wserver,
            perMessageDeflate: {
                zlibDeflateOptions: {
                    chunkSize: 1024,
                    memLevel: 7,
                    level: 3 // 1-9; lower is faster, higher is smaller
                },
                zlibInflateOptions: {
                    chunkSize: 10 * 1024
                },
                threshold: 10 // Compress messages > 1KB
            }
        });
    }
    else {
        v_wss = new v_WebSocketServer({
            server: wserver
        });
    }

    // WebSocket connection handler
    v_wss.on('connection', (ws, req) => {
        console.log(`WebSocket client connected from ${req.socket.remoteAddress}`);
        fn_onConnect_Handler(ws, req); // Call your existing handler
    });

    // Error handling for WebSocket server
    v_wss.on('error', (error) => {
        console.error('WebSocket server error:', error);
    });

    // Return the WebSocket server instance (optional, for further use)
    return v_wss;
}


module.exports = {
    getHeaderParams,
    acceptLocalConnection,
    acceptConnection,
    fn_onConnect_Handler,
    send_ok_message,
    fn_startChatServer
};
