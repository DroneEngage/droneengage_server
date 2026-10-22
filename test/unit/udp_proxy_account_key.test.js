"use strict";

/**
 * Built-in UDP proxy keyed by (account, unit): same-named units in two
 * accounts get separate pairs; AUTH `g` blocks per account.
 */

const test = require('node:test');
const assert = require('node:assert');

const { installFakeGlobals, restoreGlobals } = require('../helpers/test_globals.js');

const c_udp = require('../../src/comm_server/js_udp_proxy.js');
const c_systemCommands = require('../../src/comm_server/chat_server/js_chat_system_commands.js');


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


function fn_makeDrone(p_name, p_account) {
    const c_ws = {
        name: p_name,
        readyState: 1,
        m_loginRequest: { m_senderID: p_name, m_actorType: 'd', m_accountID: p_account, m_groupID: 'g1', m_prm: 0xffffffff },
        sent: [],
        send: function (p_payload) { c_ws.sent.push(JSON.parse(p_payload)); }
    };
    return c_ws;
}


function fn_9008(p_ws, p_en) {
    c_systemCommands.onSystemMessage({
        ty: 's', mt: 9008,
        ms: { en: p_en, socket1: { address: '0.0.0.0', port: 0 }, socket2: { address: '0.0.0.0', port: 0 } }
    }, p_ws);
}


function fn_rows(p_name) {
    return c_udp.fn_getActiveProxiesList().filter(function (r) { return r.name === p_name; });
}


function fn_setup() {
    installFakeGlobals({ public_host: 'comm.test.local', allow_udpproxy_fixed_port: false });
}


function fn_teardown(p_names) {
    for (const [c_name, c_account] of p_names) c_udp.closeUDPSocket(c_name, function () {}, c_account);
    restoreGlobals();
}


test('same unit name in two accounts gets two pairs; closing one leaves the other ready', async function () {
    fn_setup();
    try {
        const c_a = fn_makeDrone('drone1', 'accA');
        const c_b = fn_makeDrone('drone1', 'accB');
        fn_9008(c_a, true);
        assert.ok(await fn_waitFor(function () { return c_a.sent.length === 1; }));
        fn_9008(c_b, true);
        assert.ok(await fn_waitFor(function () { return c_b.sent.length === 1; }));

        const c_msA = c_a.sent[0].ms;
        const c_msB = c_b.sent[0].ms;
        assert.strictEqual(c_msA.en, true);
        assert.strictEqual(c_msB.en, true);
        const c_ports = [c_msA.socket1.port, c_msA.socket2.port, c_msB.socket1.port, c_msB.socket2.port];
        assert.strictEqual(new Set(c_ports).size, 4, 'B did not reuse A\'s sockets');

        const c_rows = fn_rows('drone1');
        assert.deepStrictEqual(c_rows.map(function (r) { return r.account; }).sort(), ['accA', 'accB']);

        // B goes away (en:false): A's pair stays and stays ready
        fn_9008(c_b, false);
        assert.ok(await fn_waitFor(function () { return c_b.sent.length === 2; }));
        assert.strictEqual(c_b.sent[1].ms.en, false);
        const c_left = fn_rows('drone1');
        assert.strictEqual(c_left.length, 1);
        assert.strictEqual(c_left[0].account, 'accA');
        assert.strictEqual(c_left[0].ready, true);
        assert.strictEqual(c_left[0].socket1.port, c_msA.socket1.port);

        // A asking again reuses its own pair
        fn_9008(c_a, true);
        assert.ok(await fn_waitFor(function () { return c_a.sent.length === 2; }));
        assert.strictEqual(c_a.sent[1].ms.socket1.port, c_msA.socket1.port);
    } finally {
        fn_teardown([['drone1', 'accA'], ['drone1', 'accB']]);
    }
});


/**
 * Calls the AUTH `g` handler and returns its REPORT_UDP_PROXIES reply.
 */
function fn_g(p_d) {
    const c_manager = require('../../src/comm_server/js_comm_server_manager_client.js');
    const c_commServer = require('../../src/comm_server/js_andruav_comm_server.js');
    const c_orig = c_manager.fn_sendMessage;
    const c_sent = [];
    c_manager.fn_sendMessage = function (p_msg) { c_sent.push(JSON.parse(p_msg)); };
    try {
        c_commServer.fn_handleSetUdpProxyState({ c: 'g', d: p_d });
    } finally {
        c_manager.fn_sendMessage = c_orig;
    }
    assert.strictEqual(c_sent.length, 1);
    assert.strictEqual(c_sent[0].c, 'f');
    return c_sent[0].d;
}


test('g with {name, account} blocks only that account\'s pair; rows carry account', async function () {
    fn_setup();
    try {
        const c_a = fn_makeDrone('drone2', 'accA');
        const c_b = fn_makeDrone('drone2', 'accB');
        fn_9008(c_a, true);
        fn_9008(c_b, true);
        assert.ok(await fn_waitFor(function () { return c_a.sent.length === 1 && c_b.sent.length === 1; }));

        const c_reply = fn_g({ name: 'drone2', account: 'accB', blocked: true, r: 'rq1' });
        assert.strictEqual(c_reply.applied, true);
        assert.strictEqual(c_reply.account, 'accB');
        assert.strictEqual(c_reply.r, 'rq1');
        const c_byAccount = {};
        c_reply.proxies.filter(function (r) { return r.name === 'drone2'; }).forEach(function (r) { c_byAccount[r.account] = r.blocked; });
        assert.deepStrictEqual(c_byAccount, { accA: false, accB: true });

        // unknown account -> nothing applied
        assert.strictEqual(fn_g({ name: 'drone2', account: 'accC', blocked: true }).applied, false);
    } finally {
        fn_teardown([['drone2', 'accA'], ['drone2', 'accB']]);
    }
});


test('g with only a name that exists in two accounts blocks neither (applied:false); a unique name still works', async function () {
    fn_setup();
    try {
        const c_a = fn_makeDrone('drone3', 'accA');
        const c_b = fn_makeDrone('drone3', 'accB');
        const c_solo = fn_makeDrone('solo', 'accA');
        fn_9008(c_a, true);
        fn_9008(c_b, true);
        fn_9008(c_solo, true);
        assert.ok(await fn_waitFor(function () { return c_a.sent.length === 1 && c_b.sent.length === 1 && c_solo.sent.length === 1; }));

        const c_ambiguous = fn_g({ name: 'drone3', blocked: true });
        assert.strictEqual(c_ambiguous.applied, false);
        assert.ok(fn_rows('drone3').every(function (r) { return r.blocked === false; }));

        // single-account behaviour unchanged: a name alone blocks its one pair
        const c_unique = fn_g({ name: 'solo', blocked: true });
        assert.strictEqual(c_unique.applied, true);
        assert.strictEqual(fn_rows('solo')[0].blocked, true);
    } finally {
        fn_teardown([['drone3', 'accA'], ['drone3', 'accB'], ['solo', 'accA']]);
    }
});
