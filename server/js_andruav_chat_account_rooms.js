"use strict";

/**
 * This module represents CHAT ROOMS for a given ACCOUNT-ID
 * each Room is related to a Group.
 */



const c_uuidv4 = require('uuidv4');
const _dumpError = require("../dumperror.js");


const c_accounts = {};

function fn_Accounts () 
{

}

function fn_getUnitKeys ()
{
	return Object.keys(c_accounts);
	
};


function fn_getUnitValues ()
{
	return Object.values(this._accounts);
	
};


function fn_getUnitCount()
{
	return Object.keys(c_accounts).length; 
};


function fn_add_member_toGroup (p_loginRequest) 
{
    const v_id  = p_loginRequest.m_accountID;
    var v_acc; 

    if (c_accounts.hasOwnProperty(v_id))
    {
        // account already exists
        console.log ("account " + v_id + " already exists")
        v_acc = c_accounts[v_id];
    }
    else
    {
        // account does not exist
         console.log ("account " + v_id + " created")
         v_acc = new Account (p_loginRequest.m_accountID);
         c_accounts[v_id] = v_acc; 
    }

    return v_acc.fn_add_member_toGroup (p_loginRequest.m_senderID ,p_loginRequest.m_groupID, p_loginRequest.m_ws);
}



function fn_del_member_fromGroup (p_websocket)
{
    if (p_websocket.hasOwnProperty("m__group")== false)
    {
        // socket is not linked to any group
        console.log ("del_member_fromGroup : Nothing to Delete");
        return ;
    }

    if (p_websocket.name == null)  // undefined or null
    {
        delete p_websocket.m__group;
                            
        console.log ('No unit name to remove');
        return ;
    }
    
   return p_websocket.m__group.fn_deleteMemberByName(p_websocket.name);

}



function fn_del_member_fromAccountByName (p_loginRequest, terminateSocket)
{
   
  var acc = c_accounts[p_loginRequest.m_accountID];

  if (acc == null)
  {
      console.log ("info: no account associated with socket .... This could be a brand new socket.");
      
      
      return ;
  }

  acc.fn_del_member_fromAccountByName (p_loginRequest.m_senderID,terminateSocket);

}



function fn_forEach (callback)
{
    var keys = Object.keys(c_accounts) ; 
    var len = keys.length;
    //console.log ("AccountsMaster.forEach Keys:" + keys);
     for (var i=0; i < len; ++i)
     {
        callback (c_accounts[keys[i]]);
     }
}





///////////////////////////////////////////////////  Account




function Account (p_accountID)
{
    this.m_accountID = p_accountID;
    this.m_groups = {};
    Object.seal(this);
}

/***
 * This is a Facade Layer that addes a member to a group.
 * Account/Group are created if any not existed.
 * Returns: true/false
 ***/
Account.prototype.fn_add_member_toGroup = function (p_unitname, p_groupname, p_ws)
{
    var gr;
    
    if (this.m_groups.hasOwnProperty(p_groupname))
    {
        // group already exists
        console.log ("group " + p_groupname + " already exists")
        
        gr = this.m_groups[p_groupname];
    }
    else
    {
        // group does not exist
         console.log ("group " + p_groupname + " created")
       
        gr = this.m_groups[p_groupname] = new Group(this,p_groupname);
    }

    return gr.fn_addMember (p_unitname, p_ws);
}

/***
 * Searchs in all groups and remove all sockets with that name.
 ***/
Account.prototype.fn_del_member_fromAccountByName = function (p_unitname, terminateSocket)
{
    this.forEach (function (group)
    {
        group.fn_deleteMemberByName (p_unitname,terminateSocket);    
    });
}


Account.prototype.forEach = function (callback)
{
    var keys = Object.keys(this.m_groups) ; 
    var len = keys.length;
    //console.log ("Account.forEach Keys:" + keys);
    for (var i=0; i < len; ++i)
    {
        callback (this.m_groups[keys[i]]);
    }
}

///////////////////////////////////////// GROUP

/***
 * Group Object
 ***/
