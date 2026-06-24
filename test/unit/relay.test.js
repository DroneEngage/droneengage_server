"use strict";

/**
 * Unit tests for server/chat_server/js_chat_relay.js
 *
 * Covers the server-to-server mesh forwarding logic:
 *  - origin id resolution
 *  - _path loop-prevention trail injection (text + binary)
 *  - _gid / _aid account/group injection (forwardMessage only)
 *  - binary payload preservation
 *  - routing to parent (super) and child (persistant relay) channels
 */

const test = require('node:test');
const assert = require('node:assert');

const {
    installFakeGlobals,
    restoreGlobals,
    makeFakeWs,
    buildBinaryFrame,
    parseHeader
} = require('../helpers/test_globals.js');

const c_relay = require('../../server/chat_server/js_chat_relay.js');


test.afterEach(() => restoreGlobals());


test('getServerOriginID returns configured server_id', () => {
    installFakeGlobals({ server_id: 'SrvA' });
    assert.strictEqual(c_relay.getServerOriginID(), 'SrvA');
});

test('getServerOriginID falls back to "unknown" when config missing', () => {
    installFakeGlobals({});
    global.m_serverconfig = undefined;
    assert.strictEqual(c_relay.getServerOriginID(), 'unknown');
});


test('forwardMessage (text) appends server_id to _path and injects _gid/_aid', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_super_server: true });
    const ws = makeFakeWs({
        senderID: 'u1',
        group: { m_ID: 'g1', m_parentAccount: { m_accountID: 'acc1' } }
    });

    c_relay.forwardMessage(JSON.stringify({ ty: 'g', sd: 'u1' }), false, ws);

    assert.strictEqual(h.parentSent.length, 1);
    const hdr = parseHeader(h.parentSent[0].message);
    assert.deepStrictEqual(hdr._path, ['SrvA']);
    assert.strictEqual(hdr._gid, 'g1');
    assert.strictEqual(hdr._aid, 'acc1');
});


test('forwardMessage does not inject _gid/_aid when ws has no group', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_super_server: true });
    const ws = makeFakeWs({ senderID: 'u1', group: null });

    c_relay.forwardMessage(JSON.stringify({ ty: 'g', sd: 'u1' }), false, ws);

    const hdr = parseHeader(h.parentSent[0].message);
    assert.deepStrictEqual(hdr._path, ['SrvA']);
    assert.strictEqual(hdr._gid, undefined);
    assert.strictEqual(hdr._aid, undefined);
});


test('forwardMessage appends to an existing _path instead of overwriting', () => {
    const h = installFakeGlobals({ server_id: 'SrvB', enable_super_server: true });
    const ws = makeFakeWs({ senderID: 'u1', group: null });

    c_relay.forwardMessage(JSON.stringify({ ty: 'g', sd: 'u1', _path: ['SrvA'] }), false, ws);

    const hdr = parseHeader(h.parentSent[0].message);
    assert.deepStrictEqual(hdr._path, ['SrvA', 'SrvB']);
});


test('forwardMessage is a no-op when ws or m_loginRequest is null', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_super_server: true });

    c_relay.forwardMessage(JSON.stringify({ ty: 'g' }), false, null);
    c_relay.forwardMessage(JSON.stringify({ ty: 'g' }), false, { m_loginRequest: null });

    assert.strictEqual(h.parentSent.length, 0);
});


test('forwardMessage (binary) preserves the binary payload after the null terminator', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_super_server: true });
    const ws = makeFakeWs({ senderID: 'u1', group: null });

    const payload = Buffer.from([9, 8, 7, 6, 5]);
    const frame = buildBinaryFrame({ ty: 'g', sd: 'u1' }, payload);

    c_relay.forwardMessage(frame, true, ws);

    const sent = h.parentSent[0].message;
    assert.ok(Buffer.isBuffer(sent));
    const hdr = parseHeader(sent);
    assert.deepStrictEqual(hdr._path, ['SrvA']);

    // payload after the null terminator must be byte-identical
    const nullIndex = sent.indexOf(0);
    const sentPayload = sent.subarray(nullIndex + 1);
    assert.deepStrictEqual(sentPayload, payload);
});


test('forwardMessage routes to child channel when enable_persistant_relay is true', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_persistant_relay: true });
    const ws = makeFakeWs({ senderID: 'u1', group: null });

    c_relay.forwardMessage(JSON.stringify({ ty: 'g', sd: 'u1' }), false, ws);

    assert.strictEqual(h.parentSent.length, 0);
    assert.strictEqual(h.childSent.length, 1);
});


test('forwardMessage routes to both channels when both relay modes enabled', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_super_server: true, enable_persistant_relay: true });
    const ws = makeFakeWs({ senderID: 'u1', group: null });

    c_relay.forwardMessage(JSON.stringify({ ty: 'g', sd: 'u1' }), false, ws);

    assert.strictEqual(h.parentSent.length, 1);
    assert.strictEqual(h.childSent.length, 1);
});


test('forwardExternalMessage appends _path but does NOT inject _gid/_aid', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_super_server: true });

    c_relay.forwardExternalMessage(JSON.stringify({ ty: 'g', sd: 'u1' }), false, { tag: 'sourceChild' });

    assert.strictEqual(h.parentSent.length, 1);
    const hdr = parseHeader(h.parentSent[0].message);
    assert.deepStrictEqual(hdr._path, ['SrvA']);
    assert.strictEqual(hdr._gid, undefined);
    assert.strictEqual(hdr._aid, undefined);
});


test('forwardExternalMessage passes source_ws to parent channel (child exclusion)', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_super_server: true });
    const sourceWs = { tag: 'childA' };

    c_relay.forwardExternalMessage(JSON.stringify({ ty: 'g', sd: 'u1' }), false, sourceWs);

    assert.strictEqual(h.parentSent[0].sourceWs, sourceWs);
});


test('forwardExternalMessage forwards to grandparent (child mode) and excludes nothing on child channel', () => {
    const h = installFakeGlobals({ server_id: 'SrvA', enable_persistant_relay: true });

    c_relay.forwardExternalMessage(JSON.stringify({ ty: 'g', sd: 'u1' }), false, { tag: 'src' });

    assert.strictEqual(h.childSent.length, 1);
    const hdr = parseHeader(h.childSent[0].message);
    assert.deepStrictEqual(hdr._path, ['SrvA']);
});
