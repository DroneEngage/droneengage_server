"use strict";

/**
 * Tests for the UDP relay broker (src/comm_server/server_to_server/js_udp_relay_client.js)
 * and the 9008 routing in js_chat_system_commands.js.
 *
 * The regression test needs nothing external. The integration tests spawn a
 * real relay process from the relay repo, found via DE_UDP_RELAY_DIR or as a
 * sibling checkout named droneengage_udp_relay_server / droneengage_udp_proxy
 * (next to this repo or one level up, for worktrees). They are skipped with
 * a message when no relay checkout with src/control_server.js is found.
 */

const test = require('node:test');
const assert = require('node:assert');
const child_process = require('child_process');
const dgram = require('dgram');
const fs = require('fs');
const net = require('net');
const path = require('path');

const { installFakeGlobals, restoreGlobals } = require('../helpers/test_globals.js');

const c_CONSTANTS = require('../../src/js_constants.js');
const c_udp = require('../../src/comm_server/js_udp_proxy.js');
const c_relayClient = require('../../src/comm_server/server_to_server/js_udp_relay_client.js');
const c_systemCommands = require('../../src/comm_server/chat_server/js_chat_system_commands.js');
const c_activeSenders = require('../../src/comm_server/chat_server/js_andruav_active_senders.js');

const CONST_ROOT = path.resolve(__dirname, '../..');
const CONST_SECRET = 'udp-relay-test-secret-0123456789abcdef';
const CONST_SERVER_ID = 'DE_CommSrv_Test';
const CONST_SSL_KEY = path.join(CONST_ROOT, 'ssl_local/ssl_airgap/domain.key');
const CONST_SSL_CERT = path.join(CONST_ROOT, 'ssl_local/ssl_airgap/domain.crt');


function fn_findRelayDir() {
    const c_candidates = [];
    if (process.env.DE_UDP_RELAY_DIR) c_candidates.push(path.resolve(process.env.DE_UDP_RELAY_DIR));
    for (const c_up of ['..', '../..']) {
        for (const c_name of ['droneengage_udp_relay_server', 'droneengage_udp_proxy']) {
            c_candidates.push(path.resolve(CONST_ROOT, c_up, c_name));
        }
    }
    return c_candidates.find(function (p_dir) {
        return fs.existsSync(path.join(p_dir, 'src/control_server.js'))
            && fs.existsSync(path.join(p_dir, 'node_modules/ws'));
    }) || null;
}

const CONST_RELAY_DIR = fn_findRelayDir();
const CONST_SKIP_RELAY = CONST_RELAY_DIR === null
    ? 'no relay checkout (set DE_UDP_RELAY_DIR to droneengage_udp_relay_server with npm install done)'
    : false;


function fn_sleep(p_ms) {
    return new Promise(function (resolve) { setTimeout(resolve, p_ms); });
}

async function fn_waitFor(p_cond, p_timeoutMs) {
    const c_end = Date.now() + (p_timeoutMs || 3000);
    while (Date.now() < c_end) {
        if (p_cond()) return true;
        await fn_sleep(10);
    }
    return p_cond();
}

function fn_freeTcpPort() {
    return new Promise(function (resolve) {
        const c_srv = net.createServer();
        c_srv.listen(0, '127.0.0.1', function () {
            const c_port = c_srv.address().port;
            c_srv.close(function () { resolve(c_port); });
        });
    });
}


/**
 * A real relay process: the relay repo's RelayPairs + ControlServer with a
 * policy hook that returns tk (and echoes cap) when the request advertises
 * cap:['tk']. stdin commands: `close <account> <unit>`, `list`.
 */
