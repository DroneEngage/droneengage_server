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
            console.log ("socket error:" + err);
         });
        
         try
         {
            this._server.bind(port, host);
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
 
         host1=host1==null?"0.0.0.0":host1;
         port1=port1==null?0:port1;
         host2=host2==null?"0.0.0.0":host2;
         port2=port2==null?0:port2;
         
         this._udp_socket1 = new udp_socket(host1,port1, this.udp2_onreceive, this);
         this._udp_socket2 = new udp_socket(host2,port2, this.udp1_onreceive, this);
 
         //this.m_watchdog = setInterval (this.checkIdle, 20000, this);
     }
 
     // checkIdle(Me)
     // {
     //     const now = Date.now();
     //     if (((now - Me._udp_socket1.getLastAccessTime()) > 20000)
     //         || (now - Me._udp_socket2.getLastAccessTime()) > 20000)
     //     {
     //         Me.close(Me);
     //     }
     // }
 
     _onReady(Me, status)
     {
         Me._ready_counter +=1;
         if (Me._ready_counter==2)
         {
             this._callback();
         }
     }
 
     close()
     {
         this._udp_socket1.close();
         this._udp_socket2.close();
         //clearInterval(Me.m_watchdog);
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
 
 module.exports = 
 {
     udp_socket,
     udp_proxy
 }
 
 