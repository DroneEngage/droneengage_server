const WebSocket = require('ws');
const c_ChatServer = require("../chat_server/js_andruav_chat_server");
const c_s2s_auth = require("../js_s2s_auth.js");

class ChildCommServer {
  constructor(parentHost, parentPort) {
    if (ChildCommServer.instance) {
      return ChildCommServer.instance;
    }

    this.m_callbackOnMessage = null;
    this.m_parentHost = parentHost;
    this.m_parentPort = parentPort;
    this.parentWs = null;
    ChildCommServer.instance = this;
    this.connectToParent(); // Connect immediately
  }

  static getInstance(parentHost, parentPort) {
    if (!ChildCommServer.instance) {
      ChildCommServer.instance = new ChildCommServer(parentHost, parentPort);
    }
    return ChildCommServer.instance;
  }

  connectToParent() {
    
    // Optional: Add headers for authentication or other purposes
    const headers = {
      // 'Authorization': 'Bearer your-token',
      // 'Custom-Header': 'value',
    };

    const options = {
      headers: headers,
      rejectUnauthorized: false, // Be very cautious with this in production!
    };
  

  const parentUrl = `wss://${this.m_parentHost}:${this.m_parentPort}`; // Construct the URL
    this.parentWs = new WebSocket(parentUrl
      ,options
    );

    this.parentWs.on('open', () => {
      console.log(`Child connected to parent server at ${parentUrl}.`);
    });

    this.parentWs.on('message', (message) => {
      this.onReceive(message);
    });

    this.parentWs.on('close', (code, reason) => {
      console.log(`Child disconnected from parent server: ${code} - ${reason}`);
      this.parentWs = null;
      // Reconnect logic can be added here.
      setTimeout(() => this.connectToParent(), 10000);
    });

    this.parentWs.on('error', (error) => {
      console.error(`WebSocket error from parent:`, error);
    });
  }

  onReceive(message) {
    try {
        // Answer the parent (super server) S2S challenge by signing the nonce.
        if (c_s2s_auth.fn_isEnabled() === true) {
            const c_env = c_s2s_auth.fn_parseEnvelope(message);
            if (c_env != null) {
                if ((c_env.s2s_auth === c_s2s_auth.CONST_S2S_AUTH_CHALLENGE) && (this.parentWs != null)) {
                    this.parentWs.send(c_s2s_auth.fn_buildResponse(c_env.nonce, global.m_serverconfig.m_configuration.server_id));
                }
                return;
            }
        }

        console.log (`CHILD RX: ${message}`);
                let v_isBinary = false;
                if (typeof (message) !== 'string') {
                        v_isBinary = true;
                }
                c_ChatServer.fn_parseExternalMessage( message, v_isBinary, this.parentWs);
      
    } catch (error) {
      console.error(`Error parsing message from parent:`, error);
    }
  }

  // Relay metadata (_path/_gid/_aid) is injected once upstream in chat_server.forwardMessage,
  // so no parsing is needed here.
  forwardMessage(message, p_isBinary) {
    if (this.parentWs && this.parentWs.readyState === WebSocket.OPEN) {
      this.parentWs.send(message, { binary: p_isBinary });
    } else {
      console.error("Parent WebSocket is not connected. Cannot forward message.");
    }
  }

  isSocketConnected() {
    return this.parentWs && this.parentWs.readyState === WebSocket.OPEN;
  }
}

module.exports = ChildCommServer;