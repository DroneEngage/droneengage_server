const WebSocket = require('ws');
const https = require('https');
const fs = require('fs');
const c_ChatServer = require("../chat_server/js_andruav_chat_server");

class ParentCommServer {
  constructor(host, port) {
    if (ParentCommServer.instance) {
      return ParentCommServer.instance;
    }

    this.m_port = port;
    this.m_host = host;
    this.wss = null;
    this.clientData = new Map(); // Store client data (IP:port -> {ws, data})

    ParentCommServer.instance = this;

    this.restart();
  }

  static getInstance(host, port) {
    if (!ParentCommServer.instance) {
      ParentCommServer.instance = new ParentCommServer(host, port);
    }
    return ParentCommServer.instance;
  }

  restart() {
    if (this.wss) {
      this.wss.close(() => {
        console.log('Super Server closed on ' + global.Colors.BSuccess + this.m_host + ':' + this.m_port + global.Colors.Reset);
        this._startServer(this.m_port, this.m_host);
      });
    } else {
      this._startServer(this.m_port, this.m_host);
    }
  }

  _startServer(port, host) {
    this.m_port = port;
    this.m_host = host;

    console.log("Super Server Starting");
    console.log("listening on ip: " + global.Colors.BSuccess + host + global.Colors.Reset + " port: " + global.Colors.BSuccess + port + global.Colors.Reset);
    
    const v_path = require('path');

    const options = {
      key: fs.readFileSync(v_path.join(__dirname, "../" + global.m_serverconfig.m_configuration.ssl_key_file.toString())),
      cert: fs.readFileSync(v_path.join(__dirname, "../" + global.m_serverconfig.m_configuration.ssl_cert_file.toString()))
    };

    const server = https.createServer(options);
    this.wss = new WebSocket.Server({ server });

    this.wss.on('connection', (child_ws, req) => {
      const clientKey = `${req.socket.remoteAddress}:${req.socket.remotePort}`; // IP:port

      this.clientData.set(clientKey, { child_ws, data: {} }); // Store ws and data

      console.log(`New Child Communication Server Connected from ${global.Colors.BSuccess}${clientKey}${global.Colors.Reset}`);
      

      child_ws.on('close', () => {
        this.clientData.delete(clientKey); // Remove client on close
        console.log(`Connection closed from ${clientKey}`);
      });

      child_ws.on('message', (p_msg) => {
        try {
        console.log (`SUPER SRV RX:: ${p_msg}`);
        let v_isBinary = false;
        if (typeof (p_msg) !== 'string') {
                v_isBinary = true;
        }
        c_ChatServer.fn_parseExternalMessage(p_msg, v_isBinary);
      } catch (error) {
        console.error(`Error parsing message from parent:`, error);
      }
      });
  
      

    });


    this.wss.on('error', (error) => {
      console.log("Error: ", error);
    });

    server.listen(this.m_port, this.m_host, () => {
      console.log(`${global.Colors.BSuccess}[OK] Super Server has Started at ${this.m_host}:${this.m_port}${global.Colors.Reset}`);
    });
  }


  // forward message to all clients
  forwardMessage(message, p_isBinary) {
    for (const { child_ws } of this.clientData.values()) {
      if (child_ws && child_ws.readyState === WebSocket.OPEN) {
        child_ws.send(message, { binary: p_isBinary });
      }
    }
  }

  getActiveClients() {
    return Array.from(this.clientData.keys());
  }
}

module.exports = ParentCommServer;