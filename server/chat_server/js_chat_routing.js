"use strict";

/**
 * Routing module.
 * Parses inbound messages (local and relayed) and dispatches them to the
 * correct delivery path (group broadcast, targeted, or system command).
 * Extracted from js_andruav_chat_server.js (behavior-preserving).
 */

const c_dumpError = require("../../dumperror.js");
const c_CONSTANTS = require("../../js_constants.js");

const c_ChatAccountRooms = require("./js_andruav_chat_account_rooms.js");
const c_PluginManager = require('./pluginManager.js');
const c_relay = require("./js_chat_relay.js");
const c_system = require("./js_chat_system_commands.js");

const forwardMessage = c_relay.forwardMessage;
const forwardExternalMessage = c_relay.forwardExternalMessage;
const getServerOriginID = c_relay.getServerOriginID;


function send_message_toMyGroup(message, isbinary, ws) {
    // This condition hides an error when ws is closed silently.
    // a new instance of ws is created with group = 0 and the  old ws is lost

    try {
        if (ws.m__group != null) {
            ws.m__group.fn_broadcast(message, isbinary, ws.m_loginRequest.m_senderID);
        }
    }
    catch (e) {
        console.log('send_message_toMyGroup :ws:' + ws.name + 'Error:' + e);
        c_dumpError.fn_dumperror(e);
    }
}


function send_message_toMyGroup_Agent(message, isbinary, ws) {
    // This condition hides an error when ws is closed silently.
    // a new instance of ws is created with group = 0 and the  old ws is lost

    try {
        if (ws.m__group != null) {
            ws.m__group.fn_broadcastToDrone(message, isbinary, ws.m_loginRequest.m_senderID);
        }
    }
    catch (e) {
        console.log('send_message_toMyGroup :ws:' + ws.name + 'Error:' + e);
        c_dumpError.fn_dumperror(e);
    }
}


function send_message_toMyGroup_GCS(message, isbinary, ws) {
    // This condition hides an error when ws is closed silently.
    // a new instance of ws is created with group = 0 and the  old ws is lost

    try {
        if (ws.m__group != null) {
            ws.m__group.fn_broadcastToGCS(message, isbinary, ws.m_loginRequest.m_senderID);
        }
    }
    catch (e) {
        console.log('send_message_toMyGroup :ws:' + ws.name + 'Error:' + e);
        c_dumpError.fn_dumperror(e);
    }
}


function send_message_toTarget(message, isbinary, target, ws, onNotFound) {
    try {
        ws.m__group.fn_sendToIndividual(message, isbinary, target, onNotFound);
    }
    catch (err) {
        c_dumpError.fn_dumperror(err);
    }
}


/**
 * Delivers a relayed broadcast scoped to the originating account/group when the
 * sender's account/group ids (_aid/_gid) are present in the message.
 * Falls back to the legacy all-accounts broadcast when ids are missing or the
 * account/group does not exist locally (backward compatibility).
 */
function deliverExternalBroadcast(p_message, p_isBinary, senderID, v_jmsg, p_actorType, p_fallback) {
    if ((v_jmsg._aid !== undefined) && (v_jmsg._gid !== undefined)) {
        if (c_ChatAccountRooms.fn_sendToAccountGroup(p_message, p_isBinary, senderID, v_jmsg._aid, v_jmsg._gid, p_actorType) === true) {
            return;
        }
    }

    // Legacy fallback: ids absent or account/group not present locally.
    p_fallback(p_message, p_isBinary, senderID);
}


/**
 * Parses messages that are received by or from a super server.
 * @param {*} p_message
 * @param {*} p_isBinary
 * @param {*} p_source_ws Optional WebSocket of the source child that sent this message (for parent mode exclusion)
 * @returns
 */
