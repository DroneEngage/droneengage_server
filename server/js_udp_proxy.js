/***
 * 
 * Author: Mohammad S. Hefny
 * Date: 04 Sep 2022
 * 
 */

"use strict";

const dgram = require('dgram');
const fs = require('fs');
const os = require('os');

const PROXY_IDLE_TIMEOUT_MS = 5 * 60 * 1000; // 5 minutes
const REAPER_INTERVAL_MS = 60 * 1000; // Check every 1 minute

const UDP_RECV_BUFFER_SIZE = 4 * 1024 * 1024; // 4 MB receive buffer
const UDP_SEND_BUFFER_SIZE = 4 * 1024 * 1024; // 4 MB send buffer

/**
 * Checks and attempts to fix Linux kernel UDP buffer size limits.
 * 
 * On Linux, socket buffer sizes are clamped by kernel parameters:
 *   - /proc/sys/net/core/rmem_max (receive buffer max)
 *   - /proc/sys/net/core/wmem_max (send buffer max)
 * 
 * If these limits are below UDP_RECV_BUFFER_SIZE / UDP_SEND_BUFFER_SIZE,
 * the function attempts to update them (requires root privileges).
 * If update fails, it logs instructions for manual configuration.
 * 
 * Called from server.js after initialization to ensure global.Colors is available.
 * Non-Linux platforms are silently skipped.
 */
function checkAndFixKernelBuffers() {
    if (os.platform() !== 'linux') return;

    const RMEM_MAX_PATH = '/proc/sys/net/core/rmem_max';
    const WMEM_MAX_PATH = '/proc/sys/net/core/wmem_max';

    try {
        const rmemMax = parseInt(fs.readFileSync(RMEM_MAX_PATH, 'utf8').trim(), 10);
        const wmemMax = parseInt(fs.readFileSync(WMEM_MAX_PATH, 'utf8').trim(), 10);

        const issues = [];
        if (rmemMax < UDP_RECV_BUFFER_SIZE) {
            issues.push({ param: 'rmem_max', current: rmemMax, required: UDP_RECV_BUFFER_SIZE, path: RMEM_MAX_PATH });
        }
        if (wmemMax < UDP_SEND_BUFFER_SIZE) {
            issues.push({ param: 'wmem_max', current: wmemMax, required: UDP_SEND_BUFFER_SIZE, path: WMEM_MAX_PATH });
        }

        if (issues.length === 0) {
            console.log('UDP buffers ' + global.Colors.BSuccess + 'OK' + global.Colors.Reset + ': rmem_max=' + global.Colors.BSuccess + rmemMax + global.Colors.Reset + ', wmem_max=' + global.Colors.BSuccess + wmemMax + global.Colors.Reset);
            return;
        }

        console.log('==================================');
        console.log(global.Colors.FgYellow + 'UDP BUFFER WARNING' + global.Colors.Reset + ': Kernel limits are below requested socket buffer sizes.');
        console.log('Socket buffers will be clamped, potentially causing packet drops under load.');
        console.log('----------------------------------');

        for (const issue of issues) {
            console.log('   ' + issue.param + ': current=' + global.Colors.Error + issue.current + global.Colors.Reset + ', required=' + global.Colors.BSuccess + issue.required + global.Colors.Reset);

            try {
                fs.writeFileSync(issue.path, String(issue.required));
                console.log('   ' + global.Colors.BSuccess + 'Updated ' + issue.param + ' to ' + issue.required + global.Colors.Reset);
            } catch (writeErr) {
                if (writeErr.code === 'EACCES' || writeErr.code === 'EPERM') {
                    console.log('   ' + global.Colors.Error + 'Cannot update ' + issue.param + ' (requires root privileges)' + global.Colors.Reset);
                } else {
                    console.log('   ' + global.Colors.Error + 'Failed to update ' + issue.param + ': ' + writeErr.message + global.Colors.Reset);
                }
            }
        }

        console.log('----------------------------------');
        console.log('To fix permanently, run as root:');
        console.log(global.Colors.Bright + '   echo ' + UDP_RECV_BUFFER_SIZE + ' > ' + RMEM_MAX_PATH + global.Colors.Reset);
        console.log(global.Colors.Bright + '   echo ' + UDP_SEND_BUFFER_SIZE + ' > ' + WMEM_MAX_PATH + global.Colors.Reset);
        console.log('Or add to /etc/sysctl.conf:');
        console.log(global.Colors.Bright + '   net.core.rmem_max = ' + UDP_RECV_BUFFER_SIZE + global.Colors.Reset);
        console.log(global.Colors.Bright + '   net.core.wmem_max = ' + UDP_SEND_BUFFER_SIZE + global.Colors.Reset);
        console.log('==================================');

    } catch (err) {
        console.log(global.Colors.Error + 'UDP buffer check failed: ' + err.message + global.Colors.Reset);
    }
}

class udp_socket {
    constructor(host, port, func, parent) {
        this._isReady = false;
        this.parent = parent;
        this._ready_counter = 0;
        this._caller_port = null;
        this._caller_ip = null;
        this._server = null;
        this._host = host;
        this._port = port;
        this._onMessageReceived = func;
        this._last_access_time = 0;
        this._server = dgram.createSocket({
            type: 'udp4',
            recvBufferSize: UDP_RECV_BUFFER_SIZE,
            sendBufferSize: UDP_SEND_BUFFER_SIZE
        });

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
        this._server.send(message, this._caller_port, this._caller_ip);
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

        this._udp_socket1 = new udp_socket(host1, port1, this.onSocket1Receive.bind(this), this);
        this._udp_socket2 = new udp_socket(host2, port2, this.onSocket2Receive.bind(this), this);
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

    onSocket1Receive(message, Me) {
        Me._udp_socket2.sendMessage(message);
    }

    onSocket2Receive(message, Me) {
        Me._udp_socket1.sendMessage(message);
    }
}

const m_activeUdpProxy = {};
let m_reaperInterval = null;

function startReaper() {
    if (m_reaperInterval) return;
    
    m_reaperInterval = setInterval(() => {
        const now = Date.now();
        const names = Object.keys(m_activeUdpProxy);
        
        for (const name of names) {
            const entry = m_activeUdpProxy[name];
            if (!entry || !entry.m_udpproxy) continue;
            
            const lastAccess = Math.max(
                entry.last_access || 0,
                entry.m_udpproxy._udp_socket1?.getLastAccessTime() || 0,
                entry.m_udpproxy._udp_socket2?.getLastAccessTime() || 0
            );
            
            if (now - lastAccess > PROXY_IDLE_TIMEOUT_MS) {
                console.log(`Reaper: Closing idle UDP proxy '${name}' (idle for ${Math.round((now - lastAccess) / 1000)}s)`);
                entry.m_udpproxy.close();
                delete m_activeUdpProxy[name];
            }
        }
        
        if (Object.keys(m_activeUdpProxy).length === 0) {
            stopReaper();
        }
    }, REAPER_INTERVAL_MS);
    
    m_reaperInterval.unref();
}

function stopReaper() {
    if (m_reaperInterval) {
        clearInterval(m_reaperInterval);
        m_reaperInterval = null;
    }
}

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
            startReaper();
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
    closeUDPSocket,
    checkAndFixKernelBuffers
};