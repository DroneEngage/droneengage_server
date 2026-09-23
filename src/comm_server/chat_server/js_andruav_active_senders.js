"use strict";

const m_activeSenderIDsList = {};

// In-memory admin blocklist. Blocked senders keep their sockets open but their
// inbound messages are silently dropped at the routing layer. Lost on restart.
const m_blockedSenders = new Set();



function addActiveSenderIDList(senderID, ws)
{
    m_activeSenderIDsList[senderID] = ws;
}


function deleteActiveSenderIDList(senderID)
{

    if (senderID == null) return;
    if (m_activeSenderIDsList[senderID] == null) return;
    delete m_activeSenderIDsList[senderID];
}

/**
 * O(1) lookup for a socket by senderID.
 * @param {string} senderID 
 * @returns {WebSocket|null}
 */
function getActiveSender(senderID)
{
    return m_activeSenderIDsList[senderID] || null;
}


/**
 * Blocks or resumes inbound traffic for a connected unit. A blocked unit keeps
 * its socket open but its messages are dropped before parsing — the client
 * cannot distinguish this from a dead link. The flag survives disconnects, so
 * a unit that reconnects under the same senderID stays blocked.
 * @param {string} senderID unit name (same key used by m_activeSenderIDsList)
 * @param {boolean} blocked
 * @returns {boolean} false when the name is neither connected nor already blocked
 */
function fn_setSenderBlocked(senderID, blocked)
{
    if (senderID == null) return false;
    if ((blocked === true)
        && (m_activeSenderIDsList.hasOwnProperty(senderID) === false)
        && (m_blockedSenders.has(senderID) === false)) {
        return false;
    }

    if (blocked === true) {
        m_blockedSenders.add(senderID);
    } else {
        m_blockedSenders.delete(senderID);
    }

    return true;
}

function fn_isSenderBlocked(senderID)
{
    return (senderID != null) && m_blockedSenders.has(senderID);
}

function fn_getBlockedSenders()
{
    return Array.from(m_blockedSenders);
}


module.exports = {
    addActiveSenderIDList,
    deleteActiveSenderIDList,
    getActiveSender,
    fn_setSenderBlocked,
    fn_isSenderBlocked,
    fn_getBlockedSenders
};