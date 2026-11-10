"use strict";

const c_dumpError = require("../../comm_server/js_dumperror.js");
const fs = require('fs').promises; // Use promises for async file I/O
const path = require('path');
const zmq = require('zeromq');
const mavlink = require("./mavlink.js");
require('dotenv').config({ path: path.resolve(__dirname, '.env') }); // Explicit .env path


const EVT_ID = 1;
const EVT_FLYING = 2;
const EVT_GPS = 3;
const EVT_GPS_MOB = 4;
const EVT_IMG = 5;
const EVT_GCS = 6;
const EVT_SIGNAL = 7;
const EVT_VIDEO_ON = 8;
const EVT_VIDEO_OFF = 9;

class CommunicatorServer {
  constructor() {
    // Configuration
    this.ZMQ_PUB_ADDRESS = process.env.PLG_CMD_PRC_ZMQ_PUB_ADDRESS || 'tcp://127.0.0.1:5555';
    this.DOWNLOADS_DIR = process.env.PLG_CMD_PRC_DOWNLOADS_DIR || './downloads';
    this.LOG_DIR = process.env.PLG_CMD_PRC_LOG_DIR || './logs';
    this.PLG_CMD_PRC_INTERVAL_SECONDS = process.env.PLG_CMD_PRC_INTERVAL_SECONDS || '300';
    this._on = process.env.PLG_CMD_PRC_ENABLED === 'true' || false;
    // In-memory Map for rate-limiting logs
    this.accountLogTimestamps = new Map();
    
    console.log ("this.DOWNLOADS_DIR:", this.DOWNLOADS_DIR);
    console.log ("this.LOG_DIR:", this.LOG_DIR);
    console.log ("this.PLG_CMD_PRC_INTERVAL_SECONDS:", this.PLG_CMD_PRC_INTERVAL_SECONDS);
    if (this._on === true)
    {
      console.log ("PLUGIN COMMAND PROCESS IS ENABLED");
    }
    else
    {
      console.log ("PLUGIN COMMAND PROCESS IS DISABLED");
    }
    // Initialize ZMQ Publisher
    this.publisher = new zmq.Publisher();
    this.messageQueue = []; // Queue for batch publishing
    this.batchInterval = 100; // Batch messages every 100ms
    this.isPublishing = false;
    
    // Precompiled regex for GPS parsing
    this.gpsRegex = /lng:([-\d.]+)\s+lat:([-\d.]+)/;
    
    // Ensure downloads directory exists
    this.initDirectories();
    // Bind ZMQ publisher
    this.initPublisher();
    // Start batch publishing
    this.startBatchPublish();
  }

  async initDirectories() {
    try {
      await fs.mkdir(this.DOWNLOADS_DIR, { recursive: true });
      console.log(`Created directory: ${this.DOWNLOADS_DIR}`);
    } catch (err) {
      console.error(`Failed to create directory: ${err.message}`);
    }
  }

  async initPublisher() {
    try {
      await this.publisher.bind(this.ZMQ_PUB_ADDRESS);
      console.log(`ZMQ Publisher bound to ${this.ZMQ_PUB_ADDRESS}`);
    } catch (err) {
      console.error(`Failed to bind ZMQ Publisher: ${err.message}`);
      // Retry after 5 seconds
      setTimeout(() => this.initPublisher(), 15000);
    }
  }

  async batchPublish() {
    if (this.isPublishing || this.messageQueue.length === 0) return;
    this.isPublishing = true;
    
    try {
      const messages = [...this.messageQueue];
      this.messageQueue = [];
      for (const text of messages) {
        await this.publisher.send(['', JSON.stringify({ text })]);
        console.log(`Published log entry: ${text}`);
      }
    } catch (err) {
      console.error(`Failed to publish batch: ${err.message}`);
      // Re-queue messages for retry
      this.messageQueue.push(...messages);
    } finally {
      this.isPublishing = false;
    }
  }

  startBatchPublish() {
    setInterval(() => this.batchPublish(), this.batchInterval);
  }

  async appendToFile(fileName, buffer) {
    try {
      await fs.appendFile(fileName, buffer);
    } catch (err) {
      console.error(`Error appending to file ${fileName}: ${err.message}`);
    }
  }

  async parseBinaryDataChunk(ws, message, jmsg, binaryDataIndex) {
    try {
      let desc = '';
      let buffer = [];
      let index = binaryDataIndex + 1;
      
      // Parse description until null byte
      while (index < message.length && message[index] !== 0) {
        desc += String.fromCharCode(message[index++]);
      }
      
      // Parse remaining buffer
      buffer = message.slice(index + 1);
      const bufferData = Buffer.from(buffer);
      const fileName = path.join(this.DOWNLOADS_DIR, `img_${jmsg.i}`);
      
      try {
        const parsedDesc = JSON.parse(desc);
        if (parsedDesc.n === 0) {
          try {
            await fs.unlink(fileName);
          } catch (err) {
            console.error(`Error unlinking file ${fileName}: ${err.message}`);
          }
        }
        await this.appendToFile(fileName, bufferData);
      } catch (err) {
        console.error(`Error parsing description or writing file: ${err.message}`);
      }
    } catch (err) {
      console.error(`Error in parseBinaryDataChunk: ${err.message}`);
    }
  }