const CONST_RELAY_SCRIPT = `
'use strict';
const path = require('path');
const dir = process.env.RELAY_DIR;
const cfg = JSON.parse(process.env.RELAY_CONFIG);
global.m_serverconfig = { m_configuration: cfg };
const { RelayPairs } = require(path.join(dir, 'src/relay_pairs.js'));
const { ControlServer } = require(path.join(dir, 'src/control_server.js'));
const pairs = new RelayPairs(cfg, {
    hookFactory: function (ctx, opts) {
        const tk = Array.isArray(opts.cap) && opts.cap.includes('tk');
        return {
            hook: function (side, rinfo) { ctx.setPeer(side, rinfo); return 'forward'; },
            reply: tk ? { tk: 'TK-' + ctx.unit, cap_seen: opts.cap } : {}
        };
    }
});
const control = new ControlServer(cfg, pairs, { log: process.env.RELAY_DEBUG ? function (t) { console.error('[relay] ' + t); } : undefined });
control.start().then(function (port) { console.log('READY ' + port); });
let buf = '';
process.stdin.on('data', function (d) {
    buf += d.toString();
    let i;
    while ((i = buf.indexOf('\\n')) >= 0) {
        const line = buf.slice(0, i).trim().split(' ');
        buf = buf.slice(i + 1);
        if (line[0] === 'close') pairs.close(line[1], line[2]);
        if (line[0] === 'list') console.log('LIST ' + JSON.stringify(pairs.list()));
    }
});
`;


async function fn_startRelay(p_port) {
    const c_ssl = fs.existsSync(CONST_SSL_KEY) && fs.existsSync(CONST_SSL_CERT);
    const c_lo = 47000 + Math.floor(Math.random() * 100) * 40;
    const c_cfg = {
        server_id: 'DE_UdpRelay_Test',
        server_ip: '127.0.0.1',
        server_port: p_port,
        enable_SSL: c_ssl,
        ssl_key_file: CONST_SSL_KEY,
        ssl_cert_file: CONST_SSL_CERT,
        s2s_cert_enabled: false,
        udp_bind_ip: '0.0.0.0',
        public_host: 'relay.test.local',
        udp_port_range: [c_lo, c_lo + 19],
        allow_fixed_port: true,
        udp_buffer_bytes: 0,
        idle_timeout_s: 300,
        reaper_interval_s: 60,
        owner_grace_s: 120,
        max_pps_per_socket: 0,
        max_bps_per_socket: 0
    };
    const c_child = child_process.spawn(process.execPath, ['-e', CONST_RELAY_SCRIPT], {
        cwd: CONST_RELAY_DIR,
        env: Object.assign({}, process.env, {
            RELAY_DIR: CONST_RELAY_DIR,
            RELAY_CONFIG: JSON.stringify(c_cfg),
            de_s2s_shared_secret: CONST_SECRET
        }),
        stdio: ['pipe', 'pipe', 'pipe']
    });
    const c_relay = { m_child: c_child, m_cfg: c_cfg, m_ssl: c_ssl, m_out: '', m_lists: [] };
    c_child.stdout.on('data', function (d) {
        c_relay.m_out += d.toString();
        let i;
        while ((i = c_relay.m_out.indexOf('\n')) >= 0) {
            const c_line = c_relay.m_out.slice(0, i);
            c_relay.m_out = c_relay.m_out.slice(i + 1);
            if (c_line.startsWith('LIST ')) c_relay.m_lists.push(JSON.parse(c_line.slice(5)));
            if (c_line.startsWith('READY ')) c_relay.m_ready = true;
        }
    });
    c_child.stderr.on('data', function (d) { c_relay.m_err = (c_relay.m_err || '') + d.toString(); });
    c_relay.list = async function () {
        const c_n = c_relay.m_lists.length;
        c_child.stdin.write('list\n');
        await fn_waitFor(function () { return c_relay.m_lists.length > c_n; });
        return c_relay.m_lists[c_relay.m_lists.length - 1];
    };
    c_relay.closePair = function (p_account, p_unit) { c_child.stdin.write(`close ${p_account} ${p_unit}\n`); };
    c_relay.stop = function () {
        return new Promise(function (resolve) {
            if (c_child.exitCode !== null || c_child.signalCode !== null) return resolve();
            c_child.once('exit', function () { resolve(); });
            c_child.kill('SIGKILL');
        });
    };
    assert.ok(await fn_waitFor(function () { return c_relay.m_ready === true; }, 5000), 'relay started: ' + (c_relay.m_err || ''));
    return c_relay;
}


