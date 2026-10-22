"use strict";

/**
 * Shared by the UDP relay tests (not a test file itself): finds the relay
 * checkout and spawns a real relay process from it.
 *
 * The relay repo is found via DE_UDP_RELAY_DIR or as a sibling checkout named
 * droneengage_udp_relay_server / droneengage_udp_proxy (next to this repo or
 * one level up, for worktrees); tests skip with CONST_SKIP_RELAY otherwise.
 * The spawned relay uses the relay repo's RelayPairs + ControlServer with a
 * policy hook that wraps the default policy (GCS pinning, without the unit
 * token) and returns a fake tk 'TK-<unit>' (echoing cap) when the request
 * advertises cap:['tk']. stdin commands: `close <account> <unit>`, `list`.
 */

const assert = require('node:assert');
const child_process = require('child_process');
const fs = require('fs');
const net = require('net');
const path = require('path');

const CONST_ROOT = path.resolve(__dirname, '../..');
const CONST_SECRET = 'udp-relay-test-secret-0123456789abcdef';
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
let fn_gcsPolicy = null;
try { fn_gcsPolicy = require(path.join(dir, 'src/gcs_policy.js')).fn_createGcsPolicy; } catch (e) { /* relay before TASK-53 */ }
const pairs = new RelayPairs(cfg, {
    hookFactory: function (ctx, opts) {
        const tk = Array.isArray(opts.cap) && opts.cap.includes('tk');
        // cap is kept away from the real policy: these comm-server tests use a
        // fake tk and no DEU1 hellos, so socket1 stays last-sender (token
        // pinning is tested in the relay repo)
        const policy = fn_gcsPolicy ? fn_gcsPolicy(ctx, Object.assign({}, opts, { cap: undefined }), cfg) : {
            hook: function (side, rinfo) { ctx.setPeer(side, rinfo); return 'forward'; }
        };
        policy.reply = tk ? { tk: 'TK-' + ctx.unit, cap_seen: opts.cap } : {};
        return policy;
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


async function fn_startRelay(p_port, p_overrides) {
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
    Object.assign(c_cfg, p_overrides || {});
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



module.exports = {
    CONST_ROOT,
    CONST_SECRET,
    CONST_SSL_KEY,
    CONST_SSL_CERT,
    CONST_RELAY_DIR,
    CONST_SKIP_RELAY,
    fn_sleep,
    fn_waitFor,
    fn_freeTcpPort,
    fn_startRelay
};
