const WebSocket = require('ws');
const https = require('https');
const fs = require('fs');
const c_ChatServer = require("../chat_server/js_andruav_chat_server");
const c_s2s_auth = require("../js_s2s_auth.js");

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

    const v_keyPath = v_path.isAbsolute(global.m_serverconfig.m_configuration.ssl_key_file.toString()) ? global.m_serverconfig.m_configuration.ssl_key_file.toString() : v_path.join(__dirname, "../" + global.m_serverconfig.m_configuration.ssl_key_file.toString());
    const v_certPath = v_path.isAbsolute(global.m_serverconfig.m_configuration.ssl_cert_file.toString()) ? global.m_serverconfig.m_configuration.ssl_cert_file.toString() : v_path.join(__dirname, "../" + global.m_serverconfig.m_configuration.ssl_cert_file.toString());
    const options = {
      key: fs.readFileSync(v_keyPath),
      cert: fs.readFileSync(v_certPath)
    };

    const server = https.createServer(options);
    this.wss = new WebSocket.Server({ server });

    this.wss.on('connection', (child_ws, req) => {
      const clientKey = `${req.socket.remoteAddress}:${req.socket.remotePort}`; // IP:port

      this.clientData.set(clientKey, { child_ws, data: {} }); // Store ws and data

      console.log(`New Child Communication Server Connected from ${global.Colors.BSuccess}${clientKey}${global.Colors.Reset}`);

      // S2S authentication: a child server must prove ownership of the private key
      // before any of its relayed traffic is trusted or forwarded.
      child_ws.m_s2s_authed = false;
      let v_authTimer = null;
      if (c_s2s_auth.fn_isEnabled() === true) {
        const c_nonce = c_s2s_auth.fn_generateNonce();
        child_ws.m_s2s_nonce = c_nonce;
        child_ws.send(c_s2s_auth.fn_buildChallenge(c_nonce));

        v_authTimer = setTimeout(() => {
          if (child_ws.m_s2s_authed !== true) {
            console.log(`${global.Colors.Error}[ATTENTION!!] Child ${clientKey} failed S2S handshake (timeout)${global.Colors.Reset}`);
            child_ws.terminate();
          }
        }, c_s2s_auth.CONST_S2S_AUTH_HANDSHAKE_TIMEOUT);
      }
      else {
        child_ws.m_s2s_authed = true;
      }

      child_ws.on('close', () => {
        this.clientData.delete(clientKey); // Remove client on close
        if (v_authTimer != null) clearTimeout(v_authTimer);
        console.log(`Connection closed from ${clientKey}`);
      });

      child_ws.on('message', (p_msg) => {
        try {
        // Handshake gate: a child must authenticate before its messages are relayed.
        if (child_ws.m_s2s_authed !== true) {
          const c_env = c_s2s_auth.fn_parseEnvelope(p_msg);
          if ((c_env != null)
            && (c_env.s2s_auth === c_s2s_auth.CONST_S2S_AUTH_RESPONSE)
            && (c_env.id != null)
            && (c_s2s_auth.fn_verify(child_ws.m_s2s_nonce, c_env.sig, c_env.id) === true)) {
            child_ws.m_s2s_authed = true;
            child_ws.m_s2s_server_id = c_env.id;
            if (v_authTimer != null) clearTimeout(v_authTimer);
            console.log(`${global.Colors.Success}[OK] Child ${clientKey} passed S2S handshake [${c_env.id}]${global.Colors.Reset}`);
          }
          else {
            console.log(`${global.Colors.Error}[ATTENTION!!] Child ${clientKey} failed S2S handshake (bad signature or missing id)${global.Colors.Reset}`);
            child_ws.terminate();
          }
          return;
        }

        console.log (`SUPER SRV RX:: ${p_msg}`);
        let v_isBinary = false;
        if (typeof (p_msg) !== 'string') {
                v_isBinary = true;
        }
        c_ChatServer.fn_parseExternalMessage(p_msg, v_isBinary, child_ws);
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


  // forward message to all clients, optionally excluding a specific child.
  // Relay metadata (_path/_gid/_aid) is injected once upstream in chat_server.forwardMessage,
  // so no parsing is needed here.
  forwardMessage(message, p_isBinary, exclude_ws = null) {
    for (const { child_ws } of this.clientData.values()) {
      if (child_ws && child_ws.readyState === WebSocket.OPEN && child_ws.m_s2s_authed === true) {
        // Skip the excluded child (sender) to prevent sending message back
        if (exclude_ws && child_ws === exclude_ws) {
          continue;
        }
        child_ws.send(message, { binary: p_isBinary });
      }
    }
  }

  getActiveClients() {
    return Array.from(this.clientData.keys());
  }
}

module.exports = ParentCommServer;