"use strict";

/**
 * This module represents CHAT ROOMS for a given ACCOUNT-ID
 * each Room is related to a Group.
 */


const { v4: uuidv4 } = require('uuid');
const _dumpError = require("../../dumperror.js");


const c_accounts = {};

function fn_getUnitKeys() {
    return Object.keys(c_accounts);
}

function fn_getUnitValues() {
    return Object.values(c_accounts);
}

function fn_getUnitCount() {
    return Object.keys(c_accounts).length;
}


/**
 * Sends a message to all Ground Control Stations (GCS) in ALL ACCounts.
 * @param {*} message 
 * @param {*} isBinary 
 * @param {*} senderId 
 */
function fn_sendToAllGCS(message, isBinary, senderId) {
    Object.values(c_accounts).forEach(account => {
        Object.values(account.m_groups).forEach(group => {
            group.fn_broadcastToGCS(message, isBinary, senderId);
        });
    });
}


/**
 * Sends a message to all Agents in ALL ACCounts.
 * @param {*} message 
 * @param {*} isBinary 
 * @param {*} senderId 
 */
function fn_sendToAllAgent(message, isBinary, senderId) {
    Object.values(c_accounts).forEach(account => {
        Object.values(account.m_groups).forEach(group => {
            group.fn_broadcastToDrone(message, isBinary, senderId);
        });
    });
}


function fn_sendToAll (message, isBinary, senderId) {
    Object.values(c_accounts).forEach(account => {
        Object.values(account.m_groups).forEach(group => {
            group.fn_broadcast(message, isBinary, senderId);
        });
    });
}

function fn_sendTIndividualId (message, isBinary, senderId, cb) {
    Object.values(c_accounts).forEach(account => {
        Object.values(account.m_groups).forEach(group => {
            group.fn_sendToIndividual(message, isBinary, senderId), cb;
        });
    });
}


function fn_add_member_to_AccountGroup(p_ws) {
    const c_loginRequest = p_ws.m_loginRequest;
    const v_id = c_loginRequest.m_accountID;
    let v_acc;

    if (c_accounts.hasOwnProperty(v_id)) {
        // account already exists
        console.log(`account ${v_id} already exists`);
        v_acc = c_accounts[v_id];
    } else {
        // account does not exist
        console.log(`account ${v_id} created`);
        v_acc = new Account(v_id);
        c_accounts[v_id] = v_acc;
    }

    return v_acc.fn_add_member_to_AccountGroup(c_loginRequest.m_senderID, c_loginRequest.m_groupID, p_ws);
}



function fn_del_member_fromGroup(p_websocket) {
    if (!p_websocket.hasOwnProperty("m__group")) {
        // socket is not linked to any group
        console.log("del_member_fromGroup: Nothing to Delete");
        return;
    }

    if (p_websocket.name == null) {
        delete p_websocket.m__group;
        console.log('No unit name to remove');
        return;
    }

    return p_websocket.m__group.fn_deleteMemberByName(p_websocket.name);

}



function fn_del_member_fromAccountByName(p_loginRequest, terminateSocket) {

    let acc = c_accounts[p_loginRequest.m_accountID];

    if (acc == null) {
        console.log("info: no account associated with socket .... This could be a brand new socket.");

        return;
    }

    acc.fn_del_member_fromAccountByName(p_loginRequest.m_senderID, terminateSocket);

}



function fn_forEach(callback) {
    Object.values(c_accounts).forEach(callback);
}

///////////////////////////////////////////////////  Account

function Account(p_accountID) {
    this.m_accountID = p_accountID;
    this.m_groups = {};
    Object.seal(this);
}

/***
 * This is a Facade Layer that addes a member to a group.
 * Account/Group are created if any not existed.
 * Returns: true/false
 ***/
Account.prototype.fn_add_member_to_AccountGroup = function (p_unitname, p_groupname, p_ws) {
    let gr;

    if (this.m_groups.hasOwnProperty(p_groupname)) {
        // group already exists
        console.log(`group ${p_groupname} already exists`);
        gr = this.m_groups[p_groupname];
    } else {
        // group does not exist
        console.log(`group ${p_groupname} created`);
        gr = this.m_groups[p_groupname] = new Group(this, p_groupname);
    }

    return gr.fn_addMember(p_unitname, p_ws);
}

/***
 * Searchs in all groups and remove all sockets with that name.
 ***/
Account.prototype.fn_del_member_fromAccountByName = function (p_unitname, terminateSocket) {
    this.forEach(group => group.fn_deleteMemberByName(p_unitname, terminateSocket));
}