function fn_setupComm(p_overrides) {
    process.env.de_s2s_shared_secret = CONST_SECRET;
    const c_harness = installFakeGlobals(Object.assign({
        server_id: CONST_SERVER_ID,
        public_host: 'comm.test.local',
        allow_udpproxy_fixed_port: false
    }, p_overrides || {}));
    require('../../src/comm_server/js_s2s_auth.js').fn_resetKeyCache();
    return c_harness;
}

function fn_teardownComm() {
    c_relayClient.fn_shutdown();
    restoreGlobals();
}


/**
 * A connected drone websocket as js_chat_connection leaves it after login.
 */
function fn_makeUnit(p_name, p_account) {
    const c_ws = {
        name: p_name,
        readyState: 1,
        m_loginRequest: { m_senderID: p_name, m_actorType: 'd', m_accountID: p_account || 'acc1', m_groupID: 'g1', m_prm: 0xffffffff },
        sent: [],
        send: function (p_payload) { c_ws.sent.push(JSON.parse(p_payload)); }
    };
    c_activeSenders.addActiveSenderIDList(p_name, c_ws);
    return c_ws;
}

function fn_request9008(p_en, p_extra) {
    return Object.assign({
        ty: c_CONSTANTS.CONST_WS_MSG_ROUTING_SYSTEM,
        mt: c_CONSTANTS.CONST_TYPE_AndruavSystem_UdpProxy,
        ms: {
            en: p_en,
            socket1: { address: '0.0.0.0', port: 0 },
            socket2: { address: '0.0.0.0', port: 15412 }
        }
    }, p_extra || {});
}

function fn_relayEndpoint(p_relay) {
    return [{ host: '127.0.0.1', port: p_relay.m_cfg.server_port, id: 'R1', ssl: p_relay.m_ssl }];
}

function fn_udpClient() {
    return new Promise(function (resolve) {
        const c_sock = dgram.createSocket('udp4');
        c_sock.m_inbox = [];
        c_sock.on('message', function (p_buf) { c_sock.m_inbox.push(p_buf.toString()); });
        c_sock.bind(0, '127.0.0.1', function () { resolve(c_sock); });
    });
}

function fn_udpSend(p_sock, p_port, p_text) {
    return new Promise(function (resolve) {
        p_sock.send(Buffer.from(p_text), p_port, '127.0.0.1', function () { resolve(); });
    });
}


// ----------------------------------------------------------------------
// regression: no udp_relay -> built-in path unchanged
// ----------------------------------------------------------------------