function Group (m_accountObj, p_ID)
{
    this.m_ID = p_ID;
    this.m_parentAccount = m_accountObj; //  u should make this == null if you want to delete this object.
    this.m_units = {};

    this.uid = c_uuidv4.uuid();
    this.m_creationDate = Date.now();
    this.m_TTX = 0;
    this.m_BTX = 0;
    this.m_lastAccessTime = 0;
    
    this.m_lng = 0;
    this.m_lat = 0;
    this.m_speed = 0;
    this.m_alt = 0;
    this.m_gps = false;
    this.m_isFlyingDone = false;
    
    
    Object.seal(this);
}



/***
 * This is a Facade Layer that addes a member to a group.
 * Account/Group are created if any not existed.
 * Returns: true/false
 ***/
Group.prototype.fn_addMember = function (p_unitname, p_ws)
{

    if (this.m_units.hasOwnProperty(p_unitname))
    {
        // this should never happen.
        console.log ("addMember [" + p_unitname + "] already EXISTS Dont Override")
       

       return false; 
    }
    else
    {
        console.log ("addMember [" + p_unitname + "] Added")
       
        this.m_units[p_unitname] = p_ws;
        p_ws.name = p_unitname;
        p_ws.m__group = this;
    }
    
    return true;
}


/***
 * Deletes a member from a group.
 * This deletes by name ... two sockets with the same name will delete each others.
 
Group.prototype.deleteMember = function (p_websocket)
{
   return p_websocket;
}
***/

/***
 * Deletes sockets with a given name. even if socket is not the same instance.
 ***/
Group.prototype.fn_deleteMemberByName = function (p_unitname,terminateSocket)
{
    try
    {
        if (this.m_units.hasOwnProperty(p_unitname))
        {

            console.log ("deleteMemberByName: deleteMember " + p_unitname);
    

            // this is a socket under the same name 
            var oldSocket = this.m_units[p_unitname];
            delete this.m_units[p_unitname];
            delete oldSocket.m__group;
            if (terminateSocket==true)
            {

                console.log ("deleteMemberByName: terminateSocket " + p_unitname);
    

                //oldSocket.unregister_db(oldSocket);
                oldSocket.m__terminated = true;
                oldSocket.terminate();
            }
    
        }
    }
    catch (e)
    {
        _dumpError.fn_dumperror(e);
    }
}

Group.prototype.forEach = function (callback)
{
    var keys = Object.keys(this.m_units) ; 
    var len = keys.length;
   // console.log ("Group.forEach Keys:" + keys);
   
   for (var i=0; i < len; ++i)
   {
        callback (this.m_units[keys[i]]);
   }
}


Group.prototype.fn_sendToIndividual = function(message, isbinary, target, c_ws)
{
   try
    {
            
        //xconsoleLog ('func: send sendToIndividual %s' ,target);
        var socket = this.m_units[target];
        if (socket != null)
        {
            //xconsoleLog ('func: send message to %s' ,socket.Name);

            socket.send(message,
            {
                binary: isbinary
            });
            return;
        }
    }
    catch (e)
    {
            console.log('broadcast :ws:' + socket.Name + ' Orphan socket Error:' + e);
            _dumpError.fn_dumperror(e);
           // if (e.message == "not opened")
           // {
               console.log('========================================');
               if (socket != null)
                    { // Most propably this is the same socket disconnected silently.
                        
                        try
                        {
                            socket.m__group.fn_deleteMemberByName (socket.name);
               
                            console.log ('unit' + socket.Name + ' found dead');
                            /////////unregister_db(oldSocket);
                            //oldSocket.Name = null; // to prevent onClose ->unregister->del_member_fromGroup so deletes the new record.
                            //socket.unregister_db ();
                            delete socket.name;
                            
                            socket.terminate();

                        }
                        catch (e)
                        {
                            _dumpError.fn_dumperror(e);
                        }
                    }
            //}
        }

}