function fn_parseExternalMessage(p_message, p_isBinary, p_source_ws = null) {
    let v_jmsg = null;
    const nullIndex = p_message.indexOf(0);
    let senderID = null;
    if (p_isBinary == true) {
        try {
            const jsonPart = nullIndex !== -1 ? p_message.subarray(0, nullIndex) : p_message;
            v_jmsg = JSON.parse(jsonPart.toString('utf8'));
            senderID = v_jmsg.sd;
        }
        catch {
            return;
        }
    }
    else {
        try {
            v_jmsg = JSON.parse(p_message);
            senderID = v_jmsg.sd;
        }
        catch {
            return;
        }
    }

    // Loop prevention: ignore messages this server has already relayed
    if (Array.isArray(v_jmsg._path) && v_jmsg._path.includes(getServerOriginID())) {
        return;
    }

    //p_message = null;
    switch (v_jmsg[c_CONSTANTS.CONST_WS_MSG_ROUTING]) {
        case c_CONSTANTS.CONST_WS_MSG_ROUTING_GROUP: // group
            // Scoped to the originating account/group (falls back to all on legacy messages)
            deliverExternalBroadcast(p_message, p_isBinary, senderID, v_jmsg, null, c_ChatAccountRooms.fn_sendToAll);
            // RELAY FORWARDING: Broadcast messages propagate in the relay tree
            forwardExternalMessage(p_message, p_isBinary, p_source_ws);
            break;

        case c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL: // individual
            try {
                // ROUTING LOGIC:
                // 1- Sender is Agent:
                //  a. Sender is an agent with null target means [broadcast] cannot broadcast to another agent.
                //  b. Sender is an agent with _GD_ target means [broadcast] to all including other drones.
                // 2- GCS Logic:
                //   a. GCS has no target then send to ALL AGENTS & GCS.
                //   b. GCS specifies AGENTS as target.
                // 3- one-to-one message as target not a reserved word.

                if ((v_jmsg.hasOwnProperty('tg') === true) && (v_jmsg['tg'].length > 0)) {
                    switch (v_jmsg['tg']) {
                        case c_CONSTANTS.CONST_WS_SENDER_ALL_GCS:
                            // Target is GCS regardless who the sender is means [broadcast] to all GCS.
                            deliverExternalBroadcast(p_message, p_isBinary, senderID, v_jmsg, 'g', c_ChatAccountRooms.fn_sendToAllGCS);
                            // RELAY FORWARDING: Broadcast messages propagate in the relay tree
                            forwardExternalMessage(p_message, p_isBinary, p_source_ws);
                            break;
                        case c_CONSTANTS.CONST_WS_SENDER_ALL:
                            // Target is _GD_  means [broadcast] to all units including GCS & drones.
                            deliverExternalBroadcast(p_message, p_isBinary, senderID, v_jmsg, null, c_ChatAccountRooms.fn_sendToAll);
                            // RELAY FORWARDING: Broadcast messages propagate in the relay tree
                            forwardExternalMessage(p_message, p_isBinary, p_source_ws);
                            break;
                        case c_CONSTANTS.CONST_WS_SENDER_ALL_AGENTS:
                            // Target is _AGN_  means [broadcast] to all drones.
                            deliverExternalBroadcast(p_message, p_isBinary, senderID, v_jmsg, 'd', c_ChatAccountRooms.fn_sendToAllAgent);
                            // RELAY FORWARDING: Broadcast messages propagate in the relay tree
                            forwardExternalMessage(p_message, p_isBinary, p_source_ws);
                            break;
                        default:
                            // ONE to ONE Message - do NOT propagate in relay tree
                            c_ChatAccountRooms.fn_sendTIndividualId(p_message, p_isBinary, v_jmsg.tg, senderID, v_jmsg._gid, function onNotFound() {
                                // One-to-one messages are not propagated further
                            });
                            break;
                    }
                    break;
                }
                else {
                    // No target specified in external message - broadcast to the originating account/group.
                    // External messages don't have socket context, so we can't determine actor type.
                    // Safe default: send to all units in scope and let them filter.
                    deliverExternalBroadcast(p_message, p_isBinary, senderID, v_jmsg, null, c_ChatAccountRooms.fn_sendToAll);
                    // RELAY FORWARDING: Broadcast messages propagate in the relay tree
                    forwardExternalMessage(p_message, p_isBinary, p_source_ws);
                }
            }
            catch (e) {
                p_message = Buffer.alloc(0);
                c_dumpError.fn_dumperror(e);
                if (global.m_logger) global.m_logger.Error('Bad Parsing Message:CONST_WS_MSG_ROUTING_INDIVIDUAL', 'fn_parseExternalMessage', null, e);
            }
            break;
    }
}


