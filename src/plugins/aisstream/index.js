"use strict";

/**
 * DroneEngage AIS Stream Plugin.
 *
 * Connects to aisstream.io WebSocket (server-side, API key stays on the server)
 * and forwards normalized AIS position reports to subscribed GCS clients via
 * the DroneEngage system message bus.
 *
 * Message protocol:
 *   GCS → Server:  CONST_TYPE_AndruavSystem_AisSub (9019)
 *       ms: { en: true/false, lat, lng, radiusKm }
 *   Server → GCS:  CONST_TYPE_AndruavSystem_AisPush (9020)
 *       ms: [ { mmsi, shipName, lat, lng, sog, cog, heading, navStatus, ts }, ... ]
 *
 * The plugin is loaded by the server's pluginManager.js. It exports
 * `processCommand` which is called for every incoming WebSocket message.
 * The plugin intercepts AIS subscribe/unsubscribe system messages and
 * ignores everything else.
 *
 * Lifecycle:
 *   - The upstream aisstream.io connection is created lazily on the first
 *     subscribe request and closed when the last subscriber is removed.
 *   - Subscribers that haven't sent a subscribe message within
 *     SUBSCRIBER_TIMEOUT_MS are automatically removed.
 *   - The upstream subscription is updated (replaced) whenever the union of
 *     subscriber bounding boxes changes.
 *
 * Auth: Mohammad Hefny
 */

const WebSocket = require('ws');
const path = require('path');

// Load plugin-local .env (git-ignored). Keeps the API key out of version
// control and out of server.config. Mirrors the command_processing plugin.
require('dotenv').config({ path: path.resolve(__dirname, '.env') });

// DroneEngage server constants (required from the host server's src directory).
// The plugin manager resolves this plugin from a path relative to the server
// root, so we can require server modules via relative paths.
// However, to keep the plugin self-contained, we define the message type
// constants here as well (they must match js_constants.js).
const CONST_TYPE_AndruavSystem_AisSub  = 9019;
const CONST_TYPE_AndruavSystem_AisPush = 9020;
const CONST_WS_MSG_ROUTING_SYSTEM      = 's';
const CONST_WS_SENDER_COMM_SERVER      = '_SYS_';

// Defaults
const AISSTREAM_URL = 'wss://stream.aisstream.io/v0/stream';
const SUBSCRIBER_TIMEOUT_MS = 120000;   // remove subscribers silent for 2 min
const RECONNECT_BASE_MS     = 2000;     // initial reconnect delay
const RECONNECT_MAX_MS      = 60000;    // max reconnect delay (with backoff)
const FLUSH_INTERVAL_MS     = 2000;     // batch AIS messages and flush every 2s
const MAX_BUFFER_SIZE       = 500;      // max messages per flush batch

// ─── Plugin state (module-level, initialized once) ──────────────────────

let _config = null;          // aisstream config resolved from .env
let _ws = null;              // upstream WebSocket to aisstream.io
let _connected = false;
let _reconnectDelay = RECONNECT_BASE_MS;
let _reconnectTimer = null;
let _subscribers = new Map(); // key: ws object → { ws, bbox, lastSeen }
let _messageBuffer = [];      // buffered AIS messages waiting to be flushed
let _flushTimer = null;
let _cleanupTimer = null;

// ─── Helpers ─────────────────────────────────────────────────────────────

/**
 * Resolve the aisstream config from the plugin-local .env (loaded above via
 * dotenv). Env vars:
 *   PLG_AISSTREAM_API_KEY  — required aisstream.io API key
 *   PLG_AISSTREAM_URL      — optional upstream WebSocket URL override
 */
function _readConfig() {
    if (_config) return _config;
    _config = {
        api_key: process.env.PLG_AISSTREAM_API_KEY || '',
        url:     process.env.PLG_AISSTREAM_URL || '',
    };
    return _config;
}

/**
 * Convert a lat/lng/radius (km) into an aisstream.io bounding box pair.
 * Returns [[latMax, lngMin], [latMin, lngMax]] (top-left, bottom-right).
 */
function _radiusToBoundingBox(lat, lng, radiusKm) {
    const dLat = radiusKm / 111.0;
    const cosLat = Math.cos((lat * Math.PI) / 180);
    const dLng = radiusKm / (111.0 * (cosLat || 1));
    return [
        [lat + dLat, lng - dLng],   // top-left corner
        [lat - dLat, lng + dLng],   // bottom-right corner
    ];
}

/**
 * Compute the union bounding box of all active subscribers.
 * Returns an array of bounding boxes (aisstream.io accepts multiple boxes)
 * or null if there are no subscribers.
 */
function _computeUnionBoundingBoxes() {
    if (_subscribers.size === 0) return null;
    const boxes = [];
    for (const sub of _subscribers.values()) {
        if (sub.bbox) {
            boxes.push(sub.bbox);
        }
    }
    return boxes.length > 0 ? boxes : null;
}

/**
 * Send a JSON message to a single WebSocket client.
 */