test('no udp_relay: 9008 en:true calls getUDPSocket and replies exactly as before', async function () {
    fn_setupComm({});
    const c_origGet = c_udp.getUDPSocket;
    const c_origClose = c_udp.closeUDPSocket;
    const c_calls = [];
    try {
        assert.strictEqual(c_relayClient.fn_isEnabled(), false);
        c_udp.getUDPSocket = function (p_name, p_s1, p_s2, p_cb) {
            c_calls.push(['get', p_name, JSON.parse(JSON.stringify(p_s1)), JSON.parse(JSON.stringify(p_s2))]);
            p_cb({ socket1: { address: 'comm.test.local', port: 50001 }, socket2: { address: 'comm.test.local', port: 50002 }, en: true });
        };
        c_udp.closeUDPSocket = function (p_name, p_cb) {
            c_calls.push(['close', p_name]);
            p_cb({ socket1: { address: '0.0.0.0', port: 0 }, socket2: { address: '0.0.0.0', port: 0 }, en: false });
        };
        const c_unit = fn_makeUnit('drone_reg');
        const c_req = fn_request9008(true, { rq: 'echo-me' });
        c_systemCommands.onSystemMessage(c_req, c_unit);

        // allow_udpproxy_fixed_port false -> both requested ports forced to 0
        assert.deepStrictEqual(c_calls, [['get', 'drone_reg', { address: '0.0.0.0', port: 0 }, { address: '0.0.0.0', port: 0 }]]);
        assert.deepStrictEqual(c_unit.sent, [{
            ty: 'i', mt: 9008, rq: 'echo-me', tg: 'drone_reg', sd: '_SYS_',
            ms: { socket1: { address: 'comm.test.local', port: 50001 }, socket2: { address: 'comm.test.local', port: 50002 }, en: true }
        }]);

        // en:false -> closeUDPSocket, same reply shape
        c_systemCommands.onSystemMessage(fn_request9008(false), c_unit);
        assert.deepStrictEqual(c_calls[1], ['close', 'drone_reg']);
        assert.deepStrictEqual(c_unit.sent[1].ms, { socket1: { address: '0.0.0.0', port: 0 }, socket2: { address: '0.0.0.0', port: 0 }, en: false });

        // a failed proxy: en:false reply via closeUDPSocket, as before
        c_udp.getUDPSocket = function (p_name, p_s1, p_s2, p_cb) { p_cb({ en: false }); };
        c_systemCommands.onSystemMessage(fn_request9008(true), c_unit);
        assert.strictEqual(c_unit.sent[2].ms.en, false);

        // non-drone actors are still ignored
        const c_gcs = fn_makeUnit('gcs_reg');
        c_gcs.m_loginRequest.m_actorType = 'g';
        c_systemCommands.onSystemMessage(fn_request9008(true), c_gcs);
        assert.strictEqual(c_gcs.sent.length, 0);
    } finally {
        c_udp.getUDPSocket = c_origGet;
        c_udp.closeUDPSocket = c_origClose;
        c_activeSenders.deleteActiveSenderIDList('drone_reg');
        c_activeSenders.deleteActiveSenderIDList('gcs_reg');
        fn_teardownComm();
    }
});


// ----------------------------------------------------------------------
// integration against a real relay process
// ----------------------------------------------------------------------

test('relay: drone 9008 gets relay public_host + in-range ports, cap reaches relay, tk reaches unit, datagrams flow', { skip: CONST_SKIP_RELAY }, async function () {
    const c_relay = await fn_startRelay(await fn_freeTcpPort());
    fn_setupComm({});
    global.m_serverconfig.m_configuration.udp_relay = fn_relayEndpoint(c_relay);
    const c_unit = fn_makeUnit('drone_i1');
    let c_drone = null;
    let c_gcs = null;
    try {
        c_relayClient.fn_initialize();
        assert.ok(await fn_waitFor(function () { return c_relayClient.fn_isConnected(); }), 'comm server authenticated with relay');

        c_systemCommands.onSystemMessage(fn_request9008(true, { ms: {
            en: true, socket1: { address: '0.0.0.0', port: 0 }, socket2: { address: '0.0.0.0', port: 14550 }, cap: ['tk']
        } }), c_unit);
        assert.ok(await fn_waitFor(function () { return c_unit.sent.length === 1; }), 'reply to unit; relay stderr: ' + (c_relay.m_err || ''));
        const c_rep = c_unit.sent[0];
        assert.strictEqual(c_rep.mt, 9008);
        assert.strictEqual(c_rep.ty, 'i');
        assert.strictEqual(c_rep.tg, 'drone_i1');
        assert.strictEqual(c_rep.sd, '_SYS_');
        assert.strictEqual(c_rep.ms.en, true);
        const [c_lo, c_hi] = c_relay.m_cfg.udp_port_range;
        for (const c_s of [c_rep.ms.socket1, c_rep.ms.socket2]) {
            assert.strictEqual(c_s.address, 'relay.test.local');
            assert.ok(c_s.port >= c_lo && c_s.port <= c_hi, 'port ' + c_s.port + ' in relay range');
        }
        assert.strictEqual(c_rep.ms.tk, 'TK-drone_i1');
        assert.deepStrictEqual(c_rep.ms.cap_seen, ['tk']);
        assert.strictEqual(c_rep.ms.pair_id, undefined);

        const c_rows = await c_relay.list();
        assert.strictEqual(c_rows.length, 1);
        assert.strictEqual(c_rows[0].account, 'acc1');
        assert.strictEqual(c_rows[0].group, 'g1');
        assert.strictEqual(c_rows[0].owner, CONST_SERVER_ID);

        // MAVLink-ish datagrams through the relay (bound on 0.0.0.0, reached via loopback)
        c_drone = await fn_udpClient();
        c_gcs = await fn_udpClient();
        await fn_udpSend(c_gcs, c_rep.ms.socket2.port, 'gcs-hello');
        await fn_sleep(100);
        await fn_udpSend(c_drone, c_rep.ms.socket1.port, 'telemetry');
        assert.ok(await fn_waitFor(function () { return c_gcs.m_inbox.includes('telemetry'); }), 'socket1 -> socket2 peer');
        await fn_udpSend(c_gcs, c_rep.ms.socket2.port, 'command');
        assert.ok(await fn_waitFor(function () { return c_drone.m_inbox.includes('command'); }), 'socket2 -> socket1 peer');

        // en:false closes the relay pair
        c_systemCommands.onSystemMessage(fn_request9008(false), c_unit);
        assert.ok(await fn_waitFor(function () { return c_unit.sent.length === 2; }), 'en:false reply; relay stderr: ' + (c_relay.m_err || ''));
        assert.deepStrictEqual(c_unit.sent[1].ms, { en: false, socket1: { address: '0.0.0.0', port: 0 }, socket2: { address: '0.0.0.0', port: 0 } });
        assert.strictEqual((await c_relay.list()).length, 0);
    } finally {
        if (c_drone) c_drone.close();
        if (c_gcs) c_gcs.close();
        c_activeSenders.deleteActiveSenderIDList('drone_i1');
        fn_teardownComm();
        await c_relay.stop();
    }
});


