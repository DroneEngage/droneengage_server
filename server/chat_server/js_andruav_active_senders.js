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



module.exports = {
    addActiveSenderIDList,
    deleteActiveSenderIDList
};