  async parseImage(ws, message, jmsg, binaryDataIndex) {
    try {
      let buffer = [];
      let desc = jmsg.ms;
      let ext = '.jpg'; // Default extension
      let loc = '_nloc_';
      
      if (!jmsg.hasOwnProperty('ms')) {
        let descStr = '';
        let index = binaryDataIndex + 1;
        while (index < message.length && message[index] !== 0) {
          descStr += String.fromCharCode(message[index++]);
        }
        buffer = message.slice(index + 1);
        desc = JSON.parse(descStr);
        ext = desc.e || '.jpg';
        if (ws.m__status?.m_lng != null) {
          loc = `_loc_${ws.m__status.m_lng}_${ws.m__status.m_lat}_`;
        }
      } else {
        buffer = message.slice(binaryDataIndex + 1);
        loc = `_loc_lng_${desc.lng}_lat_${desc.lat}_alt${desc.alt}_`;
        ext = '.bmp';
      }
      
      const bufferData = Buffer.from(buffer);
      const fileName = path.join(
        this.DOWNLOADS_DIR,
        `img_${jmsg.sd}_psid_${ws.m_loginRequest.m_accountID}${loc}_date_${new Date().toISOString()}${ext}`
      );
      
      await this.appendToFile(fileName, bufferData);
    } catch (err) {
      console.error(`Error in parseImage: ${err.message}`);
    }
  }

  parseMavlink(ws, message, jmsg, binaryDataIndex) {
    try {
      let desc = '';
      let index = binaryDataIndex;
      while (index < message.length && message[index] !== 0) {
        desc += String.fromCharCode(message[index++]);
      }
      const buffer = Buffer.from(message.slice(index + 1));
      const mavlinkProcessor = new mavlink.MAVLink20Processor(null, 0, 0);
      const mavlinkMessages = mavlinkProcessor.parseBuffer(buffer);
      
      for (const msg of mavlinkMessages) {
        if (msg._id === -1) continue;
        
        switch (msg._header.msgId) {
          case mavlink20.MAVLINK_MSG_ID_GPS_RAW_INT:
            ws.m__status.m_speed = msg.vel / 100.0;
            break;
          
          case mavlink20.MAVLINK_MSG_ID_GLOBAL_POSITION_INT:
            const lng = msg.lon * 0.0000001;
            const lat = msg.lat * 0.0000001;
            if (lng !== 0.0) {
              ws.m__status.m_lng = lng;
              ws.m__status.m_lat = lat;
            }
            ws.m__status.m_alt = msg.relative_alt * 0.001;
            
            
            if (ws.m__status?.m_lng === 0 && lng !== 0.0) {
              ws.m__status.m_lng = lng;
              ws.m__status.m_lat = lat;
              ws.m__status.m_gps = true;
            }
            
            this.writeToLogFile(
              ws.m_loginRequest.m_accountID,
              "important",
              "gps",
              `lng:${ws.m__status.m_lng} lat:${ws.m__status.m_lat} alt:${ws.m__status.m_alt} spd:${ws.m__status.m_speed}`,
              EVT_GPS
            );
            break;
        }
      }
    } catch (err) {
      console.error(`Error in parseMavlink: ${err.message}`);
    }
  }

  async writeToLogFile(p_ID, p_severity, p_subject, p_description, p_type) {
    const now = Date.now();
    const intervalMs = (this.PLG_CMD_PRC_INTERVAL_SECONDS || 5) * 1000;
    const logKey = `${p_ID}:${p_type}`;
    
    const lastLogTime = this.accountLogTimestamps.get(logKey);
    if (!lastLogTime || (now - lastLogTime) >= intervalMs) {
      this.accountLogTimestamps.set(logKey, now);
      
      let text;
      if ((p_type === EVT_GPS) || (p_type === EVT_GPS_MOB)) {
        const match = this.gpsRegex.exec(p_description);
        if (match) {
          const [, lng, lat] = match;
          const gpsLink = `<a href="https://www.google.com/maps?q=${lat},${lng}">lng:${lng} lat:${lat}</a>`;
          text = `${p_ID} | ${p_severity} | sub:${p_subject} | desc:${gpsLink}${p_description.slice(match[0].length)} | Time:${new Date(now).toISOString()}`;
        } else {
          text = `${p_ID} | ${p_severity} | Sub:${p_subject} | desc:${p_description} | Time:${new Date(now).toISOString()}`;
        }
      } else {
        text = `${p_ID} | ${p_severity} | sub:${p_subject} | desc:${p_description} | Time:${new Date(now).toISOString()}`;
      }
      
      this.messageQueue.push(text);
    }
  }