Account.prototype.forEach = function (callback) {
    Object.values(this.m_groups).forEach(callback);
}

///////////////////////////////////////// GROUP

/***
 * Group Object
 ***/
function Group(m_accountObj, p_ID) {
    this.m_ID = p_ID;
    this.m_parentAccount = m_accountObj; 
    this.m_units = {};

    this.uid = uuidv4();
    this.m_creationDate = Date.now();
    this.m_TTX = 0;
    this.m_BTX = 0;
    this.m_lastAccessTime = 0;

    Object.seal(this);
}



/***
 * This is a Facade Layer that adds a member to a group.
 * Account/Group are created if any not existed.
 * Returns: true/false
 ***/
Group.prototype.fn_addMember = function (p_unitname, p_ws) {

    if (this.m_units.hasOwnProperty(p_unitname)) {
        // this should never happen.
        console.log(`addMember [${p_unitname}] already EXISTS. Don't Override`);
        return false;
    } else {
        console.log(`addMember [${p_unitname}] Added`);
        this.m_units[p_unitname] = p_ws;
        p_ws.name = p_unitname;
        p_ws.m__group = this;
    }

    return true;
}

/***
 * Deletes sockets with a given name. even if socket is not the same instance.
 ***/
Group.prototype.fn_deleteMemberByName = function (p_unitname, terminateSocket) {
    try {
        if (this.m_units.hasOwnProperty(p_unitname)) {
            console.log(`deleteMemberByName: deleteMember ${p_unitname}`);
            // this is a socket under the same name 
            const oldSocket = this.m_units[p_unitname];
            delete this.m_units[p_unitname];
            delete oldSocket.m__group;
            if (terminateSocket) {
                console.log(`deleteMemberByName: terminateSocket ${p_unitname}`);
                oldSocket.m__terminated = true;
                oldSocket.terminate();
            }
        }
    } catch (e) {
        _dumpError.fn_dumperror(e);
    }
}

Group.prototype.forEach = function (callback) {
    Object.values(this.m_units).forEach(callback);
}

Group.prototype.fn_sendToIndividual = function (message, isbinary, target, onNotFound) {
    try {
        const socket = this.m_units[target];
        if (socket != null) {
            socket.send(message, { binary: isbinary });
        } else {
            onNotFound && onNotFound(target);
        }
    } catch (e) {
        console.log(`broadcast :ws:${socket.Name} Orphan socket Error: ${e}`);
        _dumpError.fn_dumperror(e);
        this.fn_handleOrphanSocket(socket);
    }
}

Group.prototype.fn_broadcastToGCS = function (p_message, isbinary, senderID) {
    this.fn_broadcast(p_message, isbinary, senderID, 'g');
}

Group.prototype.fn_broadcastToDrone = function (p_message, p_isbinary, senderID) {
    this.fn_broadcast(p_message, p_isbinary, senderID, 'd');
}

Group.prototype.fn_broadcast = function (p_message, p_isbinary, senderID, p_actorType = null) {
    // I am using c_ws.m_loginRequest.m_senderID instead of p_message.sd 
    // to prevent sender socket from deceiving the server as m_senderID is recieved from AUTH Server.
    
    for (const [_, socket] of Object.entries(this.m_units)) {
        try {
            if (socket.m_loginRequest.m_senderID !== senderID &&
                (p_actorType === null || socket.m_loginRequest.m_actorType === p_actorType)) {
                socket.send(p_message, { binary: p_isbinary });
            }
        } catch (e) {
            console.log(`broadcast :ws:${socket.Name} Orphan socket Error: ${e}`);
            _dumpError.fn_dumperror(e);
            this.fn_handleOrphanSocket(socket);
        }
    }
}

Group.prototype.fn_handleOrphanSocket = function (socket) {
    if (socket != null) {
        try {
            socket.m__group.fn_deleteMemberByName(socket.name);
            console.log(`unit ${socket.Name} found dead`);
            delete socket.name;
            socket.terminate();
        } catch (e) {
            _dumpError.fn_dumperror(e);
        }
    }
}

module.exports = {
    fn_getUnitKeys,
    fn_getUnitValues,
    fn_getUnitCount,
    fn_add_member_to_AccountGroup,
    fn_del_member_fromGroup,
    fn_del_member_fromAccountByName,
    fn_forEach,
    fn_sendToAllGCS,
    fn_sendToAllAgent,
    fn_sendToAll,
    fn_sendTIndividualId
};