test('relay: unit disconnect sends unit_offline (pair kept); relay closed push gives the unit 9008 en:false', { skip: CONST_SKIP_RELAY }, async function () {
    const c_relay = await fn_startRelay(await fn_freeTcpPort());
    fn_setupComm({});
    global.m_serverconfig.m_configuration.udp_relay = fn_relayEndpoint(c_relay);
    const c_unit1 = fn_makeUnit('drone_off');
    const c_unit2 = fn_makeUnit('drone_push');
    try {
        c_relayClient.fn_initialize();
        assert.ok(await fn_waitFor(function () { return c_relayClient.fn_isConnected(); }));
        c_systemCommands.onSystemMessage(fn_request9008(true), c_unit1);
        c_systemCommands.onSystemMessage(fn_request9008(true), c_unit2);
        assert.ok(await fn_waitFor(function () { return c_unit1.sent.length === 1 && c_unit2.sent.length === 1; }));

        // what js_chat_connection's close hook calls
        c_activeSenders.deleteActiveSenderIDList('drone_off');
        c_relayClient.fn_onUnitDisconnected(c_unit1);
        await fn_sleep(150);
        const c_rows = await c_relay.list();
        const v_row = c_rows.find(function (r) { return r.unit === 'drone_off'; });
        assert.ok(v_row, 'pair still exists (unit_offline, not close)');
        assert.strictEqual(v_row.online, false);
        assert.strictEqual(c_rows.find(function (r) { return r.unit === 'drone_push'; }).online, true);

        // the relay closes drone_push's pair (e.g. admin): the unit is told
        c_relay.closePair('acc1', 'drone_push');
        assert.ok(await fn_waitFor(function () { return c_unit2.sent.length === 2; }));
        assert.strictEqual(c_unit2.sent[1].mt, 9008);
        assert.strictEqual(c_unit2.sent[1].tg, 'drone_push');
        assert.deepStrictEqual(c_unit2.sent[1].ms, { en: false, socket1: { address: '0.0.0.0', port: 0 }, socket2: { address: '0.0.0.0', port: 0 } });
    } finally {
        c_activeSenders.deleteActiveSenderIDList('drone_off');
        c_activeSenders.deleteActiveSenderIDList('drone_push');
        fn_teardownComm();
        await c_relay.stop();
    }
});