  async processCommand(ws, message, jmsg, binaryDataIndex, callback) {
    try {
      if (this._on !== true) return ;
      if (jmsg?.en === true) return;

      ws.m__status = ws.m__status || {};
      const id = parseInt(jmsg.mt);

      switch (id) {
        case 2:
          await this.parseBinaryDataChunk(ws, message, jmsg, binaryDataIndex);
          if (callback) {
            callback(true, { mt: 1005, ms: { C: 2 } });
          }
          break;

        case 1006:
          await this.parseImage(ws, message, jmsg, binaryDataIndex);
          await this.writeToLogFile(
            ws.m_loginRequest.m_accountID,
            "important",
            "IMAGE",
            ws.m_loginRequest.m_accountID,
            EVT_IMG
          );
          break;

        case 1004:
          let internalMsg = jmsg.ms;
          if (typeof internalMsg === 'string') {
            internalMsg = JSON.parse(internalMsg);
          }
          
          if (internalMsg.GS === true)
          {
            await this.writeToLogFile(
              ws.m_loginRequest.m_accountID,
              "info",
              "user",
              'GCS',
              EVT_GCS
            );
            return ;
          }
          if (internalMsg.hasOwnProperty('dv'))
          {
            ws.m__status.m_de = true;
          }
          else
          {
            ws.m__status.m_de = false;
          }
          
          
          ws.m__status.m_actorType = 'd';
          ws.m__status.m_useFCB = internalMsg.FI ?? false;
          
          

          if (internalMsg.FL) {
            await this.writeToLogFile(
              ws.m_loginRequest.m_accountID,
              "important",
              "In Sky",
              `Flying ${ws.m_loginRequest.m_accountID}`,
              EVT_FLYING
              );
            ws.m__status.m_isFlyingDone = true;
          }
          else
          {
            await this.writeToLogFile(
            ws.m_loginRequest.m_accountID,
            "info",
            "unit",
            `${ws.m__status.m_de===true?'DE':'Andruav'}`,
            EVT_ID
           );
          }

          ws.m__status.m_lastAccessTime = new Date();
          break;

        case 1002:
          let internalMsg2 = jmsg.ms;
          if (typeof internalMsg2 === 'string') {
            internalMsg2 = JSON.parse(internalMsg2);
          }

          if (internalMsg2.p) {
            const lng = parseFloat(internalMsg2.ln);
            const lat = parseFloat(internalMsg2.la);
            if (lng !== 0.0) {
              ws.m__status.m_lng = lng;
              ws.m__status.m_lat = lat;
            }
            ws.m__status.m_speed = parseFloat(internalMsg2.s);
            ws.m__status.m_alt = parseFloat(internalMsg2.a);

            if (ws.m__status.m_lng === 0 && lng !== 0.0) {
              ws.m__status.m_lng = lng;
              ws.m__status.m_lat = lat;
              ws.m__status.m_gps = true;
            }
          } else {
            ws.m__status.m_speed = 0.0;
            ws.m__status.m_alt = 0.0;
          }

          await this.writeToLogFile(
            ws.m_loginRequest.m_accountID,
            "info",
            "gps-m",
            `lng:${ws.m__status.m_lng} lat:${ws.m__status.m_lat} spd:${ws.m__status.m_alt}`,
            EVT_GPS_MOB
          );
          break;

        case 6502:
          this.parseMavlink(ws, message, jmsg, binaryDataIndex);
          break;

        case 1059: // CONST_TYPE_AndruavMessage_CommSignalsStatus
          let signalMsg = jmsg.ms;
          if (typeof signalMsg === 'string') {
            signalMsg = JSON.parse(signalMsg);
          }

          ws.m__status.m_mobile = true;
          ws.m__status.m_mobileSignalLevel = signalMsg.r ?? 0;
          ws.m__status.m_mobileNetworkType = signalMsg.s ?? 0;
          if (signalMsg.op != null) ws.m__status.m_operatorName = signalMsg.op;
          if (signalMsg.c != null)  ws.m__status.m_countryIso = signalMsg.c;
          if (signalMsg.ds != null) ws.m__status.m_dataState = signalMsg.ds;

          await this.writeToLogFile(
            ws.m_loginRequest.m_accountID,
            "info",
            "signal",
            `dbm:${signalMsg.r ?? 0} netType:${signalMsg.s ?? 0} op:${signalMsg.op ?? ''} country:${signalMsg.c ?? ''} dataState:${signalMsg.ds ?? 0}`,
            EVT_SIGNAL
          );
          break;

        case 1021:
          let signalMsg2 = jmsg.ms;
          if (typeof signalMsg2 === 'string') {
            signalMsg2 = JSON.parse(signalMsg2);
          }

          // ms.w.packet, or the raw payload when w is absent
          const w = signalMsg2.w || signalMsg2;
          const packet = w && w.packet;
          if (packet && packet.joinme) {
            await this.writeToLogFile(
              ws.m_loginRequest.m_accountID,
              "important",
              "video",
              `video on ch:${w.channel} by:${w.number}`,
              EVT_VIDEO_ON
            );
          } else if (packet && packet.hangup) {
            await this.writeToLogFile(
              ws.m_loginRequest.m_accountID,
              "important",
              "video",
              `video off ch:${w.channel} by:${w.number}`,
              EVT_VIDEO_OFF
            );
          }
          break;
      }
    } catch (err) {
      console.error(`Error in processCommand: ${err.message}`);
    }
  }
}

// Export a singleton instance
module.exports = new CommunicatorServer();