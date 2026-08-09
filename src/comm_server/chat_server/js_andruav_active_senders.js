"use strict";

const m_activeSenderIDsList = {};



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


module.exports = {
    addActiveSenderIDList,
    deleteActiveSenderIDList,
    getActiveSender
};