test('relay down + fallback none: en:false after udp_relay_pending_s, then unsolicited en:true when the relay comes up', { skip: CONST_SKIP_RELAY }, async function () {
    const c_port = await fn_freeTcpPort();
    fn_setupComm({ udp_relay_fallback: 'none', udp_relay_pending_s: 0.3 });
    const c_ssl = fs.existsSync(CONST_SSL_KEY) && fs.existsSync(CONST_SSL_CERT);
    global.m_serverconfig.m_configuration.udp_relay = [{ host: '127.0.0.1', port: c_port, id: 'R1', ssl: c_ssl }];
    const c_unit = fn_makeUnit('drone_wait');
    let c_relay = null;
    try {
        c_relayClient.fn_initialize();
        const c_t0 = Date.now();
        c_systemCommands.onSystemMessage(fn_request9008(true), c_unit);
        await fn_sleep(150);
        assert.strictEqual(c_unit.sent.length, 0, 'request is held while the relay is down');
        assert.ok(await fn_waitFor(function () { return c_unit.sent.length === 1; }));
        assert.ok(Date.now() - c_t0 >= 290, 'en:false only after udp_relay_pending_s');
        assert.strictEqual(c_unit.sent[0].ms.en, false);

        // relay comes up: the client reconnects (backoff) and pushes en:true unasked
        c_relay = await fn_startRelay(c_port);
        assert.ok(await fn_waitFor(function () { return c_unit.sent.length === 2; }, 8000), 'unsolicited 9008 en:true');
        const c_push = c_unit.sent[1];
        assert.strictEqual(c_push.mt, 9008);
        assert.strictEqual(c_push.ty, 'i');
        assert.strictEqual(c_push.tg, 'drone_wait');
        assert.strictEqual(c_push.ms.en, true);
        assert.strictEqual(c_push.ms.socket1.address, 'relay.test.local');
    } finally {
        c_activeSenders.deleteActiveSenderIDList('drone_wait');
        fn_teardownComm();
        if (c_relay) await c_relay.stop();
    }
});


test('relay down + fallback builtin: the unit gets a built-in pair on the comm server public_host', async function () {
    const c_port = await fn_freeTcpPort();
    fn_setupComm({ udp_relay_fallback: 'builtin', udp_relay_pending_s: 0.2 });
    global.m_serverconfig.m_configuration.udp_relay = [{ host: '127.0.0.1', port: c_port, id: 'R1' }];
    const c_unit = fn_makeUnit('drone_bi');
    try {
        c_relayClient.fn_initialize();
        c_systemCommands.onSystemMessage(fn_request9008(true), c_unit);
        assert.ok(await fn_waitFor(function () { return c_unit.sent.length === 1; }));
        const c_ms = c_unit.sent[0].ms;
        assert.strictEqual(c_ms.en, true);
        assert.strictEqual(c_ms.socket1.address, 'comm.test.local');
        assert.strictEqual(c_ms.socket2.address, 'comm.test.local');
        assert.ok(c_udp.fn_getActiveProxiesList().some(function (r) { return r.name === 'drone_bi'; }));

        // en:false goes to the built-in proxy it was served from
        c_systemCommands.onSystemMessage(fn_request9008(false), c_unit);
        assert.ok(await fn_waitFor(function () { return c_unit.sent.length === 2; }));
        assert.strictEqual(c_unit.sent[1].ms.en, false);
        assert.ok(!c_udp.fn_getActiveProxiesList().some(function (r) { return r.name === 'drone_bi'; }));
    } finally {
        c_udp.closeUDPSocket('drone_bi', function () {});
        c_activeSenders.deleteActiveSenderIDList('drone_bi');
        fn_teardownComm();
    }
});


