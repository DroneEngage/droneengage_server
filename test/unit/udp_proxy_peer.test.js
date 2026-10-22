"use strict";

/**
 * GCS-side pinning on the comm server (TASK-54): the party IP stored at
 * connect (trust_proxy_header), gcs_ips on relay open, peer_allow when a GCS
 * joins, and the 9021 UdpProxyPeer system message.
 *
 * Parties connect through the real js_chat_connection.fn_onConnect_Handler
 * (local_server_enabled + legacy query credentials) with a fake socket, so
 * the accept and close hooks run as in production. Relay tests spawn a real
 * relay process (see relay_process_helper.js).
 */

const test = require('node:test');
const assert = require('node:assert');
const EventEmitter = require('events');

const { installFakeGlobals, restoreGlobals } = require('../helpers/test_globals.js');
const r = require('./relay_process_helper.js');

const c_CONSTANTS = require('../../src/js_constants.js');
const c_connection = require('../../src/comm_server/chat_server/js_chat_connection.js');
const c_systemCommands = require('../../src/comm_server/chat_server/js_chat_system_commands.js');
const c_relayClient = require('../../src/comm_server/server_to_server/js_udp_relay_client.js');
const c_manager = require('../../src/comm_server/js_comm_server_manager_client.js');

const CONST_SERVER_ID = 'DE_CommSrv_Peer';


class CFakeSocket extends EventEmitter {
    constructor(p_ip) {
        super();
        this.readyState = 1;
        this.sent = [];
        this._socket = { remoteAddress: p_ip, remotePort: 40000 };
    }
    send(p_payload) { this.sent.push(JSON.parse(p_payload)); }
    close() { if (this.readyState === 1) { this.readyState = 3; this.emit('close', 1000); } }
    terminate() { this.close(); }
}


let g_origSend = null;

function fn_setup(p_overrides) {
    process.env.de_s2s_shared_secret = r.CONST_SECRET;
    installFakeGlobals(Object.assign({
        server_id: CONST_SERVER_ID,
        public_host: 'comm.test.local',
        local_server_enabled: true,
        local_server_account_id: 'acc1',
        allow_udpproxy_fixed_port: false
    }, p_overrides || {}));
    require('../../src/comm_server/js_s2s_auth.js').fn_resetKeyCache();
    g_origSend = c_manager.fn_sendMessage;
    c_manager.fn_sendMessage = function () {};
}

function fn_teardown(p_sockets) {
    (p_sockets || []).forEach(function (p_ws) { p_ws.close(); });
    c_relayClient.fn_shutdown();
    c_manager.fn_sendMessage = g_origSend;
    restoreGlobals();
}


/**
 * Connects a party through fn_onConnect_Handler. p_opts {ip, xff, account}.
 */
function fn_connect(p_name, p_actor, p_opts) {
    const c_opts = p_opts || {};
    if (c_opts.account) global.m_serverconfig.m_configuration.local_server_account_id = c_opts.account;
    const c_ws = new CFakeSocket(c_opts.ip || '127.0.0.1');
    const c_headers = {};
    if (c_opts.xff) c_headers['x-forwarded-for'] = c_opts.xff;
    c_connection.fn_onConnect_Handler(c_ws, {
        url: `/?f=key${p_name}&s=${p_name}&at=${p_actor}`,
        headers: c_headers,
        socket: { remoteAddress: c_opts.ip || '127.0.0.1' }
    });
    global.m_serverconfig.m_configuration.local_server_account_id = 'acc1';
    assert.ok(c_ws.m_loginRequest, p_name + ' accepted');
    return c_ws;
}


function fn_9008(p_unitWs) {
    c_systemCommands.onSystemMessage({
        ty: 's', mt: c_CONSTANTS.CONST_TYPE_AndruavSystem_UdpProxy,
        ms: { en: true, socket1: { address: '0.0.0.0', port: 0 }, socket2: { address: '0.0.0.0', port: 0 } }
    }, p_unitWs);
}


/**
 * Sends 9021 from p_gcsWs and resolves with the reply's ms.
 */
async function fn_9021(p_gcsWs, p_unit, p_act) {
    const c_before = p_gcsWs.sent.length;
    c_systemCommands.onSystemMessage({
        ty: 's', mt: c_CONSTANTS.CONST_TYPE_AndruavSystem_UdpProxyPeer, ms: { tg_unit: p_unit, act: p_act }
    }, p_gcsWs);
    assert.ok(await r.fn_waitFor(function () { return p_gcsWs.sent.length > c_before; }), '9021 reply');
    const c_reply = p_gcsWs.sent[p_gcsWs.sent.length - 1];
    assert.strictEqual(c_reply.mt, 9021);
    assert.strictEqual(c_reply.ty, 'i');
    assert.strictEqual(c_reply.sd, '_SYS_');
    return c_reply.ms;
}


