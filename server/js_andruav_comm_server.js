"use strict";

const m_commServerManagerClient = require ("./js_comm_server_manager_client");
var m_agent_chat_server; 
const c_ChatAccountRooms = require("./js_andruav_chat_account_rooms");
const { v4: uuidv4 } = require('uuid');
const c_CONSTANTS = require ("../js_constants");
const v_version = require('../package.json').version;

/**
 * Decrypt message from Auth server. 
 * TODO: NOT IMPLEMENTED
 * @param {raw message from Auth} p_msg 
 */
function fn_decryptAuthMessage (p_msg)
{
    try
    {
        const v_msg =  JSON.parse(p_msg);

        return v_msg;
    }
    catch (ex)
    {
        console.log (ex);
    }

    return null;
}

/**
 * Generates a reply to a login request. Should be a temp key used by agent to login and server IP & port.
 * @param {*} p_cmd request from AUTH_SERVER
 * @returns 
 */
function fn_generateLoginRequestReply (p_cmd)
{
    const c_reply = {
        'c':c_CONSTANTS.CONST_CS_CMD_LOGIN_REQUEST, // info
        'd': {}
    };

    c_reply.d [c_CONSTANTS.CONST_CS_REQUEST_ID.toString()] = p_cmd.d[c_CONSTANTS.CONST_CS_REQUEST_ID.toString()];
    c_reply.d [c_CONSTANTS.CONST_CS_ERROR.toString()] = c_CONSTANTS.CONST_ERROR_NON;
    c_reply.d [c_CONSTANTS.CONST_CS_SERVER_PUBLIC_HOST.toString()] = global.m_serverconfig.m_configuration.public_host;
    c_reply.d [c_CONSTANTS.CONST_CS_SERVER_PORT.toString()] = global.m_serverconfig.m_configuration.server_port;
    c_reply.d [c_CONSTANTS.CONST_CS_LOGIN_TEMP_KEY.toString()] = p_cmd.d[c_CONSTANTS.CONST_CS_LOGIN_TEMP_KEY.toString()];
    
    return c_reply;
}


/**
 * Called whenever a connection established with AuthServer.
 * There is always one single Auth server.
 */
function fn_AuthServerConnectionHandler ()
{
    fn_updateServerWatchdog();
}

/**
 * Generates CONST_CS_LOGIN_TEMP_KEY to be sent to agent through AndruavAuth
 * Agent will use this key to connect to AndruavServer.
 * add it to waiting list so that can be either deleted when timeout or retrieved when agent connects to AndruavServer.            
 * @param {*} p_cmd JSON object of contains details of connection request.
 * p_cmd.c = 'b' for new login request.
 * p_cmd.d{a: SID, at: actor type (d for drone) d, b:GroupID=1, r: requestID GUID, f: login_temp_key (instead of party ID)}
 */
 function fn_handleLoginResponses (p_cmd)
{
    // Generates CONST_CS_LOGIN_TEMP_KEY to be sent to agent through AndruavAuth
    // Agent will use this key to connect to AndruavServer.
    p_cmd.d[c_CONSTANTS.CONST_CS_LOGIN_TEMP_KEY.toString()] = uuidv4().replaceAll('-','');
    // add it to waiting list so that can be either deleted when timeout or retrieved when agent connects to AndruavServer.            
    m_agent_chat_server.fn_addWaitingAccount (p_cmd.d[c_CONSTANTS.CONST_CS_LOGIN_TEMP_KEY.toString()],p_cmd.d);
    console.log ("New Party: " + JSON.stringify(p_cmd));

    const c_reply = fn_generateLoginRequestReply (p_cmd);

    m_commServerManagerClient.fn_sendMessage(JSON.stringify(c_reply));
}

/**
 * Handles messages received from Auth Server. It is called per message.
 * @param {raw messages from auth server} p_msg 
 */
function fn_AuthServerMessagesHandler (p_msg)
{
    try
    {
        const p_cmd = fn_decryptAuthMessage (p_msg);
        if ((p_cmd == null) || (!p_cmd.hasOwnProperty('c')))
        {
            // invalid command
            return;
        }

        switch (p_cmd.c)
        {
            case c_CONSTANTS.CONST_CS_CMD_LOGIN_REQUEST:
            {
                //return ; // testing to generate timeout at authenticator.
                
                if ((!p_cmd.hasOwnProperty('d'))
                    || p_cmd.d.hasOwnProperty('d')
                    )
                    break;
                fn_handleLoginResponses(p_cmd);
            }
            break;

            
            case c_CONSTANTS.CONST_CS_CMD_LOGOUT_REQUEST:
            {
                //@todo: not implemented    
                
            }
            break;
        }
    }
    catch (ex)
    {
        console.log ("err: fn_AuthServerMessagesHandler: " + ex);
    }
}


function fn_updateServerWatchdog()
{
    try
    {
        // prepare my Info Card
        var v_obj = { 
                    'isOnline':true,
                    'version': v_version,
                    'serverId':global.m_serverconfig.m_configuration.server_id,
                    'public_host':global.m_serverconfig.m_configuration.public_host, // this is the ip that is listening to the connections.
                    'serverPort':global.m_serverconfig.m_configuration.server_port,
                    // this is why we send this message with every connection.
                    // it is crucial to Auth server to know the exact unique list of keys
                    // so that it can rout correctly in case of multiple communication_servers
                    'accounts':c_ChatAccountRooms.fn_getUnitKeys() 
                };
        
        // send Info Card to Andruav Auth
        m_commServerManagerClient.fn_sendMessage(JSON.stringify(
            {
                'c': c_CONSTANTS.CONST_CS_CMD_INFO, // info
                'd': v_obj  // data
            }));
    }
    catch (ex)
    {
        console.log ("Error:fn_updateServerWatchdog:" + ex.toString());
    }
}

/**
 * Start Server
 */
function fn_startServer ()
{

    console.log (global.Colors.Success + "[OK] Communication Server Started"  + global.Colors.Reset);

    if (global.m_serverconfig.m_configuration.allow_fake_SSL ===true)
    {
        // ATTENTION: should be FALSE in PRODUCTION
        // skip verifying fake SSL
        console.log (global.Colors.BFgYellow + "ATTENTION!!  SECURITY RISK . NODE_TLS_REJECT_UNAUTHORIZED = 0" + global.Colors.Reset);

        process.env["NODE_TLS_REJECT_UNAUTHORIZED"] = 0;
    } 
    else if (global.m_serverconfig.m_configuration.ca_cert_path ===true)
    {
        process.env["NODE_EXTRA_CA_CERTS"] = global.m_serverconfig.m_configuration.ca_cert_path;
    }

    

    // if (global.m_serverconfig.m_configuration.dynamicKEY)
    // {
    //     // initialize key once
    //     fn_refreshDynamicKeys(); 
        
    //     // start a timer
    //     setInterval(function()
    //     {
    //         fn_refreshDynamicKeys()
    //     }, global.m_serverconfig.m_configuration.dynamicKEYInterval); // Time in milliseconds;
        
    // }

    m_commServerManagerClient.fn_onMessageReceived  = fn_AuthServerMessagesHandler;
    m_commServerManagerClient.fn_onMessageOpened    = fn_AuthServerConnectionHandler;
    m_commServerManagerClient.fn_startServer();
    m_agent_chat_server = global.m_chat_server_singelton_get_instance();
    m_agent_chat_server.fn_startServer();
    
}


module.exports = {
    fn_startServer: fn_startServer,
};