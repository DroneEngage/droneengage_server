"use strict";
const path = require('path');

/**
 * Routes communication between different parties.
 * This is Party-to-Party Module.
 */
const c_dumpError = require("../../dumperror.js");

const c_CONSTANTS = require("../../js_constants.js");
const c_ChatAccountRooms = require("./js_andruav_chat_account_rooms.js");
const c_CommServerManagerClient = require("../js_comm_server_manager_client.js");
const c_andruav_comm_server = require("../js_andruav_comm_server.js");

const c_andruav_active_senders = require("./js_andruav_active_senders.js");

const c_PluginManager = require('./pluginManager.js');




let v_andruavTasks;


function getHeaderParams(url) {
    let regex = /[?&]([^=#]+)=([^&#]*)/g,
        params = {},
        match;


    while (match = regex.exec(url)) {
        params[match[1]] = match[2];
    }

    return params;
}


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


    function forwardMessage(message, p_isBinary, p_ws)
    {
        const attached_data = {
            'g': p_ws.m__group,
            's': p_ws.m_loginRequest.m_senderID,
            'b': p_isBinary
        };

        if (global.m_serverconfig.m_configuration.enable_super_server === true)
        {
            global.m_andruav_channel_parent_server.getInstance().forwardMessage(message, p_isBinary, attached_data);
        }
                    
        if (global.m_serverconfig.m_configuration.enable_persistant_relay === true)
        {
            global.m_andruav_channel_child_socket.getInstance().forwardMessage(message, p_isBinary, attached_data);
        }
    }


    function _acceptConnection(c_onb, p_ws){
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
    function acceptLocalConnection(c_params, p_ws)
    {
        const sender_id = c_params[c_CONSTANTS.CONST_CS_SENDER_ID.toString()];
        const c_onb = {};
        c_onb.m_senderID = sender_id;
        c_onb.m_accountID = global.m_serverconfig.m_configuration.local_server_account_id==null?'1':global.m_serverconfig.m_configuration.local_server_account_id;
        c_onb.m_groupID = '1';
        c_onb.m_requestID = sender_id;  // !Change it to a random number
        c_onb.m_actorType = c_params.at==null?'a':c_params.at;  // use suggested actor type.
        c_onb.m_prm = 0xffffffff;
        c_onb.m_creationDate = Date.now();


        // Seal the object to prevent adding or removing properties, but allow modifying existing properties
        Object.seal(c_onb);
        
        _acceptConnection (c_onb, p_ws);

        
    }

    /**
     * Accepts a connection from AuthServer after validating the key.
     * @param {*} v_loginTempKey 
     * @param {*} p_ws 
     */
    function acceptConnection(v_loginTempKey, c_params, p_ws)
    {
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
                
                c_CommServerManagerClient.fn_onMessageOpened(); // give feedback to AUTH server 
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
     * Parses messages that are received by or from a super server.
     * @param {*} p_message 
     * @param {*} p_isBinary 
     * @returns 
     */
    function fn_parseExternalMessage(p_message, p_isBinary) {
        let v_jmsg = null;
        const nullIndex = p_message.indexOf(0);
        let senderID = null;
        if (p_isBinary == true) {
            try {
                
                if (nullIndex !== -1) {
                    const c_buff = p_message.slice(0, nullIndex);
                    v_jmsg = JSON.parse(c_buff.toString('utf8'));
                }

                if (v_jmsg == null) {
                    // bug fix: sometimes text message is sent as binary although it has no binary extension.
                    v_jmsg = JSON.parse(p_message);
                }
                senderID = v_jmsg.sd;
            }
            catch {
                return;
            }
        }
        else {
            
            // This condition hides an error when ws is closed silently.
            // a new instance of ws is created with group = 0 and the  old ws is lost
            // do not log statistics for now.
            try {
                v_jmsg = JSON.parse(p_message);
                senderID = v_jmsg.sd;
                //p_message_w_permission = p_message
            }
            catch {
                return;
            }
        }

        //p_message = null;
        switch (v_jmsg[c_CONSTANTS.CONST_WS_MSG_ROUTING]) {
            case c_CONSTANTS.CONST_WS_MSG_ROUTING_GROUP: // group
                c_ChatAccountRooms.fn_sendToAllGCS(p_message, p_isBinary, senderID);
                c_ChatAccountRooms.fn_sendToAllAgent(p_message, p_isBinary, senderID);
                
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
                                c_ChatAccountRooms.fn_sendToAllGCS(p_message, p_isBinary, senderID);
                                break;
                            case c_CONSTANTS.CONST_WS_SENDER_ALL:
                                // Target is _GD_  means [broadcast] to all units including GCS & drones.
                                c_ChatAccountRooms.fn_sendToAll(p_message, p_isBinary, senderID);
                                break;
                            case c_CONSTANTS.CONST_WS_SENDER_ALL_AGENTS:
                                // Target is _AGN_  means [broadcast] to all drones.
                                c_ChatAccountRooms.fn_sendToAllAgent(p_message, p_isBinary, senderID);
                                break;
                            default:
                                // ONE to ONE Message
                                c_ChatAccountRooms.fn_sendTIndividualId(p_message, p_isBinary, v_jmsg.tg, function onNotFound()
                                {
                                    // TODO: DONT PROPAGATE MORE
                                });
                                break;
                        }
                        break;
                    }
                    else
                        // 2- Agent  & (v_jmsg['tg'] == null)
                        if (p_ws.m_loginRequest.m_actorType === c_CONSTANTS.CONST_ACTOR_TYPE_DRONE) {
                            // Default broadcast for agents is to GCS only
                            c_ChatAccountRooms.fn_sendToAllGCS(p_message, p_isBinary, senderID);
                            break;
                        }
                        else
                            // 3- GCS Logic  & (v_jmsg['tg'] == null)
                            if (p_ws.m_loginRequest.m_actorType === c_CONSTANTS.CONST_ACTOR_TYPE_GCS) {
                                // Default broadcast for GCS is to all units.
                                c_ChatAccountRooms.fn_sendToAll(p_message, p_isBinary, senderID);
                                break;
                            }
                }
                catch (e) {
                    p_message = Buffer.alloc(0);
                    c_dumpError.fn_dumperror(e);
                    if (global.m_logger) global.m_logger.Error('Bad Parsing Message:CONST_WS_MSG_ROUTING_INDIVIDUAL', 'fn_parseMessage', null, e);
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

                if (nullIndex !== -1) {
                    const c_buff = p_message.slice(0, nullIndex);
                    v_jmsg = JSON.parse(c_buff.toString('utf8'));
                }

                if (v_jmsg == null) {
                    // bug fix: sometimes text message is sent as binary although it has no binary extension.
                    v_jmsg = JSON.parse(p_message);
                }

                // INJECT permission with each gcs message. That is the only way to make sure that gcs can you fake it.
                // reconstruct the binary packet
                v_jmsg.p = p_ws.m_loginRequest.m_prm;
                const v_jmsg_str = JSON.stringify(v_jmsg);
                const v_jmsgBuffer = Buffer.from(v_jmsg_str, 'utf8');
                p_message_w_permission = Buffer.concat([v_jmsgBuffer, p_message.slice(nullIndex)]);

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
        
        c_PluginManager.fn_processPlugins(p_ws, p_message, v_jmsg,nullIndex, 
            function (p_param1, p_param2) {
    
                        return;
            });
        
        p_message = null;

        switch (v_jmsg[c_CONSTANTS.CONST_WS_MSG_ROUTING]) {
            case c_CONSTANTS.CONST_WS_MSG_ROUTING_GROUP: // group
            try
            {
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
                                if (p_ws!=null){
                                    send_message_toMyGroup_GCS(p_message_w_permission, p_isBinary, p_ws);
                                }
                                
                                forwardMessage(p_message_w_permission, p_isBinary, p_ws);
                                
                                break;
                            case c_CONSTANTS.CONST_WS_SENDER_ALL:
                                // Target is _GD_  means [broadcast] to all units including GCS & drones.
                                if (p_ws!=null){
                                    send_message_toMyGroup(p_message_w_permission, p_isBinary, p_ws);
                                }
                                
                                forwardMessage(p_message_w_permission, p_isBinary, p_ws);
                                
    
                                break;
                            case c_CONSTANTS.CONST_WS_SENDER_ALL_AGENTS:
                                // Target is _AGN_  means [broadcast] to all drones.
                                if (p_ws!=null){
                                    send_message_toMyGroup_Agent(p_message_w_permission, p_isBinary, p_ws);
                                }
                                forwardMessage(p_message_w_permission, p_isBinary, p_ws);
                                
                                break;
                            default:
                                // ONE to ONE Message
                                if (p_ws!=null){
                                    send_message_toTarget(p_message_w_permission, p_isBinary, v_jmsg.tg, p_ws, function onNotFound()
                                {
                                    forwardMessage(p_message_w_permission, p_isBinary, p_ws);
                                });
                                }
                                else
                                {
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
                    onSystemMessage (v_jmsg, p_ws);
                }
                break;

        }
    }

    function onSystemMessage(v_jmsg, p_ws){
        const udp = require('../js_udp_proxy.js');
        
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
                                    c_WS.send(JSON.stringify(v_jmsg));
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
                    {
                        if (v_andruavTasks == null) break;
                        c_dumpError.fn_dumpdebug("load tasks command");
                        let v_params = {
                            resultfunc: function (res) {
                                if (res.length == 0) {
                                    // no data
                                    c_dumpError.fn_dumpdebug("Data Found:" + res.length);

                                }
                                else {
                                    // data found
                                    c_dumpError.fn_dumpdebug("Rows Length:" + res.length);
                                    v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL; 
                                    v_jmsg.tg = p_ws.name; // sender = target
                                    v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                                    //c_dumpError.fn_dumpdebug (JSON.stringify(res[0].task));
                                    //c_dumpError.fn_dumpdebug (JSON.stringify(res[1].task));
                                    for (let i = 0; i < res.length; i++) {
                                        v_jmsg.sid = res[i].SID; // add SID to p_message to point to a task.
                                        v_jmsg.ms = res[i].task;
                                        //c_dumpError.fn_dumpdebug (JSON.stringify(v_jmsg));
                                        v_jmsg.mt = res[i].messageType; // rechange command to the saved command.
                                        //c_dumpError.fn_dumpdebug('MESSAGE:'+JSON.stringify(v_jmsg));
                                        c_WS.send(JSON.stringify(v_jmsg));
                                    }
                                }

                            },
                            errfunc: function (err) {
                                c_dumpError.fn_dumperror(err);
                            }
                        };
                        let mms = v_jmsg.ms;
                        if (typeof mms === 'string' || mms instanceof String) {
                            // backword compatible
                            mms = JSON.parse(v_jmsg.ms); //Internal p_message JSON
                        }

                        if ((mms.ai == null) || (mms.ai.length == 0)) {
                            return; // error List all crosee accounts tasks ... ERROR
                        }
                        c_dumpError.fn_dumpdebug(mms);
                        c_dumpError.fn_dumpdebug(mms.messageType);
                        //{resultfunc,errfunc,largerThan_SID, party_sid,sender,receiver,messageType,task,isPermanent}
                        if (mms.hasOwnProperty("lts")) v_params.largerThan_SID = mms.lts;
                        if (mms.hasOwnProperty("ps")) v_params.party_sid = mms.ps;
                        if (mms.hasOwnProperty("ac")) v_params.accessCode = mms.ac;
                        if (mms.hasOwnProperty("ai")) v_params.accountID = mms.ai;
                        if (mms.hasOwnProperty("gn")) v_params.groupName = mms.gn;
                        if (mms.hasOwnProperty("s")) v_params.sender = mms.s;
                        if (mms.hasOwnProperty("r")) v_params.receiver = mms.r;
                        if (mms.hasOwnProperty("mt")) v_params.messageType = mms.mt;
                        if (mms.hasOwnProperty("ip")) v_params.isPermanent = mms.ip;
                        c_dumpError.fn_dumpdebug(v_params);
                        v_andruavTasks.fn_get_tasks(v_params);
                    }
                    break;
                case c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveTasks:
                    {
                        if (v_andruavTasks == null) break;
                        c_dumpError.fn_dumpdebug("save tasks command");
                        let v_params = {
                            resultfunc: function (res) {
                                if (res.length == 0) {
                                    // no data
                                    c_dumpError.fn_dumpdebug("Data Found:" + res.length);

                                }
                                else {
                                    // data found
                                    c_dumpError.fn_dumpdebug("Rows Length:" + res.length);
                                    v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL; // individual p_message
                                    v_jmsg.tg = p_ws.name; // sender = target
                                    v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                                    v_jmsg.mt = c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveTasks;
                                    v_jmsg.ms = "Done";
                                    p_ws.send(JSON.stringify(v_jmsg));
                                    //console.log (JSON.stringify(res[0].task));
                                    //console.log (JSON.stringify(res[1].task));

                                }

                            },
                            errfunc: function (err) {
                                c_dumpError.fn_dumperror(err);
                            }
                        };
                        let mms = v_jmsg.ms;
                        if (typeof mms === 'string' || mms instanceof String) {
                            // backword compatible
                            mms = JSON.parse(v_jmsg.ms); //Internal p_message JSON
                        }
                        c_dumpError.fn_dumpdebug(mms);
                        if ((mms.ai == null) || (mms.ai.length == 0)) {
                            return; // error List all crosee accounts tasks ... ERROR
                        }
                        // c_dumpError.fn_dumpdebug(mms.messageType);

                        if (mms.hasOwnProperty("ac")) v_params.accessCode = mms.ac;
                        if (mms.hasOwnProperty("ai")) v_params.accountID = mms.ai;
                        if (mms.hasOwnProperty("ps")) v_params.party_sid = mms.ps;
                        if (mms.hasOwnProperty("gn")) v_params.groupName = mms.gn;
                        if (mms.hasOwnProperty("s")) v_params.sender = mms.s;
                        if (mms.hasOwnProperty("r")) v_params.receiver = mms.r;
                        if (mms.hasOwnProperty("mt")) v_params.messageType = mms.mt;
                        if (mms.hasOwnProperty("ip")) v_params.isPermanent = mms.ip;
                        if (mms.hasOwnProperty("t")) v_params.task = JSON.stringify(mms.t);

                        c_dumpError.fn_dumpdebug(v_params);
                        v_andruavTasks.fn_add_task(v_params);
                    }
                    break;
                case c_CONSTANTS.CONST_TYPE_AndruavSystem_DeleteTasks:
                    {
                        if (v_andruavTasks == null) break;
                        c_dumpError.fn_dumpdebug("delete tasks command");
                        let v_params = {
                            resultfunc: function (res) {
                                if (res.length == 0) {
                                    // no data
                                    c_dumpError.fn_dumpdebug("Data Found:" + res.length);

                                }
                                else {
                                    // data found
                                    c_dumpError.fn_dumpdebug("Rows Length:" + res.length);
                                    v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
                                    v_jmsg.tg = p_ws.name; // sender = target
                                    v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                                    v_jmsg.mt = '9003';
                                    v_jmsg.ms = "Done";
                                    c_WS.send(JSON.stringify(v_jmsg));

                                }

                            },
                            errfunc: function (err) {
                                c_dumpError.fn_dumperror(err);
                            }
                        };

                        let mms = v_jmsg.ms;
                        if (typeof mms === 'string' || mms instanceof String) {
                            // backword compatible
                            mms = JSON.parse(v_jmsg.ms); //Internal p_message JSON
                        }
                        c_dumpError.fn_dumpdebug(mms);
                        // c_dumpError.fn_dumpdebug(mms.messageType);
                        if ((mms.ai == null) || (mms.ai.length == 0)) {
                            return; // error List all crosee accounts tasks ... ERROR
                        }


                        if (mms.hasOwnProperty("ac")) v_params.accessCode = mms.ac;
                        if (mms.hasOwnProperty("ai")) v_params.accountID = mms.ai;
                        if (mms.hasOwnProperty("ps")) v_params.party_sid = mms.ps;
                        if (mms.hasOwnProperty("gn")) v_params.groupName = mms.gn;
                        if (mms.hasOwnProperty("s")) v_params.sender = mms.s;
                        if (mms.hasOwnProperty("r")) v_params.receiver = mms.r;
                        if (mms.hasOwnProperty("mt")) v_params.messageType = mms.mt;
                        if (mms.hasOwnProperty("ip")) v_params.isPermanent = mms.ip;
                        if (mms.hasOwnProperty("t")) v_params.task = JSON.stringify(mms.t);


                        c_dumpError.fn_dumpdebug(v_params);
                        //v_andruavTasks.delete_tasks(v_params);
                    }
                    break;
                case c_CONSTANTS.CONST_TYPE_AndruavSystem_DisableTasks:
                    {
                        c_dumpError.fn_dumpdebug("disable tasks command");
                        let v_params = {
                            resultfunc: function (res) {
                                if (res.affectedRows == 0) {
                                    // no data
                                    c_dumpError.fn_dumpdebug("Data Found:" + res.affectedRows);

                                }
                                else {
                                    // data found
                                    c_dumpError.fn_dumpdebug("Rows Length:" + res.affectedRows);
                                    v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
                                    v_jmsg.tg = p_ws.name; // sender = target
                                    v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                                    v_jmsg.mt = '9003';
                                    v_jmsg.ms = "Done";
                                    c_WS.send(JSON.stringify(v_jmsg));

                                }

                            },
                            errfunc: function (err) {
                                c_dumpError.fn_dumperror(err);
                            }
                        };

                        let mms = v_jmsg.ms;
                        if (typeof mms === 'string' || mms instanceof String) {
                            // backword compatible
                            mms = JSON.parse(v_jmsg.ms); //Internal p_message JSON
                        }
                        if ((mms.ai == null) || (mms.ai.length == 0)) {
                            return; // No Global wide operation is allowed
                        }

                        c_dumpError.fn_dumpdebug(mms);

                        if (mms.hasOwnProperty("ac")) v_params.accessCode = mms.ac;
                        if (mms.hasOwnProperty("ai")) v_params.accountID = mms.ai;
                        if (mms.hasOwnProperty("ps")) v_params.party_sid = mms.ps;
                        if (mms.hasOwnProperty("gn")) v_params.groupName = mms.gn;
                        if (mms.hasOwnProperty("s")) v_params.sender = mms.s;
                        if (mms.hasOwnProperty("r")) v_params.receiver = mms.r;
                        if (mms.hasOwnProperty("mt")) v_params.messageType = mms.mt;
                        if (mms.hasOwnProperty("ip")) v_params.isPermanent = mms.ip;
                        if (mms.hasOwnProperty("t")) v_params.task = JSON.stringify(mms.t);

                        c_dumpError.fn_dumpdebug(v_params);
                        v_andruavTasks.fn_disable_tasks(v_params);
                    }
                    break;

            }
        }
        catch (e) {
            c_dumpError.fn_dumperror(e);
            if (global.m_logger) global.m_logger.Error('Bad Parsing Message:CONST_WS_MSG_ROUTING_SYSTEM', 'fn_parseMessage', null, e);
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
                if ((v_loginTempKey.fn_isAlphanumeric() !== true) || (v_loginTempKey.length > c_PARAM_LENGTH)) {
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

        
        

        

        function fn_onWsMessage(p_msg) {
            //console.log ("debug ... fn_onWsMessage code: " + p_msg);
            let v_isBinary = false;
            if (typeof (p_msg) !== 'string') {
                v_isBinary = true;
            }
            fn_parseMessage(this, p_msg, v_isBinary);
            p_msg = null;
        }


        function fn_onWsClose(p_code) {
            // this function can be called during key validation.
            // also this function is called when terminated a socket when a senderID kicks out an older unit with same senderID.
            // in this case m__terminated = true other wise the new senderID will also kick itself.
            //console.log ("debug ... fn_onWsClose code: " + p_code + " of key " + v_loginTempKey);
            if (this.hasOwnProperty('m__terminated' == false) || (this.m__terminated == false)) {
                c_ChatAccountRooms.fn_del_member_fromAccountByName(this.m_loginRequest, true);
            }

            // remove from active senderIDs list.
            if (p_ws.m_loginRequest != null) {
                c_andruav_active_senders.deleteActiveSenderIDList(p_ws.m_loginRequest.m_senderID);
                if (c_CommServerManagerClient.fn_onMessageOpened) {
                    c_CommServerManagerClient.fn_onMessageOpened();
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
        else
        {

            fn_validateKey(c_params);

            if (v_loginTempKey != null) {
                // OK THIS IS A VALID LOGIN... Lets' get him in the right chat room and send a welcome reply.

                acceptConnection (v_loginTempKey, c_params, p_ws);
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
    const options = {
        key: v_fs.readFileSync(v_path.join(__dirname, '../', global.m_serverconfig.m_configuration.ssl_key_file.toString())),
        cert: v_fs.readFileSync(v_path.join(__dirname, '../', global.m_serverconfig.m_configuration.ssl_cert_file.toString()))
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
    else
    {
        v_wss = new v_WebSocketServer({
        server: wserver});
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

function fn_initTasks() {
    if (global.m_serverconfig.m_configuration.ignoreLoadingTasks === false) {
        v_andruavTasks = require("../js_andruavTasks_v2.js");
        v_andruavTasks.fn_initTasks();
    }
}


function fn_startServer() {
    console.log(global.Colors.BSuccess + "[OK] Comm Server Manager has Started at port " + global.m_serverconfig.m_configuration.server_port + global.Colors.Reset);
    c_PluginManager.fn_initPlugins();
    fn_initTasks();
    fn_startChatServer();
}





module.exports = {
    fn_startServer: fn_startServer,
    fn_parseExternalMessage
};