"use strict";

/**
 * System-command module.
 * Handles CONST_WS_MSG_ROUTING_SYSTEM messages (UDP proxy, ping, ver,
 * logout, and task commands). Extracted from js_andruav_chat_server.js
 * (behavior-preserving). Task commands are delegated to js_chat_tasks.js.
 */

const c_dumpError = require("../../dumperror.js");
const c_CONSTANTS = require("../../js_constants.js");
const udp = require('../js_udp_proxy.js');

const c_ChatAccountRooms = require("./js_andruav_chat_account_rooms.js");
const c_tasks = require("./js_chat_tasks.js");


function onSystemMessage(v_jmsg, p_ws) {

    try {
        switch (v_jmsg[c_CONSTANTS.CONST_WS_MESSAGE_ID]) {
            case c_CONSTANTS.CONST_TYPE_AndruavSystem_UdpProxy:
                if (p_ws.m_loginRequest.m_actorType !== c_CONSTANTS.CONST_ACTOR_TYPE_DRONE) {
                    // only vehicle can create udp proxy
                    return;
                }


                if (v_jmsg.ms.en === true) {
                    if (global.m_serverconfig.m_configuration.allow_udpproxy_fixed_port !== true) {   // unit cannot determine port numbers if allow_udpproxy_fixed_port != true.
                        v_jmsg.ms.socket1.port = 0;
                        v_jmsg.ms.socket2.port = 0;
                    }

                    udp.getUDPSocket(p_ws.name, v_jmsg.ms.socket1, v_jmsg.ms.socket2, function (ms) {
                        if (ms.en === false) {
                            v_jmsg.ms.socket1.port = 0;
                            v_jmsg.ms.socket2.port = 0;

                            udp.closeUDPSocket(p_ws.name, function (ms) {
                                v_jmsg.ms = ms;
                                v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
                                v_jmsg.tg = p_ws.name; // sender = target
                                v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                                p_ws.send(JSON.stringify(v_jmsg));
                            });


                        } else {
                            v_jmsg.ms = ms;
                            v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
                            v_jmsg.tg = p_ws.name; // sender = target
                            v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                            p_ws.send(JSON.stringify(v_jmsg));
                        }
                    });
                } else
                    if (v_jmsg.ms.en === false) {
                        udp.closeUDPSocket(p_ws.name, function (ms) {
                            v_jmsg.ms = ms;
                            v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
                            v_jmsg.tg = p_ws.name; // sender = target
                            v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                            p_ws.send(JSON.stringify(v_jmsg));
                        });
                    }
                break;

            case c_CONSTANTS.CONST_TYPE_AndruavSystem_Ping:
                v_jmsg.ms.s = 'OK:pong';
                p_ws.send(JSON.stringify(v_jmsg));
                break;

            case 'ver':
                v_jmsg.ms = 'ver:' + version;
                p_ws.send(JSON.stringify(v_jmsg));
                c_dumpError.fn_dumpdebug('ver cmd:' + version);
                break;

            case c_CONSTANTS.CONST_TYPE_AndruavSystem_LogoutCommServer:
                c_ChatAccountRooms.fn_del_member_fromGroup(p_ws);
                v_jmsg.ms = { s: 'OK:dell' };
                p_ws.send(JSON.stringify(v_jmsg));
                break;

            case c_CONSTANTS.CONST_TYPE_AndruavSystem_LoadTasks:
                c_tasks.fn_handleLoadTasks(v_jmsg, p_ws);
                break;

            case c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveTasks:
                c_tasks.fn_handleSaveTasks(v_jmsg, p_ws);
                break;

            case c_CONSTANTS.CONST_TYPE_AndruavSystem_DeleteTasks:
                c_tasks.fn_handleDeleteTasks(v_jmsg, p_ws);
                break;

            case c_CONSTANTS.CONST_TYPE_AndruavSystem_DisableTasks:
                c_tasks.fn_handleDisableTasks(v_jmsg, p_ws);
                break;

        }
    }
    catch (e) {
        c_dumpError.fn_dumperror(e);
        if (global.m_logger) global.m_logger.Error('Bad Parsing Message:CONST_WS_MSG_ROUTING_SYSTEM', 'fn_parseMessage', null, e);
    }
}


module.exports = {
    onSystemMessage
};
