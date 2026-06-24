"use strict";

/**
 * Relay module.
 * Handles server-to-server mesh forwarding and loop-prevention metadata injection.
 * Extracted from js_andruav_chat_server.js (behavior-preserving).
 * See wiki/MessagePropagation.md for architecture details.
 */


// Server ID for origin tracking in mesh relay (prevents message loops)
// Uses configured server_id from global config
function getServerOriginID() {
    return global.m_serverconfig?.m_configuration?.server_id || 'unknown';
}


function forwardMessage(message, p_isBinary, p_ws)
{
    if (p_ws == null || p_ws.m_loginRequest == null) {
        return;
    }

    const c_group = p_ws.m__group;

    // IMPORTANT: Inject relay metadata in a SINGLE parse pass to avoid re-parsing downstream:
    //  - _path : append this server to the loop-prevention trail (relay-forwarded messages only).
    //  - _gid / _aid : the LOCAL sender's group/account, used for scoped delivery on the receiver.
    // NOTE: This parsing is intentionally separate from fn_parseMessage() parsing.
    // DO NOT move this injection to fn_parseMessage() because:
    // 1. These fields should ONLY be added to relay-forwarded messages, NOT local client messages
    // 2. Binary message format is: JSON + char(0) + binary_payload
    //    The binary payload must be preserved when reconstructing the message.
    // See wiki/MessagePropagation.md for architecture details.
    let forwardMsg = message;
    try {
        const v_originID = getServerOriginID();
        if (p_isBinary) {
            // Binary: parse JSON part only (before null terminator), preserve binary payload
            const nullIndex = message.indexOf(0);
            const jsonPart = nullIndex !== -1 ? message.subarray(0, nullIndex) : message;
            const v_jmsg = JSON.parse(jsonPart.toString('utf8'));

            if (!Array.isArray(v_jmsg._path)) v_jmsg._path = [];
            v_jmsg._path.push(v_originID);
            if (c_group != null) {
                v_jmsg._gid = c_group.m_ID;
                v_jmsg._aid = c_group.m_parentAccount.m_accountID;
            }
            const newJsonBuffer = Buffer.from(JSON.stringify(v_jmsg), 'utf8');
            // Reconstruct: new_json + original_null_and_binary
            forwardMsg = nullIndex !== -1
                ? Buffer.concat([newJsonBuffer, message.subarray(nullIndex)])
                : newJsonBuffer;
        } else {
            // Text: simple JSON parse/modify/stringify
            const v_jmsg = JSON.parse(message);
            if (!Array.isArray(v_jmsg._path)) v_jmsg._path = [];
            v_jmsg._path.push(v_originID);
            if (c_group != null) {
                v_jmsg._gid = c_group.m_ID;
                v_jmsg._aid = c_group.m_parentAccount.m_accountID;
            }
            forwardMsg = JSON.stringify(v_jmsg);
        }
    } catch (e) {
        // If parsing fails, forward original message
        console.log("Failed to parse message for relay metadata:", e.message);
    }

    if (global.m_serverconfig.m_configuration.enable_super_server === true)
    {
        global.m_andruav_channel_parent_server.getInstance().forwardMessage(forwardMsg, p_isBinary);
    }

    if (global.m_serverconfig.m_configuration.enable_persistant_relay === true)
    {
        global.m_andruav_channel_child_socket.getInstance().forwardMessage(forwardMsg, p_isBinary);
    }
}


/**
 * Forwards external relay messages further in the relay tree.
 * Enables tree-like propagation: Child A -> Parent -> Child B/C + Grandparent
 * @param {*} message
 * @param {*} p_isBinary
 * @param {*} p_source_ws Optional WebSocket of the source child (for parent mode exclusion)
 */
function forwardExternalMessage(message, p_isBinary, p_source_ws = null) {
    // Append this server to the _path trail (for loop prevention)
    let forwardMsg = message;
    try {
        const v_originID = getServerOriginID();
        if (p_isBinary) {
            const nullIndex = message.indexOf(0);
            const jsonPart = nullIndex !== -1 ? message.subarray(0, nullIndex) : message;
            const v_jmsg = JSON.parse(jsonPart.toString('utf8'));

            if (!Array.isArray(v_jmsg._path)) v_jmsg._path = [];
            v_jmsg._path.push(v_originID);
            const newJsonBuffer = Buffer.from(JSON.stringify(v_jmsg), 'utf8');
            forwardMsg = nullIndex !== -1
                ? Buffer.concat([newJsonBuffer, message.subarray(nullIndex)])
                : newJsonBuffer;
        } else {
            const v_jmsg = JSON.parse(message);
            if (!Array.isArray(v_jmsg._path)) v_jmsg._path = [];
            v_jmsg._path.push(v_originID);
            forwardMsg = JSON.stringify(v_jmsg);
        }
    } catch (e) {
        console.log("Failed to parse external message for path tracking:", e.message);
    }

    // Forward to parent (grandparent) if in child mode
    if (global.m_serverconfig.m_configuration.enable_persistant_relay === true) {
        global.m_andruav_channel_child_socket.getInstance().forwardMessage(forwardMsg, p_isBinary);
    }

    // Forward to children (excluding source child) if in parent mode
    if (global.m_serverconfig.m_configuration.enable_super_server === true) {
        global.m_andruav_channel_parent_server.getInstance().forwardMessage(forwardMsg, p_isBinary, p_source_ws);
    }
}


module.exports = {
    getServerOriginID,
    forwardMessage,
    forwardExternalMessage
};
