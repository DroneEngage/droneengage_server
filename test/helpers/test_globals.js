"use strict";

/**
 * Test harness helpers.
 *
 * The comm-server code reads runtime state from `global.*` (server config and
 * the relay-channel singletons). These helpers install fake globals and capture
 * messages that would be forwarded to parent/child relay servers, so the pure
 * logic can be exercised without a live server or database.
 *
 * Usage:
 *   const { installFakeGlobals, restoreGlobals } = require('../helpers/test_globals.js');
 *   beforeEach(() => harness = installFakeGlobals({ server_id: 'SrvA', enable_super_server: true }));
 *   afterEach(() => restoreGlobals());
 */

const GLOBAL_KEYS = [
    'm_serverconfig',
    'm_andruav_channel_parent_server',
    'm_andruav_channel_child_socket',
    'm_logger',
    'Colors'
];

let v_saved = null;


/**
 * Installs fake globals and returns a harness object whose `parentSent` /
 * `childSent` arrays capture forwarded messages.
 *
 * @param {object} p_config overrides merged into m_serverconfig.m_configuration
 * @returns {{config:object, parentSent:Array, childSent:Array}}
 */
function installFakeGlobals(p_config) {
    // Save existing globals exactly once so nested installs don't lose originals.
    if (v_saved === null) {
        v_saved = {};
        for (const c_key of GLOBAL_KEYS) {
            v_saved[c_key] = Object.prototype.hasOwnProperty.call(global, c_key)
                ? { present: true, value: global[c_key] }
                : { present: false };
        }
    }

    const c_harness = {
        config: Object.assign({
            server_id: 'SrvTest',
            enable_super_server: false,
            enable_persistant_relay: false
        }, p_config || {}),
        parentSent: [],
        childSent: []
    };

    global.m_serverconfig = { m_configuration: c_harness.config };

    global.m_andruav_channel_parent_server = {
        getInstance: function () {
            return {
                forwardMessage: function (p_message, p_isBinary, p_source_ws) {
                    c_harness.parentSent.push({ message: p_message, isBinary: p_isBinary, sourceWs: p_source_ws });
                }
            };
        }
    };

    global.m_andruav_channel_child_socket = {
        getInstance: function () {
            return {
                forwardMessage: function (p_message, p_isBinary, p_source_ws) {
                    c_harness.childSent.push({ message: p_message, isBinary: p_isBinary, sourceWs: p_source_ws });
                }
            };
        }
    };

    // Silence/disable optional logger by default; tests can override.
    global.m_logger = null;
    global.Colors = { BSuccess: '', Error: '', Reset: '' };

    return c_harness;
}


/**
 * Restores the globals that were present before installFakeGlobals().
 */
function restoreGlobals() {
    if (v_saved === null) return;
    for (const c_key of GLOBAL_KEYS) {
        const c_entry = v_saved[c_key];
        if (c_entry.present) {
            global[c_key] = c_entry.value;
        } else {
            delete global[c_key];
        }
    }
    v_saved = null;
}


/**
 * Builds a minimal fake WebSocket-like object used as a message sender.
 *
 * @param {object} p_opts { senderID, actorType, prm, group, name }
 *   group: { m_ID, m_parentAccount:{ m_accountID } } or null
 * @returns fake ws with a `sent` array capturing ws.send(...) payloads.
 */
function makeFakeWs(p_opts) {
    p_opts = p_opts || {};
    const c_ws = {
        name: p_opts.name || p_opts.senderID || 'unit1',
        m__group: p_opts.group === undefined ? null : p_opts.group,
        m_loginRequest: {
            m_senderID: p_opts.senderID || 'unit1',
            m_actorType: p_opts.actorType || 'd',
            m_prm: p_opts.prm === undefined ? 0xffffffff : p_opts.prm
        },
        m_status: { m_TTX: 0, m_BTX: 0 },
        sent: [],
        send: function (p_payload) { c_ws.sent.push(p_payload); },
        close: function () { c_ws.closed = true; }
    };
    return c_ws;
}


/**
 * Builds a binary comm-server frame: JSON header + char(0) + binary payload.
 * @param {object} p_header JSON header object
 * @param {Buffer} p_payload binary payload (optional)
 * @returns {Buffer}
 */
function buildBinaryFrame(p_header, p_payload) {
    const c_json = Buffer.from(JSON.stringify(p_header), 'utf8');
    const c_null = Buffer.from([0]);
    const c_payload = p_payload || Buffer.from([1, 2, 3, 4]);
    return Buffer.concat([c_json, c_null, c_payload]);
}


/**
 * Parses the JSON header from a (text or binary) forwarded frame.
 * @param {string|Buffer} p_message
 * @returns {object}
 */
function parseHeader(p_message) {
    if (Buffer.isBuffer(p_message)) {
        const c_nullIndex = p_message.indexOf(0);
        const c_jsonPart = c_nullIndex !== -1 ? p_message.subarray(0, c_nullIndex) : p_message;
        return JSON.parse(c_jsonPart.toString('utf8'));
    }
    return JSON.parse(p_message);
}


module.exports = {
    installFakeGlobals,
    restoreGlobals,
    makeFakeWs,
    buildBinaryFrame,
    parseHeader
};