function _sendToClient(ws, message) {
    try {
        if (ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify(message));
        }
    } catch (e) {
        // Client may have disconnected; ignore.
    }
}

/**
 * Broadcast the flushed AIS message buffer to all active subscribers.
 */
function _flushBuffer() {
    if (_messageBuffer.length === 0) return;
    const batch = _messageBuffer;
    _messageBuffer = [];

    const msg = {
        ty: CONST_WS_MSG_ROUTING_SYSTEM,
        sd: CONST_WS_SENDER_COMM_SERVER,
        mt: CONST_TYPE_AndruavSystem_AisPush,
        ms: batch,
    };
    const serialized = JSON.stringify(msg);

    for (const sub of _subscribers.values()) {
        try {
            if (sub.ws.readyState === WebSocket.OPEN) {
                sub.ws.send(serialized);
            }
        } catch (e) {
            // Ignore individual send failures.
        }
    }
}

/**
 * Remove subscribers that haven't sent a subscribe message recently.
 */
function _cleanupStaleSubscribers() {
    const now = Date.now();
    let changed = false;
    for (const [key, sub] of _subscribers) {
        if (now - sub.lastSeen > SUBSCRIBER_TIMEOUT_MS) {
            _subscribers.delete(key);
            changed = true;
        }
    }
    if (changed) {
        _updateUpstreamSubscription();
        if (_subscribers.size === 0) {
            _disconnectUpstream();
        }
    }
}

// ─── Upstream aisstream.io connection ────────────────────────────────────

/**
 * Establish (or re-establish) the WebSocket connection to aisstream.io.
 */
function _connectUpstream() {
    const cfg = _readConfig();
    if (!cfg.api_key || cfg.api_key === '') {
        console.error('[aisstream] No api_key configured: set PLG_AISSTREAM_API_KEY in the plugin .env');
        return;
    }
    if (_ws && (_ws.readyState === WebSocket.OPEN || _ws.readyState === WebSocket.CONNECTING)) {
        return;
    }

    const url = cfg.url || AISSTREAM_URL;
    console.log(`[aisstream] Connecting to ${url} ...`);

    try {
        _ws = new WebSocket(url, { perMessageDeflate: true });
    } catch (e) {
        console.error('[aisstream] WebSocket creation failed:', e.message);
        _scheduleReconnect();
        return;
    }

    _ws.on('open', () => {
        console.log('[aisstream] Connected');
        _connected = true;
        _reconnectDelay = RECONNECT_BASE_MS;
        _updateUpstreamSubscription();
    });

    _ws.on('message', (data) => {
        _handleUpstreamMessage(data);
    });

    _ws.on('close', () => {
        console.log('[aisstream] Connection closed');
        _connected = false;
        _ws = null;
        if (_subscribers.size > 0) {
            _scheduleReconnect();
        }
    });

    _ws.on('error', (err) => {
        console.error('[aisstream] WebSocket error:', err.message);
        // 'close' will fire after 'error' and handle reconnect.
    });
}

/**
 * Close the upstream connection.
 */
function _disconnectUpstream() {
    if (_reconnectTimer) {
        clearTimeout(_reconnectTimer);
        _reconnectTimer = null;
    }
    if (_ws) {
        try { _ws.close(); } catch (e) { /* ignore */ }
        _ws = null;
    }
    _connected = false;
}

/**
 * Schedule a reconnect with exponential backoff.
 */
function _scheduleReconnect() {
    if (_reconnectTimer) return;
    const delay = _reconnectDelay;
    _reconnectDelay = Math.min(_reconnectDelay * 2, RECONNECT_MAX_MS);
    console.log(`[aisstream] Reconnecting in ${delay}ms ...`);
    _reconnectTimer = setTimeout(() => {
        _reconnectTimer = null;
        _connectUpstream();
    }, delay);
}

/**
 * Send (or replace) the upstream subscription with the current union of
 * subscriber bounding boxes.
 */
function _updateUpstreamSubscription() {
    if (!_connected || !_ws || _ws.readyState !== WebSocket.OPEN) return;

    const boxes = _computeUnionBoundingBoxes();
    if (!boxes) {
        // No subscribers — nothing to subscribe to.
        return;
    }

    const cfg = _readConfig();
    const subscription = {
        APIKey: cfg.api_key,
        BoundingBoxes: boxes,
        FilterMessageTypes: ['PositionReport', 'StandardSearchAndRescueAircraftReport'],
    };

    try {
        _ws.send(JSON.stringify(subscription));
        console.log(`[aisstream] Subscription updated: ${boxes.length} bounding box(es), ${_subscribers.size} subscriber(s)`);
    } catch (e) {
        console.error('[aisstream] Failed to send subscription:', e.message);
    }
}

/**
 * Handle an incoming aisstream.io message: normalize and buffer it.
 */