Group.prototype.fn_broadcastToGCS = function(message, isbinary, c_ws)
{
    var keys = Object.keys(this.m_units) ; 
    var len = keys.length;
    // console.log ("Group.forEach Keys:" + keys);
   
    for (var i=0; i < len; ++i)
    {
         try
        {
            var socket = this.m_units[keys[i]];
            if ((socket.m_loginRequest.m_actorType === 'g')
                && (socket.m_loginRequest.m_senderID != c_ws.m_loginRequest.m_senderID))
            {
                //xconsoleLog ('func: send message to %s' ,value.Name);

                socket.send(message,
                {
                    binary: isbinary
                });
            }
        }
        catch (e)
        {
            console.log('broadcast :ws:' + socket.Name + ' Orphan socket Error:' + e);
            _dumpError.fn_dumperror(e);
            console.log('========================================');
            if (socket != null)
            { // Most propably this is the same socket disconnected silently.
                        
                try
                {
                    socket.m__group.fn_deleteMemberByName (socket.name);
               
                    console.log ('unit' + socket.Name + ' found dead');
                    /////////unregister_db(oldSocket);
                    //oldSocket.Name = null; // to prevent onClose ->unregister->del_member_fromGroup so deletes the new record.
                    //socket.unregister_db ();
                    delete socket.name;
                    socket.terminate();

                }
                catch (e)
                {
                    _dumpError.fn_dumperror(e);
                }
            }
        
        }
    }
}


Group.prototype.fn_broadcastToDrone = function(message, isbinary, c_ws)
{
    var keys = Object.keys(this.m_units) ; 
    var len = keys.length;
    // console.log ("Group.forEach Keys:" + keys);
   
    for (var i=0; i < len; ++i)
    {
         try
        {
            var socket = this.m_units[keys[i]];
            if ((socket.m_loginRequest.m_actorType === 'd')
                && (socket.m_loginRequest.m_senderID != c_ws.m_loginRequest.m_senderID))
            {
                //xconsoleLog ('func: send message to %s' ,value.Name);

                socket.send(message,
                {
                    binary: isbinary
                });
            }
        }
        catch (e)
        {
            console.log('broadcast :ws:' + socket.Name + ' Orphan socket Error:' + e);
            _dumpError.fn_dumperror(e);
            console.log('========================================');
            if (socket != null)
            { // Most propably this is the same socket disconnected silently.
                        
                try
                {
                    socket.m__group.fn_deleteMemberByName (socket.name);
               
                    console.log ('unit' + socket.Name + ' found dead');
                    /////////unregister_db(oldSocket);
                    //oldSocket.Name = null; // to prevent onClose ->unregister->del_member_fromGroup so deletes the new record.
                    //socket.unregister_db ();
                    delete socket.name;
                    socket.terminate();

                }
                catch (e)
                {
                    _dumpError.fn_dumperror(e);
                }
            }
        
        }
    }
}

Group.prototype.broadcast = function(p_message, p_isbinary, c_ws)
{
    var keys = Object.keys(this.m_units) ; 
    var len = keys.length;
    // console.log ("Group.forEach Keys:" + keys);
   
    for (var i=0; i < len; ++i)
    {
         try
        {
            var c_targetSocket = this.m_units[keys[i]];
            if (c_targetSocket.name != c_ws.name)
            {
                //xconsoleLog ('func: send message to %s' ,value.Name);

                c_targetSocket.send(p_message,
                {
                    binary: p_isbinary
                });
            }
        }
        catch (e)
        {
            console.log('broadcast :ws:' + c_targetSocket.Name + ' Orphan socket Error:' + e);
            _dumpError.fn_dumperror(e);
            console.log('========================================');
            if (c_targetSocket != null)
            { // Most propably this is the same socket disconnected silently.
                        
                try
                {
                    c_targetSocket.m__group.fn_deleteMemberByName (c_targetSocket.name);
               
                    console.log ('unit' + c_targetSocket.Name + ' found dead');
                    /////////unregister_db(oldSocket);
                    //oldSocket.Name = null; // to prevent onClose ->unregister->del_member_fromGroup so deletes the new record.
                    //c_targetSocket.unregister_db ();
                    delete c_targetSocket.name;
                    c_targetSocket.terminate();

                }
                catch (e)
                {
                    _dumpError.fn_dumperror(e);
                }
            }
        
        }
    }

}



module.exports = 
{
    fn_getUnitKeys:fn_getUnitKeys,
    fn_getUnitValues:fn_getUnitValues,
    fn_getUnitCount:fn_getUnitCount,
    fn_add_member_toGroup:fn_add_member_toGroup,
    fn_del_member_fromGroup:fn_del_member_fromGroup,
    fn_del_member_fromAccountByName:fn_del_member_fromAccountByName,
    fn_forEach: fn_forEach,
    fn_del_member_fromAccountByName: fn_del_member_fromAccountByName
};