async function fn_relayRow(p_relay, p_unit) {
    const c_rows = await p_relay.list();
    return c_rows.find(function (p_row) { return p_row.unit === p_unit; });
}


async function fn_startRelayed() {
    const c_relay = await r.fn_startRelay(await r.fn_freeTcpPort(), { gcs_peer_policy: 'allowlist_then_tofu' });
    global.m_serverconfig.m_configuration.udp_relay = [{ host: '127.0.0.1', port: c_relay.m_cfg.server_port, id: 'R1', ssl: c_relay.m_ssl }];
    c_relayClient.fn_initialize();
    assert.ok(await r.fn_waitFor(function () { return c_relayClient.fn_isConnected(); }), 'relay link up');
    return c_relay;
}


// ----------------------------------------------------------------------

test('party IP at connect: first X-Forwarded-For entry with trust_proxy_header, socket address otherwise', function () {
    fn_setup({ trust_proxy_header: true });
    const c_sockets = [];
    try {
        const c_gcs = fn_connect('gcs_xff', 'g', { ip: '::ffff:10.0.0.9', xff: '203.0.113.7, 10.0.0.1' });
        c_sockets.push(c_gcs);
        assert.strictEqual(c_gcs.m_remoteIp, '203.0.113.7');

        global.m_serverconfig.m_configuration.trust_proxy_header = false;
        const c_gcs2 = fn_connect('gcs_noxff', 'g', { ip: '::ffff:10.0.0.9', xff: '203.0.113.7' });
        c_sockets.push(c_gcs2);
        assert.strictEqual(c_gcs2.m_remoteIp, '10.0.0.9', 'header ignored, IPv4-mapped reduced');

        global.m_serverconfig.m_configuration.trust_proxy_header = true;
        assert.strictEqual(c_connection.fn_getClientIp({ headers: {}, socket: { remoteAddress: '198.51.100.2' } }), '198.51.100.2', 'no header -> socket');
    } finally {
        fn_teardown(c_sockets);
    }
});


test('relay open carries gcs_ips of the unit\'s account+group only; a GCS joining later is peer_allow-ed', { skip: r.CONST_SKIP_RELAY }, async function () {
    fn_setup({ trust_proxy_header: true });
    const c_relay = await fn_startRelayed();
    const c_sockets = [];
    try {
        c_sockets.push(fn_connect('gcs_a', 'g', { xff: '10.1.1.1' }));
        c_sockets.push(fn_connect('gcs_other', 'g', { xff: '10.2.2.2', account: 'acc2' }));
        const c_unit = fn_connect('u1', 'd', { xff: '10.9.9.9' });
        c_sockets.push(c_unit);

        fn_9008(c_unit);
        assert.ok(await r.fn_waitFor(function () { return c_unit.sent.some(function (m) { return m.mt === 9008; }); }));
        let v_row = await fn_relayRow(c_relay, 'u1');
        assert.strictEqual(v_row.account, 'acc1');
        assert.deepStrictEqual(v_row.pinned.gcs.allowlist, ['10.1.1.1'], 'only the same-account GCS, not the drone or acc2');
        assert.strictEqual(v_row.pinned.gcs.mode, 'allowlist');

        // a GCS logging in to the same account+group is allowed right away
        c_sockets.push(fn_connect('gcs_late', 'g', { xff: '10.1.1.3' }));
        await r.fn_sleep(150);
        v_row = await fn_relayRow(c_relay, 'u1');
        assert.deepStrictEqual(v_row.pinned.gcs.allowlist, ['10.1.1.1', '10.1.1.3']);

        // ... a GCS of another account is not
        c_sockets.push(fn_connect('gcs_other2', 'g', { xff: '10.2.2.3', account: 'acc2' }));
        await r.fn_sleep(150);
        v_row = await fn_relayRow(c_relay, 'u1');
        assert.deepStrictEqual(v_row.pinned.gcs.allowlist, ['10.1.1.1', '10.1.1.3']);

        // the real close hook: unit_offline, pair kept
        c_unit.close();
        await r.fn_sleep(150);
        v_row = await fn_relayRow(c_relay, 'u1');
        assert.ok(v_row, 'pair kept after the unit socket closed');
        assert.strictEqual(v_row.online, false);
    } finally {
        fn_teardown(c_sockets);
        await c_relay.stop();
    }
});


