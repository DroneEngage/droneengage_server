"use strict";

/**
 * QueryServer module.
 * Handles CONST_TYPE_AndruavSystem_QueryServer messages from units.
 * The unit sends a subcommand in v_jmsg.ms.sc to query server state.
 * The comm server replies with a CONST_TYPE_AndruavSystem_StateServer message
 * containing the result.
 */

const c_CONSTANTS = require("../../js_constants.js");
const c_dbProxyClient = require("../server_to_server/js_db_proxy_client.js");


/**
 * Handles CONST_TYPE_AndruavSystem_QueryServer messages from units.
 *
 * Current subcommands:
 *   CONST_TYPE_AndruavSystem_QueryServer_SubCmd_Is_Storage_Server_Connected (1)
 *     - Replies with { sc: 1, v: <bool>, st: <connectionStateString> }
 */
function fn_handleQueryServer(v_jmsg, p_ws) {
    let mms = v_jmsg.ms;
    if (typeof mms === 'string' || mms instanceof String) {
        try { mms = JSON.parse(mms); } catch (e) { return; }
    }
    if (mms == null) return;

    const c_subCmd = mms.sc;
    let v_replyMs = null;

    switch (c_subCmd) {
        case c_CONSTANTS.CONST_TYPE_AndruavSystem_QueryServer_SubCmd_Is_Storage_Server_Connected:
            v_replyMs = {
                sc: c_subCmd,
                v: c_dbProxyClient.fn_isConnected(),
                st: c_dbProxyClient.fn_getConnectionState() // CONST_CONNECTION_STATE
            };
            break;

        default:
            v_replyMs = {
                sc: c_subCmd,
                v: false,
                st: 'unknown_subcommand'
            };
            break;
    }

    v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_SYSTEM;
    v_jmsg.tg = p_ws.name;
    v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
    v_jmsg.mt = c_CONSTANTS.CONST_TYPE_AndruavSystem_StateServer;
    v_jmsg.ms = v_replyMs;
    p_ws.send(JSON.stringify(v_jmsg));
}


module.exports = {
    fn_handleQueryServer
};
