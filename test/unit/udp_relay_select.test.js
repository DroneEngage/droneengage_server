"use strict";

/**
 * Several UDP relays (TASK-57): selection (hash / first_healthy /
 * least_loaded) and failover between two real relay processes.
 */

const test = require('node:test');
const assert = require('node:assert');

const { installFakeGlobals, restoreGlobals } = require('../helpers/test_globals.js');
const r = require('./relay_process_helper.js');

const c_relayClient = require('../../src/comm_server/server_to_server/js_udp_relay_client.js');
const c_systemCommands = require('../../src/comm_server/chat_server/js_chat_system_commands.js');
const c_activeSenders = require('../../src/comm_server/chat_server/js_andruav_active_senders.js');


function fn_units(p_n) {
    const c_out = [];
    for (let i = 0; i < p_n; ++i) c_out.push(['acc' + (i % 7), 'drone' + i]);
    return c_out;
}


test('hash: the same account/unit maps to the same relay, also after a fresh module load', function () {
    const c_ids = ['R1', 'R2', 'R3'];
    const c_first = fn_units(300).map(function ([a, u]) { return c_relayClient.fn_selectRelayId(a, u, c_ids, 'hash'); });

    // a comm server restart = a fresh module with no state
    const c_file = require.resolve('../../src/comm_server/server_to_server/js_udp_relay_client.js');
    const c_cached = require.cache[c_file];
    delete require.cache[c_file];
    const c_fresh = require(c_file);
    require.cache[c_file] = c_cached;
    assert.notStrictEqual(c_fresh, c_relayClient);
    const c_second = fn_units(300).map(function ([a, u]) { return c_fresh.fn_selectRelayId(a, u, c_ids, 'hash'); });
    assert.deepStrictEqual(c_second, c_first);

    // every relay gets a share
    for (const c_id of c_ids) {
        const c_n = c_first.filter(function (p) { return p === c_id; }).length;
        assert.ok(c_n > 50, `${c_id} got ${c_n}/300`);
    }
    // relay order in the list does not matter
    const c_shuffled = fn_units(300).map(function ([a, u]) { return c_relayClient.fn_selectRelayId(a, u, ['R3', 'R1', 'R2'], 'hash'); });
    assert.deepStrictEqual(c_shuffled, c_first);
});


test('hash: removing one relay moves only that relay\'s units', function () {
    const c_units = fn_units(300);
    const c_before = c_units.map(function ([a, u]) { return c_relayClient.fn_selectRelayId(a, u, ['R1', 'R2', 'R3'], 'hash'); });
    const c_after = c_units.map(function ([a, u]) { return c_relayClient.fn_selectRelayId(a, u, ['R1', 'R3'], 'hash'); });
    let v_moved = 0;
    c_before.forEach(function (p_was, i) {
        if (p_was === 'R2') {
            assert.ok(c_after[i] === 'R1' || c_after[i] === 'R3');
            v_moved += 1;
        } else {
            assert.strictEqual(c_after[i], p_was, 'a unit not on R2 must not move');
        }
    });
    assert.ok(v_moved > 50);
});


test('first_healthy and least_loaded', function () {
    assert.strictEqual(c_relayClient.fn_selectRelayId('a', 'u', ['R2', 'R1'], 'first_healthy'), 'R2');
    assert.strictEqual(c_relayClient.fn_selectRelayId('a', 'u', ['R1', 'R2', 'R3'], 'least_loaded', { R1: 9, R2: 3, R3: 5 }), 'R2');
    assert.strictEqual(c_relayClient.fn_selectRelayId('a', 'u', ['R1', 'R2'], 'least_loaded', { R1: 4, R2: 4 }), 'R1', 'ties in list order');
    assert.strictEqual(c_relayClient.fn_selectRelayId('a', 'u', ['R1', 'R2'], 'least_loaded', { R1: 4 }), 'R2', 'unknown load counts as 0');
    assert.strictEqual(c_relayClient.fn_selectRelayId('a', 'u', [], 'hash'), null);
});


/**
 * A unit name in p_account that hashes to p_want among p_ids.
 */
function fn_unitOn(p_account, p_want, p_ids, p_prefix) {
    for (let i = 0; i < 1000; ++i) {
        const c_name = p_prefix + i;
        if (c_relayClient.fn_selectRelayId(p_account, c_name, p_ids, 'hash') === p_want) return c_name;
    }
    throw new Error('no unit name maps to ' + p_want);
}


function fn_makeUnit(p_name) {
    const c_ws = {
        name: p_name,
        readyState: 1,
        m_loginRequest: { m_senderID: p_name, m_actorType: 'd', m_accountID: 'acc1', m_groupID: 'g1', m_prm: 0xffffffff },
        sent: [],
        send: function (p_payload) { c_ws.sent.push(JSON.parse(p_payload)); }
    };
    c_activeSenders.addActiveSenderIDList(p_name, c_ws);
    return c_ws;
}


