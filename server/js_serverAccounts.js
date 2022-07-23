
const { v4: uuidv4 } = require('uuid');
const c_dumpError = require("./js_dumperror.js");

function consoleLog(text)
{
	console.log (text);
}


function AccountsMaster ()
{
    // list of Account
    this._accounts = {};
}



AccountsMaster.prototype.getUnitKeys = function ()
{
	return Object.keys(this._accounts);
	
};


AccountsMaster.prototype.getUnitValues = function ()
{
	return Object.values(this._accounts);
	
};


AccountsMaster.prototype.getUnitCount = function ()
{
	if (andruavClient.andruavUnitList == null) return 0;
	return Object.keys(this._accounts).length; 
};


AccountsMaster.prototype.fn_add_member_toGroup = function (p_unitname,p_groupname,p_websocket) 
{
    var id  = p_websocket.m_andruavParams.SID;
    var acc; 
    if (this._accounts.hasOwnProperty(id))
    {
        // account already exists
        consoleLog ("account " + id + " already exists")
        acc = this._accounts[id];
    }
    else
    {
        // account does not exist
         consoleLog ("account " + id + " created")
         acc = this._accounts[id] = new Account(id);
    }

    return acc.fn_add_member_toGroup (p_unitname,p_groupname,p_websocket);
}



AccountsMaster.prototype.del_member_fromGroup = function (p_websocket)
{
    if (p_websocket.hasOwnProperty("group")== false)
    {
        // socket is not linked to any group
        consoleLog ("del_member_fromGroup : Nothing to Delete");
        return ;
    }

    if (p_websocket.name == null)  // undefined or null
    {
        delete p_websocket.group;
                            
        consoleLog ('No unit name to remove');
        return ;
    }
    
   return p_websocket.group.fn_deleteMemberByName(p_websocket.name);

}



AccountsMaster.prototype.fn_del_member_fromAccountByName = function (p_unitname, accountID,terminateSocket)
{
   
  var acc = this._accounts[accountID];

  if (acc == null)
  {
      consoleLog ("info: no account associated with socket .... This could be a brand new socket.");
      
      
      return ;
  }

  acc.fn_del_member_fromAccountByName (p_unitname,terminateSocket);

}



AccountsMaster.prototype.forEach = function (callback)
{
    var keys = Object.keys(this._accounts) ; 
    var len = keys.length;
    //consoleLog ("AccountsMaster.forEach Keys:" + keys);
     for (var i=0; i < len; ++i)
     {
        callback (this._accounts[keys[i]]);
     }
}



///////////////////////////////////////////////////  Account




function Account (id)
{
    this.ID = id;
    this._groups = {};
    
    Object.seal(this);
}

/***
 * This is a Facade Layer that addes a member to a group.
 * Account/Group are created if any not existed.
 * Returns: true/false
 ***/
Account.prototype.fn_add_member_toGroup = function (p_unitname,p_groupname,p_websocket)
{
    var gr;
    if (this._groups.hasOwnProperty(p_groupname))
    {
        // group already exists
        consoleLog ("group " + p_groupname + " already exists")
        
        gr = this._groups[p_groupname];
    }
    else
    {
        // group does not exist
         consoleLog ("group " + p_groupname + " created")
       
        gr = this._groups[p_groupname] = new Group(this,p_groupname);
    }

    return gr.fn_addMember (p_unitname,p_websocket);
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
    var keys = Object.keys(this._groups) ; 
    var len = keys.length;
    //consoleLog ("Account.forEach Keys:" + keys);
    for (var i=0; i < len; ++i)
    {
        callback (this._groups[keys[i]]);
    }
}

///////////////////////////////////////// GROUP

/***
 * Group Object
 ***/
function Group (account,id)
{
    this.ID = id;
    this._parentAccount = account; //  u should make this == null if you want to delete this object.
    this._units = {};

    this.BTX = 0;
    this.TTX = 0;
    this.m_creationDate = new Date();
    this.lastAccessTime = new Date();
    this.m_andruavSockets = new Map();
    this.lng = 0;
    this.lat = 0;
    this.m_speed = 0.0;
    this.alt = 0.0;
    this.gps = false;
    this.uid = uuidv4();
    
    Object.seal(this);
}



/***
 * This is a Facade Layer that addes a member to a group.
 * Account/Group are created if any not existed.
 * Returns: true/false
 ***/