test('9021 allow: refused for another account or without full-control permission; same account adds the GCS IP', { skip: r.CONST_SKIP_RELAY }, async function () {
    fn_setup({ trust_proxy_header: true });
    const c_relay = await fn_startRelayed();
    const c_sockets = [];
    try {
        const c_unit = fn_connect('u2', 'd', { xff: '10.9.9.8' });
        c_sockets.push(c_unit);
        fn_9008(c_unit);
        assert.ok(await r.fn_waitFor(function () { return c_unit.sent.some(function (m) { return m.mt === 9008; }); }));

        const c_stranger = fn_connect('gcs_stranger', 'g', { xff: '10.66.6.6', account: 'acc2' });
        c_sockets.push(c_stranger);
        assert.deepStrictEqual(await fn_9021(c_stranger, 'u2', 'allow'), { tg_unit: 'u2', error: 'not_allowed' });
        assert.strictEqual((await fn_relayRow(c_relay, 'u2')).pinned.gcs.allowlist, undefined, 'still tofu, nothing allowed');

        const c_viewer = fn_connect('gcs_viewer', 'g', { xff: '10.1.1.7' });
        c_sockets.push(c_viewer);
        c_viewer.m_loginRequest.m_prm = 0x100;   // WP control only, not full control
        assert.deepStrictEqual(await fn_9021(c_viewer, 'u2', 'reset'), { tg_unit: 'u2', error: 'permission_denied' });
        assert.deepStrictEqual(await fn_9021(c_viewer, 'u2', 'allow'), { tg_unit: 'u2', error: 'permission_denied' });

        const c_pilot = fn_connect('gcs_pilot', 'g', { xff: '10.1.1.8' });
        c_sockets.push(c_pilot);
        // joining already allowed 10.1.1.8; the pilot's address then changes
        // (new NAT), which only an explicit 9021 allow fixes
        c_pilot.m_remoteIp = '10.1.1.99';
        assert.ok(!(await fn_relayRow(c_relay, 'u2')).pinned.gcs.allowlist.includes('10.1.1.99'));
        const c_allow = await fn_9021(c_pilot, 'u2', 'allow');
        assert.strictEqual(c_allow.tg_unit, 'u2');
        assert.strictEqual(c_allow.applied, true);
        assert.strictEqual(c_allow.pinned.gcs.mode, 'allowlist');
        assert.ok(c_allow.pinned.gcs.allowlist.includes('10.1.1.99'));
        assert.ok(!c_allow.pinned.gcs.allowlist.includes('10.66.6.6'));

        // non-GCS senders are ignored; a bad act is reported
        c_systemCommands.onSystemMessage({ ty: 's', mt: 9021, ms: { tg_unit: 'u2', act: 'allow' } }, c_unit);
        await r.fn_sleep(50);
        assert.ok(!c_unit.sent.some(function (m) { return m.mt === 9021; }));
        assert.deepStrictEqual(await fn_9021(c_pilot, 'u2', 'explode'), { tg_unit: 'u2', error: 'bad_request' });
    } finally {
        fn_teardown(c_sockets);
        await c_relay.stop();
    }
});


test('9021 query/reset return the relay pinned state', { skip: r.CONST_SKIP_RELAY }, async function () {
    fn_setup({ trust_proxy_header: false });
    const c_relay = await fn_startRelayed();
    const c_sockets = [];
    try {
        const c_unit = fn_connect('u3', 'd', { ip: '127.0.0.1' });
        c_sockets.push(c_unit);
        fn_9008(c_unit);
        assert.ok(await r.fn_waitFor(function () { return c_unit.sent.some(function (m) { return m.mt === 9008; }); }));
        const c_gcs = fn_connect('gcs_q', 'g', { ip: '::ffff:127.0.0.5' });
        c_sockets.push(c_gcs);

        const c_query = await fn_9021(c_gcs, 'u3', 'query');
        assert.deepStrictEqual(c_query, { tg_unit: 'u3', pinned: (await fn_relayRow(c_relay, 'u3')).pinned });
        assert.strictEqual(c_query.pinned.gcs.mode, 'allowlist');
        assert.deepStrictEqual(c_query.pinned.gcs.allowlist, ['127.0.0.5']);

        const c_reset = await fn_9021(c_gcs, 'u3', 'reset');
        assert.strictEqual(c_reset.applied, true);
        assert.strictEqual(c_reset.pinned.gcs.mode, 'allowlist');
    } finally {
        fn_teardown(c_sockets);
        await c_relay.stop();
    }
});


test('9021 without a relay: the built-in proxy is mode open', async function () {
    fn_setup({});
    const c_sockets = [];
    try {
        assert.strictEqual(c_relayClient.fn_isEnabled(), false);
        const c_unit = fn_connect('u4', 'd');
        const c_gcs = fn_connect('gcs_b', 'g');
        c_sockets.push(c_unit, c_gcs);
        assert.deepStrictEqual(await fn_9021(c_gcs, 'u4', 'query'), { tg_unit: 'u4', pinned: { gcs: { mode: 'open' } } });
        assert.deepStrictEqual(await fn_9021(c_gcs, 'u4', 'allow'), { tg_unit: 'u4', pinned: { gcs: { mode: 'open' } } });
        // unknown unit -> not_allowed
        assert.deepStrictEqual(await fn_9021(c_gcs, 'nobody', 'query'), { tg_unit: 'nobody', error: 'not_allowed' });
    } finally {
        fn_teardown(c_sockets);
    }
});
