/***
 * 
 * Author: Mohammad S. Hefny
 * Date: 04 Sep 2022
 * 
 */

"use strict";

class udp_socket {
    constructor(host, port, func, parent) {
        this._isReady = false;
        this.parent = parent;
        this.dgram = require('dgram');
        this._ready_counter = 0;
        this._caller_port = null;
        this._caller_ip = null;
        this._server = null;
        this._host = host;
        this._port = port;
        this._onMessageReceived = func;
        this._last_access_time = 0;
        this._server = this.dgram.createSocket('udp4');

        this._server.on('listening', () => {
            this._port = this._server.address().port;
            this._host = this._server.address().address;
            this._isReady = true;
            this.parent._onReady(this.parent, this._isReady);
            console.log('UDP Listener Active ' + this._host + ' at port ' + this._port);
        });

        this._server.on('message', (message, remote) => {
            this._last_access_time = Date.now();
            this._caller_ip = remote.address;
            this._caller_port = remote.port;
            if (this._onMessageReceived) {
                this._onMessageReceived(message, this.parent);
            }
        });

        this._server.on('error', (err) => {
            if (err.code === 'EADDRINUSE') {
                console.log('UDP Listener Cannot Open ' + this._host + ' at port ' + this._port);
                this._isReady = false;
                this.parent._onReady(this.parent, this._isReady);
            }
            console.log("socket error: " + err);
        });

        try {
            this._server.bind({
                address: host,
                port: port,
                exclusive: true
            });
        } catch (err) {
            console.error("Binding error: ", err);
            this._isReady = false;
            this.parent._onReady(this.parent, this._isReady);
        }
    }

    close() {
        try {
            this._isReady = false;
            this._server.close();
        } catch (err) {
            console.error("Error closing socket: ", err);
        }
    }

    isReady() {
        return this._isReady;
    }

    getLastAccessTime() {
        return this._last_access_time;
    }

    setOnReceive(func) {
        this._onMessageReceived = func;
    }

    sendMessage(message) {
        if (this._caller_port === null || this._server === null) return;
        this._server.send(message, 0, message.length, this._caller_port, this._caller_ip);
    }

    getConfig() {
        /*
            Note that socket may be listening to an ip that is not the public IP.
            so you need to return the public IP "public_host".
            The only exception is that host is listening to a particular ip given by this._host
            in this case chat parties can see tihis ip as it is specified by them.
        */
            let host = global.m_serverconfig.m_configuration.public_host;

        if (this._host !== "0.0.0.0") {
            host = this._host;
        }

        return {
            address: host,
            port: this._port
        };
    }
}

class udp_proxy {
    constructor(host1, port1, host2, port2, callback) {
        this._callback = callback;
        this._ready_counter = 0;
        this._ready_proxy = true;

        host1 = host1 || "0.0.0.0";
        port1 = port1 || 0;
        host2 = host2 || "0.0.0.0";
        port2 = port2 || 0;

        this._udp_socket1 = new udp_socket(host1, port1, this.udp2_onreceive.bind(this), this);
        this._udp_socket2 = new udp_socket(host2, port2, this.udp1_onreceive.bind(this), this);
    }

    _onReady(Me, status) {
        this._ready_proxy = this._ready_proxy && status;
        Me._ready_counter += 1;
        if (Me._ready_counter === 2) {
            this._callback(this._ready_proxy);
        }
    }

    close() {
        try {
            this._udp_socket1.close();
        } catch (err) {
            console.error("Error closing socket1: ", err);
        }
        try {
            this._udp_socket2.close();
        } catch (err) {
            console.error("Error closing socket2: ", err);
        }

        this._udp_socket1.parent = null;
        this._udp_socket2.parent = null;

        this._udp_socket1 = null;
        this._udp_socket2 = null;
    }

    getConfig() {
        return {
            socket1: this._udp_socket1.getConfig(),
            socket2: this._udp_socket2.getConfig()
        };
    }

    isReady() {
        return this._udp_socket1.isReady() && this._udp_socket2.isReady();
    }

    udp1_onreceive(message, Me) {
        Me._udp_socket1.sendMessage(message);
    }

    udp2_onreceive(message, Me) {
        Me._udp_socket2.sendMessage(message);
    }
}

const m_activeUdpProxy = {};

function closeUDPSocket(name, callback) {
    let ms = {};
    if (m_activeUdpProxy.hasOwnProperty(name)) {
        ms = m_activeUdpProxy[name].m_udpproxy.getConfig();
        m_activeUdpProxy[name].m_udpproxy.close();
        delete m_activeUdpProxy[name];
    } else {
        ms = {
            socket1: { address: '0.0.0.0', port: 0 },
            socket2: { address: '0.0.0.0', port: 0 }
        };
    }

    ms.en = false;
    callback(ms);
}

function getUDPSocket(name, socket1, socket2, callback) {
    if (!m_activeUdpProxy.hasOwnProperty(name) || m_activeUdpProxy[name] == null) {
        // New socket
        const obj = {
            created: Date.now(),
            last_access: Date.now()
        };
        m_activeUdpProxy[name] = obj;

        obj.m_udpproxy = new udp_proxy("0.0.0.0", socket1.port, "0.0.0.0", socket2.port, (enabled) => {
            const ms = obj.m_udpproxy.getConfig();
            ms.en = enabled;
            callback(ms);
        });
    } else {
        // This unit has already a socket
        const ms = m_activeUdpProxy[name].m_udpproxy.getConfig();

        if ((socket1.port === 0 || ms.socket1.port === socket1.port) && (socket2.port === 0 || ms.socket2.port === socket2.port)) {
            // Same socket same configuration.
            ms.last_access = Date.now();
            ms.en = true;
            callback(ms);
        } else {
            // Close unit old socket
            closeUDPSocket(name, () => {
                getUDPSocket(name, socket1, socket2, callback); // Recursive to create a new one after deleting the current.
            });
        }
    }
}

module.exports = {
    udp_socket,
    udp_proxy,
    getUDPSocket,
    closeUDPSocket
};