Group.prototype.fn_addMember = function (p_unitname,p_websocket)
{

    if (this._units.hasOwnProperty(p_unitname))
    {
        // this should never happen.
        consoleLog ("fn_addMember [" + p_unitname + "] already EXISTS Dont Override")
       

       return false; 
    }
    else
    {
        consoleLog ("fn_addMember [" + p_unitname + "] Added")
       
        this._units[p_unitname] = p_websocket;
        p_websocket.name = p_unitname;
        p_websocket.group = this;
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
        if (this._units.hasOwnProperty(p_unitname))
        {

            consoleLog ("fn_deleteMemberByName: deleteMember " + p_unitname);
    

            // this is a socket under the same name 
            var oldSocket = this._units[p_unitname];
            delete this._units[p_unitname];
            delete oldSocket.group;
            if (terminateSocket==true)
            {

                consoleLog ("fn_deleteMemberByName: terminateSocket " + p_unitname);
    

                oldSocket.fn_register_db(oldSocket);
                oldSocket.terminate();
            }
    
        }
    }
    catch (e)
    {
        c_dumpError.fn_dumperror(e);
    }
}

Group.prototype.forEach = function (callback)
{
    var keys = Object.keys(this._units) ; 
    var len = keys.length;
   // consoleLog ("Group.forEach Keys:" + keys);
   
   for (var i=0; i < len; ++i)
   {
        callback (this._units[keys[i]]);
   }
}


Group.prototype.fn_sendToIndividual = function(message, v_isBinary, target)
{
   try
    {
            
        //xconsoleLog ('func: send fn_sendToIndividual %s' ,target);
        var socket = this._units[target];
        if (socket != null)
        {
            //xconsoleLog ('func: send message to %s' ,socket.Name);

            socket.send(message,
            {
                binary: v_isBinary
            });
            return;
        }
    }
    catch (e)
    {
            consoleLog('fn_broadcast :ws:' + socket.Name + ' Orphan socket Error:' + e);
            c_dumpError.fn_dumperror(e);
           // if (e.message == "not opened")
           // {
               consoleLog('========================================');
               if (socket != null)
                    { // Most propably this is the same socket disconnected silently.
                        
                        try
                        {
                            socket.group.fn_deleteMemberByName (socket.name);
               
                            consoleLog ('unit' + socket.Name + ' found dead');
                            /////////fn_register_db(oldSocket);
                            //oldSocket.Name = null; // to prevent onClose ->unregister->del_member_fromGroup so deletes the new record.
                            socket.fn_register_db ();
                            delete socket.name;
                            
                            socket.terminate();

                        }
                        catch (e)
                        {
                            c_dumpError.fn_dumperror(e);
                        }
                    }
            //}
        }

}

Group.prototype.fn_broadcast = function(message, v_isBinary, ws)
{
    var keys = Object.keys(this._units) ; 
    var len = keys.length;
    // consoleLog ("Group.forEach Keys:" + keys);
   
    for (var i=0; i < len; ++i)
    {
         try
        {
            var socket = this._units[keys[i]];
            if (socket.m_andruavParams.uid != ws.m_andruavParams.uid)
            {
                //xconsoleLog ('func: send message to %s' ,value.Name);

                socket.send(message,
                {
                    binary: v_isBinary
                });
            }
        }
        catch (e)
        {
            consoleLog('fn_broadcast :ws:' + socket.Name + ' Orphan socket Error:' + e);
            c_dumpError.fn_dumperror(e);
            consoleLog('========================================');
            if (socket != null)
            { // Most propably this is the same socket disconnected silently.
                        
                try
                {
                    socket.group.fn_deleteMemberByName (socket.name);
               
                    consoleLog ('unit' + socket.Name + ' found dead');
                    /////////fn_register_db(oldSocket);
                    //oldSocket.Name = null; // to prevent onClose ->unregister->del_member_fromGroup so deletes the new record.
                    socket.fn_register_db ();
                    delete socket.name;
                    socket.terminate();

                }
                catch (e)
                {
                    c_dumpError.fn_dumperror(e);
                }
            }
        
        }
    }

}



exports.m_andruavSocket = function m_andruavSocket()
{
    //this.RX=0;
    //this.TX=0;

    this.fn_init = function()
    {
        this.lng = 0;
        this.lat = 0;
        this.m_speed = 0.0;
        this.alt = 0.0;
    }


}


exports.AccountMaster = new AccountsMaster ();