test('relay: AUTH e lists relay rows, g blocks via the relay, j status on connect, change and heartbeat', { skip: CONST_SKIP_RELAY }, async function () {
    const c_relay = await fn_startRelay(await fn_freeTcpPort());
    fn_setupComm({});
    global.m_serverconfig.m_configuration.udp_relay = fn_relayEndpoint(c_relay);
    const c_commServer = require('../../src/comm_server/js_andruav_comm_server.js');
    const c_manager = require('../../src/comm_server/js_comm_server_manager_client.js');
    const c_origSend = c_manager.fn_sendMessage;
    const c_toAuth = [];
    c_manager.fn_sendMessage = function (p_msg) { c_toAuth.push(JSON.parse(p_msg)); };
    const c_unit = fn_makeUnit('drone_dash');
    const fn_status = function () { return c_toAuth.filter(function (m) { return m.c === 'j'; }); };
    try {
        c_relayClient.fn_initialize({ heartbeatMs: 150 });
        c_relayClient.fn_setSendToAuthCallback(function (p_msg) { c_manager.fn_sendMessage(p_msg); });
        assert.ok(await fn_waitFor(function () {
            return fn_status().some(function (m) { return m.d.status === c_CONSTANTS.CONST_STORAGE_STATE_CONNECTED; });
        }), 'j connected on connect');
        const c_first = fn_status().find(function (m) { return m.d.status === c_CONSTANTS.CONST_STORAGE_STATE_CONNECTED; });
        assert.strictEqual(c_first.d.relay_id, 'R1');
        assert.strictEqual(c_first.d.host, '127.0.0.1');
        assert.strictEqual(c_first.d.port, c_relay.m_cfg.server_port);

        c_systemCommands.onSystemMessage(fn_request9008(true), c_unit);
        assert.ok(await fn_waitFor(function () { return c_unit.sent.length === 1; }));

        // e: the relay pair shows up in the report
        c_commServer.fn_AuthServerMessagesHandler(JSON.stringify({ c: 'e', d: { r: 'req-e' } }));
        assert.ok(await fn_waitFor(function () { return c_toAuth.some(function (m) { return m.c === 'f'; }); }));
        const c_report = c_toAuth.find(function (m) { return m.c === 'f'; });
        assert.strictEqual(c_report.d.r, 'req-e');
        const c_row = c_report.d.proxies.find(function (r) { return r.name === 'drone_dash'; });
        assert.ok(c_row, 'relay row in e report');
        assert.strictEqual(c_row.account, 'acc1');
        assert.strictEqual(c_row.owner, CONST_SERVER_ID);
        assert.strictEqual(c_row.relay_id, 'DE_UdpRelay_Test');
        assert.strictEqual(c_row.socket1.port, c_unit.sent[0].ms.socket1.port);
        assert.strictEqual(c_row.blocked, false);

        // g: block goes to the relay
        c_commServer.fn_AuthServerMessagesHandler(JSON.stringify({ c: 'g', d: { name: 'drone_dash', blocked: true } }));
        assert.ok(await fn_waitFor(function () { return c_toAuth.some(function (m) { return m.c === 'f' && m.d.name === 'drone_dash'; }); }));
        const c_gReply = c_toAuth.find(function (m) { return m.c === 'f' && m.d.name === 'drone_dash'; });
        assert.strictEqual(c_gReply.d.applied, true);
        assert.strictEqual(c_gReply.d.proxies.find(function (r) { return r.name === 'drone_dash'; }).blocked, true);
        assert.strictEqual((await c_relay.list())[0].blocked, true);

        // heartbeat: repeated j while connected, with the pair count
        const c_before = fn_status().length;
        await fn_sleep(400);
        assert.ok(fn_status().length >= c_before + 2, 'heartbeat j every interval');
        assert.strictEqual(fn_status()[fn_status().length - 1].d.pairs, 1);

        // change: relay goes away -> disconnected status right away
        await c_relay.stop();
        assert.ok(await fn_waitFor(function () {
            return fn_status().some(function (m) { return m.d.status === c_CONSTANTS.CONST_STORAGE_STATE_DISCONNECTED; });
        }), 'j disconnected on change');
    } finally {
        c_manager.fn_sendMessage = c_origSend;
        c_activeSenders.deleteActiveSenderIDList('drone_dash');
        fn_teardownComm();
        await c_relay.stop();
    }
});