function _handleUpstreamMessage(data) {
    let parsed;
    try {
        // aisstream.io sends binary frames containing UTF-8 JSON.
        const text = data.toString('utf8');
        parsed = JSON.parse(text);
    } catch (e) {
        return;
    }

    if (!parsed || !parsed.MessageType) return;

    // Skip subscription confirmations.
    if (parsed.MessageType === 'SubscriptionConfirmation') return;

    // Extract position from the typed message.
    const normalized = _normalizeAisMessage(parsed);
    if (!normalized) return;

    _messageBuffer.push(normalized);

    // Flush immediately if the buffer is full.
    if (_messageBuffer.length >= MAX_BUFFER_SIZE) {
        _flushBuffer();
    }
}

/**
 * Convert an aisstream.io message envelope into a normalized AIS position
 * object suitable for the webclient's AisStreamProvider.
 *
 * Supported message types:
 *   - PositionReport (types 1-3)
 *   - StandardSearchAndRescueAircraftReport (type 9 — aircraft!)
 *   - StandardClassBPositionReport / ExtendedClassBPositionReport (type 18/19)
 *   - LongRangeAisBroadcastMessage (type 27 — reduced position)
 */
function _normalizeAisMessage(envelope) {
    const meta = envelope.MetaData || {};
    const msgType = envelope.MessageType;
    const msgBody = envelope.Message && envelope.Message[msgType];

    if (!msgBody) return null;

    // Position fields vary by message type, but MetaData always has lat/lng.
    const lat = meta.Latitude != null ? meta.Latitude : (msgBody.Latitude != null ? msgBody.Latitude : null);
    const lng = meta.Longitude != null ? meta.Longitude : (msgBody.Longitude != null ? msgBody.Longitude : null);

    if (lat == null || lng == null) return null;

    return {
        mmsi: meta.MMSI != null ? meta.MMSI : (msgBody.UserID != null ? msgBody.UserID : null),
        shipName: meta.ShipName || '',
        lat: lat,
        lng: lng,
        sog: msgBody.Sog != null ? msgBody.Sog : null,          // knots
        cog: msgBody.Cog != null ? msgBody.Cog : null,          // degrees
        heading: msgBody.TrueHeading != null ? msgBody.TrueHeading : null,  // degrees
        navStatus: msgBody.NavigationalStatus != null ? msgBody.NavigationalStatus : null,
        msgType: msgType,
        ts: Date.now(),
    };
}

// ─── Plugin interface ────────────────────────────────────────────────────

/**
 * Start the flush and cleanup timers (called once on module load).
 */
function _startTimers() {
    if (!_flushTimer) {
        _flushTimer = setInterval(_flushBuffer, FLUSH_INTERVAL_MS);
    }
    if (!_cleanupTimer) {
        _cleanupTimer = setInterval(_cleanupStaleSubscribers, 30000);
    }
}

// Start timers immediately on module load.
_startTimers();

/**
 * Plugin command processor — called by the server's pluginManager for every
 * incoming WebSocket message. Intercepts AIS subscribe/unsubscribe system
 * messages and ignores everything else.
 *
 * @param {WebSocket} p_ws       the sender's WebSocket
 * @param {*} p_message          raw message (Buffer or string)
 * @param {object} v_jmsg        parsed JSON message object
 * @param {number} nullIndex     index of null byte in binary messages
 * @param {function} callback    callback (currently unused)
 */
async function processCommand(p_ws, p_message, v_jmsg, nullIndex, callback) {
    // Only intercept system messages.
    if (!v_jmsg) return;
    if (v_jmsg.ty !== CONST_WS_MSG_ROUTING_SYSTEM) return;
    if (v_jmsg.mt !== CONST_TYPE_AndruavSystem_AisSub) return;

    const ms = v_jmsg.ms || {};
    const enable = ms.en === true;

    if (!enable) {
        // Unsubscribe
        _subscribers.delete(p_ws);
        console.log(`[aisstream] Unsubscribe: ${p_ws.name || 'unknown'} (${_subscribers.size} remaining)`);
        if (_subscribers.size === 0) {
            _disconnectUpstream();
        } else {
            _updateUpstreamSubscription();
        }
        return;
    }

    // Subscribe / update subscription
    const lat = Number(ms.lat);
    const lng = Number(ms.lng);
    const radiusKm = Number(ms.radiusKm);

    if (!isFinite(lat) || !isFinite(lng) || !isFinite(radiusKm) || radiusKm <= 0) {
        console.warn(`[aisstream] Invalid subscribe area: lat=${ms.lat} lng=${ms.lng} radiusKm=${ms.radiusKm}`);
        return;
    }

    const bbox = _radiusToBoundingBox(lat, lng, radiusKm);
    _subscribers.set(p_ws, {
        ws: p_ws,
        bbox: bbox,
        lastSeen: Date.now(),
    });

    console.log(`[aisstream] Subscribe: ${p_ws.name || 'unknown'} lat=${lat} lng=${lng} r=${radiusKm}km (${_subscribers.size} total)`);

    // Ensure upstream connection is active.
    if (!_connected) {
        _connectUpstream();
    } else {
        _updateUpstreamSubscription();
    }
}

module.exports = {
    processCommand,
};