/**
 * Handle messages received from onMessage event
 * @param {*} p_ws
 * @param {*} p_message
 * @param {*} p_isBinary
 */
function fn_parseMessage(p_ws, p_message, p_isBinary) {

    let v_jmsg = null;
    let p_message_w_permission = null;
    const nullIndex = p_message.indexOf(0);
    if (p_isBinary == true) {
        try {
            p_ws.m_status.m_BTX += p_message.length;

            // Parse JSON header: use subarray (zero-copy) and parse once
            const jsonPart = nullIndex !== -1
                ? p_message.subarray(0, nullIndex)
                : p_message;
            v_jmsg = JSON.parse(jsonPart.toString('utf8'));

            // INJECT permission with each gcs message. That is the only way to make sure that gcs can you fake it.
            // reconstruct the binary packet
            v_jmsg.p = p_ws.m_loginRequest.m_prm;
            const v_jmsg_str = JSON.stringify(v_jmsg);
            const v_jmsgBuffer = Buffer.from(v_jmsg_str, 'utf8');
            p_message_w_permission = nullIndex !== -1
                ? Buffer.concat([v_jmsgBuffer, p_message.subarray(nullIndex)])
                : v_jmsgBuffer;

        }
        catch {
            return;
        }
    }
    else {
        if (p_ws.m_status == null) {
            // unauthorized login.
            return;
        }
        p_ws.m_status.m_TTX += p_message.length;

        // This condition hides an error when ws is closed silently.
        // a new instance of ws is created with group = 0 and the  old ws is lost
        // do not log statistics for now.
        try {
            v_jmsg = JSON.parse(p_message);

            // INJECT permission with each gcs message. That is the only way to make sure that gcs can you fake it.
            v_jmsg.p = p_ws.m_loginRequest.m_prm;
            p_message_w_permission = JSON.stringify(v_jmsg);
        }
        catch {
            return;
        }
    }

    c_PluginManager.fn_processPlugins(p_ws, p_message, v_jmsg, nullIndex,
        function (p_param1, p_param2) {

            return;
        });

    p_message = null;

    switch (v_jmsg[c_CONSTANTS.CONST_WS_MSG_ROUTING]) {
        case c_CONSTANTS.CONST_WS_MSG_ROUTING_GROUP: // group
            try {
                // send to group
                if (p_ws.m_loginRequest.m_actorType == c_CONSTANTS.CONST_ACTOR_TYPE_GCS) {
                    send_message_toMyGroup(p_message_w_permission, p_isBinary, p_ws);
                }
                else {
                    send_message_toMyGroup_GCS(p_message_w_permission, p_isBinary, p_ws);
                }

                forwardMessage(p_message_w_permission, p_isBinary, p_ws);

            }
            catch (e) {
                p_message_w_permission = Buffer.alloc(0);
                c_dumpError.fn_dumperror(e);
                if (global.m_logger) global.m_logger.Error('Bad Parsing Message:CONST_WS_MSG_ROUTING_INDIVIDUAL', 'fn_parseMessage', null, e);
            }
            break;
        case c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL: // individual
            try {
                // ROUTING LOGIC:
                // 1- Sender is Agent:
                //  a. Sender is an agent with null target means [broadcast] cannot broadcast to another agent.
                //  b. Sender is an agent with _GD_ target means [broadcast] to all including other drones.
                // 2- GCS Logic:
                //   a. GCS has no target then send to ALL AGENTS & GCS.
                //   b. GCS specifies AGENTS as target.
                // 3- one-to-one message as target not a reserved word.

                if ((v_jmsg.hasOwnProperty('tg') === true) && (v_jmsg['tg'].length > 0)) {
                    switch (v_jmsg['tg']) {
                        case c_CONSTANTS.CONST_WS_SENDER_ALL_GCS:
                            // Target is GCS regardless who the sender is means [broadcast] to all GCS.
                            if (p_ws != null) {
                                send_message_toMyGroup_GCS(p_message_w_permission, p_isBinary, p_ws);
                            }

                            forwardMessage(p_message_w_permission, p_isBinary, p_ws);

                            break;
                        case c_CONSTANTS.CONST_WS_SENDER_ALL:
                            // Target is _GD_  means [broadcast] to all units including GCS & drones.
                            if (p_ws != null) {
                                send_message_toMyGroup(p_message_w_permission, p_isBinary, p_ws);
                            }

                            forwardMessage(p_message_w_permission, p_isBinary, p_ws);


                            break;
                        case c_CONSTANTS.CONST_WS_SENDER_ALL_AGENTS:
                            // Target is _AGN_  means [broadcast] to all drones.
                            if (p_ws != null) {
                                send_message_toMyGroup_Agent(p_message_w_permission, p_isBinary, p_ws);
                            }
                            forwardMessage(p_message_w_permission, p_isBinary, p_ws);

                            break;
                        default:
                            // ONE to ONE Message
                            if (p_ws != null) {
                                send_message_toTarget(p_message_w_permission, p_isBinary, v_jmsg.tg, p_ws, function onNotFound() {
                                    forwardMessage(p_message_w_permission, p_isBinary, p_ws);
                                });
                            }
                            else {
                                forwardMessage(p_message_w_permission, p_isBinary, p_ws);
                            }
                            break;
                    }
                    break;
                }
                else
                    // 2- Agent  & (v_jmsg['tg'] == null)
                    if (p_ws.m_loginRequest.m_actorType === c_CONSTANTS.CONST_ACTOR_TYPE_DRONE) {
                        // Default broadcast for agents is to GCS only
                        send_message_toMyGroup_GCS(p_message_w_permission, p_isBinary, p_ws);
                        forwardMessage(p_message_w_permission, p_isBinary, p_ws);
                        break;
                    }
                    else
                        // 3- GCS Logic  & (v_jmsg['tg'] == null)
                        if (p_ws.m_loginRequest.m_actorType === c_CONSTANTS.CONST_ACTOR_TYPE_GCS) {
                            // Default broadcast for GCS is to all units.
                            send_message_toMyGroup(p_message_w_permission, p_isBinary, p_ws);
                            forwardMessage(p_message_w_permission, p_isBinary, p_ws);
                            break;
                        }
            }
            catch (e) {
                p_message_w_permission = Buffer.alloc(0);
                c_dumpError.fn_dumperror(e);
                if (global.m_logger) global.m_logger.Error('Bad Parsing Message:CONST_WS_MSG_ROUTING_INDIVIDUAL', 'fn_parseMessage', null, e);
            }
            break;

        case c_CONSTANTS.CONST_WS_MSG_ROUTING_SYSTEM:
            {
                c_system.onSystemMessage(v_jmsg, p_ws);
            }
            break;

    }
}


module.exports = {
    fn_parseMessage,
    fn_parseExternalMessage,
    send_message_toMyGroup,
    send_message_toMyGroup_Agent,
    send_message_toMyGroup_GCS,
    send_message_toTarget,
    deliverExternalBroadcast
};
