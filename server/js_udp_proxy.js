/***
 * 
 * 
 *  Author: Mohammad S. Hefny
 *  Date 04 Sep 2022
 * 
 */

 "use strict";

    


 class udp_socket {
 
     constructor(host,port,func, parent)
     {
         this._isReady = false;
         this.parent = parent;
         this.dgram = require('dgram');
         this._ready_counter=0;
         this._caller_port = null;
         this._caller_ip   = null;
         this._server = null;
         this._host   = host;
         this._port   = port;
         this._onMessageReceived = func;
         this._last_access_time = 0;
         this._server = this.dgram.createSocket('udp4');
 
         this._server.on('listening', function () {
             Me._port = this.address().port;
             Me._host = this.address().address;
             Me._isReady = true;
             Me.parent._onReady(Me.parent, Me._isReady);
             console.log('UDP Listener Active ' + Me._host + ' at port ' + Me._port);
         });
         var _onMessageReceived = this._onMessageReceived;
         var Me = this;
         this._server.on('message', function (message, remote) {
             Me._last_access_time = Date.now();
             Me._caller_ip   = remote.address;
             Me._caller_port = remote.port;
             if (_onMessageReceived!= undefined)
             {
                 _onMessageReceived (message,Me.parent);
             }
         });

         this._server.on('error', function (err)
         {
            if (err.code == 'EADDRINUSE')
            {
                console.log('UDP Listener Cannot Open ' + Me._host + ' at port ' + Me._port);
                Me._isReady = false;
                Me.parent._onReady(Me.parent, Me._isReady);
            }
            console.log ("socket error:" + err);
         });
        
         try
         {
            this._server.bind({
                'address': host,
                'port': port,
                'exclusive': true
              });
         }
         catch 
         {
            this._isReady = false;
            this.parent._onReady(this.parent, this._isReady);
         }
         
     }
 
     close ()
     {
         try
         {
            this._isReady = false;
            this._server.close();
         }
         catch
         {
             
         }
     }

     isReady ()
     {
        return this._isReady;
     }
 
     getLastAccessTime()
     {
         return this._last_access_time;
     }
 
     setOnReceive (func)
     {
         _onMessageReceived = func;
     }
     
     sendMessage (message)
     {
         if ((this._caller_port == null) || (this._server==null)) return ;
         this._server.send(message, 0, message.length, this._caller_port, this._caller_ip); 
     }
 
     getConfig()
     {
        /*
            Note that socket may be listening to an ip that is not the public IP.
            so you need to return the public IP "public_host".
            The only exception is that host is listening to a particular ip given by this._host
            in this case chat parties can see tihis ip as it is specified by them.
        */
        var host = global.m_serverconfig.m_configuration.public_host;

        if (this._host != "0.0.0.0")
        {
            host = this._host;
        }
         var config = {
             'address':host,
             'port': this._port
         };
 
         return config;
     }
     
 }
 
 
 class udp_proxy {
     constructor (host1, port1, host2, port2, callback)
     {
         this._callback = callback;
         this._ready_counter = 0;
         this._ready_proxy = true;   

         host1=host1==null?"0.0.0.0":host1;
         port1=port1==null?0:port1;
         host2=host2==null?"0.0.0.0":host2;
         port2=port2==null?0:port2;
         
         this._udp_socket1 = new udp_socket(host1,port1, this.udp2_onreceive, this);
         this._udp_socket2 = new udp_socket(host2,port2, this.udp1_onreceive, this);
 
     }
 
     _onReady(Me, status)
     {
        this._ready_proxy = this._ready_proxy && status;
        Me._ready_counter +=1;
        if (Me._ready_counter==2)
        {
            this._callback(this._ready_proxy);
        }
     }
 
     close()
     {
         this._udp_socket1.close();
         this._udp_socket2.close();
     }
 
     getConfig()
     {
         var config = 
         {
             'socket1': this._udp_socket1.getConfig(),
             'socket2': this._udp_socket2.getConfig()
         }
 
         return config;
     }

     isReady ()
     {
        return this._udp_socket1.isReady() && this._udp_socket1.isReady();
     }

     udp1_onreceive (message, Me)
     {
         Me._udp_socket1.sendMessage(message); 
     }
 
     udp2_onreceive (message, Me)
     {
         Me._udp_socket2.sendMessage(message); 
     }
    
 }
 


 const m_activeUdpProxy = {};


 function closeUDPSocket (name, callback)
 {
    var ms = {};
    if (m_activeUdpProxy.hasOwnProperty(name))
    {
        ms = m_activeUdpProxy[name].m_udpproxy.getConfig();
        m_activeUdpProxy[name].m_udpproxy.close();
        m_activeUdpProxy[name] = null;
    }
    else
    {
        ms = {
            'socket1': {'address':'0.0.0.0', 'port':0},
            'socket2': {'address':'0.0.0.0', 'port':0}
        };
    }

    ms.en = false;

    callback(ms);
 }

 
 function getUDPSocket (name, socket1, socket2, callback)
    {
        if ((!m_activeUdpProxy.hasOwnProperty(name)) || (m_activeUdpProxy[name]==null))
        {   // new socket
            var obj = {};
            obj.created = Date.now();
            obj.last_access = Date.now();
            m_activeUdpProxy[name] = obj;
            
            obj.m_udpproxy = new udp_proxy("0.0.0.0", socket1.port,"0.0.0.0", socket2.port, function (enabled)
            {
                var ms = obj.m_udpproxy.getConfig();
                ms.en = enabled;
                callback(ms);
            }); 
        }
        else
        {   // this unit has already a socket
            var ms = m_activeUdpProxy[name].m_udpproxy.getConfig();

            if (((socket1.port ==0) || (ms.socket1.port == socket1.port)) && ((socket2.port ==0 ) || (ms.socket2.port == socket2.port)))
            {  // same socket same configuration.
                ms.last_access = Date.now();
                ms.en = true;
                callback(ms);
            }
            else
            {   // close unit old socket 
                closeUDPSocket (name, function ()
                {
                    getUDPSocket (name, socket1, socket2, callback); // recursive to create a new one after deleting the current.
                });
            }
        }
    }
 

 module.exports = 
 {
     udp_socket,
     udp_proxy,
     getUDPSocket,
     closeUDPSocket
 }
 
 