function fn_9008(p_ws) {
    c_systemCommands.onSystemMessage({
        ty: 's', mt: 9008, ms: { en: true, socket1: { address: '0.0.0.0', port: 0 }, socket2: { address: '0.0.0.0', port: 0 } }
    }, p_ws);
}


test('two relays: killing relay 1 moves only its units to relay 2 (9008 en:true pushed); they stay when relay 1 recovers', { skip: r.CONST_SKIP_RELAY, timeout: 60000 }, async function () {
    process.env.de_s2s_shared_secret = r.CONST_SECRET;
    installFakeGlobals({ server_id: 'DE_CommSrv_Multi', public_host: 'comm.test.local', udp_relay_failover_s: 1, udp_relay_select: 'hash' });
    require('../../src/comm_server/js_s2s_auth.js').fn_resetKeyCache();
    const c_port1 = await r.fn_freeTcpPort();
    const c_port2 = await r.fn_freeTcpPort();
    let v_relay1 = await r.fn_startRelay(c_port1, { public_host: 'relay1.test.local' });
    const c_relay2 = await r.fn_startRelay(c_port2, { public_host: 'relay2.test.local' });
    global.m_serverconfig.m_configuration.udp_relay = [
        { host: '127.0.0.1', port: c_port1, id: 'R1', ssl: v_relay1.m_ssl },
        { host: '127.0.0.1', port: c_port2, id: 'R2', ssl: c_relay2.m_ssl }
    ];
    const c_ids = ['R1', 'R2'];
    const c_onR1 = [fn_unitOn('acc1', 'R1', c_ids, 'ua'), fn_unitOn('acc1', 'R1', c_ids, 'ux')];
    const c_onR2 = [fn_unitOn('acc1', 'R2', c_ids, 'ub')];
    const c_units = {};
    try {
        c_relayClient.fn_initialize();
        // both links up, so hash sees both relays
        assert.ok(await r.fn_waitFor(fn_bothUp, 5000), 'both relays authenticated');

        for (const c_name of c_onR1.concat(c_onR2)) {
            c_units[c_name] = fn_makeUnit(c_name);
            fn_9008(c_units[c_name]);
        }
        assert.ok(await r.fn_waitFor(function () {
            return Object.values(c_units).every(function (u) { return u.sent.length === 1; });
        }));
        for (const c_name of c_onR1) assert.strictEqual(c_units[c_name].sent[0].ms.socket1.address, 'relay1.test.local', c_name);
        for (const c_name of c_onR2) assert.strictEqual(c_units[c_name].sent[0].ms.socket1.address, 'relay2.test.local', c_name);

        // relay 1 dies: within udp_relay_failover_s (1 s) + 10 s its units get relay 2's sockets
        const c_t0 = Date.now();
        await v_relay1.stop();
        assert.ok(await r.fn_waitFor(function () {
            return c_onR1.every(function (n) { return c_units[n].sent.length === 2; });
        }, 11000), 'relay 1 units got an unsolicited 9008');
        assert.ok(Date.now() - c_t0 < 11000);
        const [c_lo2, c_hi2] = c_relay2.m_cfg.udp_port_range;
        for (const c_name of c_onR1) {
            const c_push = c_units[c_name].sent[1];
            assert.strictEqual(c_push.mt, 9008);
            assert.strictEqual(c_push.tg, c_name);
            assert.strictEqual(c_push.ms.en, true);
            assert.strictEqual(c_push.ms.socket1.address, 'relay2.test.local');
            assert.ok(c_push.ms.socket2.port >= c_lo2 && c_push.ms.socket2.port <= c_hi2);
        }
        for (const c_name of c_onR2) assert.strictEqual(c_units[c_name].sent.length, 1, c_name + ' (already on relay 2) got nothing');
        const c_rows2 = await c_relay2.list();
        assert.deepStrictEqual(c_rows2.map(function (x) { return x.unit; }).sort(), c_onR1.concat(c_onR2).sort());

        // relay 1 comes back: nothing moves back (no flapping)
        v_relay1 = await r.fn_startRelay(c_port1, { public_host: 'relay1.test.local' });
        assert.ok(await r.fn_waitFor(fn_bothUp, 15000), 'relay 1 reconnected');
        await r.fn_sleep(300);
        for (const c_row of c_relayClient.fn_getUnitPlacement()) assert.strictEqual(c_row.relay_id, 'R2', c_row.unit);
        for (const c_name of Object.keys(c_units)) {
            const c_expected = c_onR1.includes(c_name) ? 2 : 1;
            assert.strictEqual(c_units[c_name].sent.length, c_expected, c_name + ' got no further 9008');
        }
    } finally {
        Object.keys(c_units).forEach(function (n) { c_activeSenders.deleteActiveSenderIDList(n); });
        c_relayClient.fn_shutdown();
        restoreGlobals();
        await v_relay1.stop();
        await c_relay2.stop();
    }

    function fn_bothUp() {
        const c_states = c_relayClient.fn_getRelayStates();
        return c_states.length === 2 && c_states.every(function (p_s) { return p_s.connected; });
    }
});
