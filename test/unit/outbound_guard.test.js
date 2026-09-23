"use strict";

/**
 * Unit tests for the outbound backpressure guard in
 * comm_server/chat_server/js_outbound_guard.js
 *
 * Bands per socket (defaults):
 *   buffered < high_water_bytes          -> send everything
 *   high_water .. hard_cap_bytes         -> send must_deliver, skip latest_wins
 *   buffered >= hard_cap_bytes           -> terminate socket
 */

const test = require('node:test');
const assert = require('node:assert');

const { installFakeGlobals, restoreGlobals } = require('../helpers/test_globals.js');
const c_guard = require('../../src/comm_server/chat_server/js_outbound_guard.js');

const HIGH_WATER = 262144;
const HARD_CAP = 4194304;
const TELEMETRY_MT = 1002; // configured as latest_wins in setup()


function setup(configOverrides) {
    const c_harness = installFakeGlobals(Object.assign({
        outbound_guard: {
            enabled: true,
            high_water_bytes: HIGH_WATER,
            hard_cap_bytes: HARD_CAP,
            latest_wins_message_types: [1002, 6502]
        }
    }, configOverrides || {}));
    c_guard.fn_reloadConfig();
    return c_harness;
}

function teardown() {
    restoreGlobals();
    c_guard.fn_reloadConfig();
}

/**
 * Fake ws socket with a settable bufferedAmount.
 */
function makeSocket(bufferedAmount) {
    const ws = {
        name: 'unit1',
        bufferedAmount: bufferedAmount,
        m_status: { m_TTX: 0, m_BTX: 0 },
        sent: [],
        terminated: false,
        send: function (payload, opts) { ws.sent.push({ payload: payload, opts: opts }); },
        terminate: function () { ws.terminated = true; }
    };
    return ws;
}


test('class: unknown mt is must_deliver, configured mt is latest_wins', () => {
    setup();
    try {
        assert.equal(c_guard.fn_getMessageClass(1002), c_guard.CLASS_LATEST_WINS);
        assert.equal(c_guard.fn_getMessageClass(6502), c_guard.CLASS_LATEST_WINS);
        assert.equal(c_guard.fn_getMessageClass(1005), c_guard.CLASS_MUST_DELIVER); // RemoteExecute
        assert.equal(c_guard.fn_getMessageClass(999999), c_guard.CLASS_MUST_DELIVER);
    } finally {
        teardown();
    }
});


test('below high water: both classes are sent', () => {
    setup();
    try {
        const ws = makeSocket(HIGH_WATER - 1);
        assert.equal(c_guard.fn_safeSend(ws, 'm1', false, c_guard.CLASS_LATEST_WINS), 'sent');
        assert.equal(c_guard.fn_safeSend(ws, 'm2', false, c_guard.CLASS_MUST_DELIVER), 'sent');
        assert.equal(ws.sent.length, 2);
    } finally {
        teardown();
    }
});


test('between high water and hard cap: latest_wins skipped, must_deliver sent', () => {
    setup();
    try {
        const ws = makeSocket(HIGH_WATER + 1);
        assert.equal(c_guard.fn_safeSend(ws, 'tele', false, c_guard.CLASS_LATEST_WINS), 'skipped');
        assert.equal(c_guard.fn_safeSend(ws, 'cmd', false, c_guard.CLASS_MUST_DELIVER), 'sent');
        assert.equal(ws.sent.length, 1);
        assert.equal(ws.sent[0].payload, 'cmd');
        assert.equal(ws.m_status.m_dropped, 1);
    } finally {
        teardown();
    }
});


test('must_deliver is never skipped below hard cap', () => {
    setup();
    try {
        const ws = makeSocket(HARD_CAP - 1);
        assert.equal(c_guard.fn_safeSend(ws, 'cmd', false, c_guard.CLASS_MUST_DELIVER), 'sent');
        assert.equal(ws.sent.length, 1);
        assert.equal(ws.terminated, false);
    } finally {
        teardown();
    }
});


test('missing class argument defaults to must_deliver', () => {
    setup();
    try {
        const ws = makeSocket(HIGH_WATER + 1);
        assert.equal(c_guard.fn_safeSend(ws, 'cmd', false), 'sent');
        assert.equal(ws.sent.length, 1);
    } finally {
        teardown();
    }
});


test('at/above hard cap: socket terminated for both classes', () => {
    setup();
    try {
        const ws = makeSocket(HARD_CAP);
        assert.equal(c_guard.fn_safeSend(ws, 'cmd', false, c_guard.CLASS_MUST_DELIVER), 'terminated');
        assert.equal(ws.terminated, true);
        assert.equal(ws.sent.length, 0);
        assert.equal(ws.m_status.m_terminated_for_backlog, 1);

        const ws2 = makeSocket(HARD_CAP + 100000);
        assert.equal(c_guard.fn_safeSend(ws2, 'tele', false, c_guard.CLASS_LATEST_WINS), 'terminated');
        assert.equal(ws2.terminated, true);
    } finally {
        teardown();
    }
});


test('terminate() throwing is swallowed and still returns terminated', () => {
    setup();
    try {
        const ws = makeSocket(HARD_CAP);
        ws.terminate = function () { throw new Error('dead'); };
        assert.equal(c_guard.fn_safeSend(ws, 'cmd', false, c_guard.CLASS_MUST_DELIVER), 'terminated');
    } finally {
        teardown();
    }
});


test('socket without bufferedAmount is treated as empty (sent)', () => {
    setup();
    try {
        const ws = makeSocket(0);
        delete ws.bufferedAmount;
        assert.equal(c_guard.fn_safeSend(ws, 'm', false, c_guard.CLASS_LATEST_WINS), 'sent');
        assert.equal(ws.sent.length, 1);
    } finally {
        teardown();
    }
});


test('socket without m_status still skips/terminates without crashing', () => {
    setup();
    try {
        const ws = makeSocket(HIGH_WATER + 1);
        delete ws.m_status;
        assert.equal(c_guard.fn_safeSend(ws, 'tele', false, c_guard.CLASS_LATEST_WINS), 'skipped');

        const ws2 = makeSocket(HARD_CAP);
        delete ws2.m_status;
        assert.equal(c_guard.fn_safeSend(ws2, 'cmd', false, c_guard.CLASS_MUST_DELIVER), 'terminated');
    } finally {
        teardown();
    }
});


test('disabled guard sends everything regardless of bufferedAmount', () => {
    setup({ outbound_guard: { enabled: false } });
    try {
        const ws = makeSocket(HARD_CAP * 10);
        assert.equal(c_guard.fn_safeSend(ws, 'tele', false, c_guard.CLASS_LATEST_WINS), 'sent');
        assert.equal(ws.sent.length, 1);
        assert.equal(ws.terminated, false);
    } finally {
        teardown();
    }
});


test('missing outbound_guard config uses defaults (enabled, cap enforced)', () => {
    installFakeGlobals({});
    c_guard.fn_reloadConfig();
    try {
        const ws = makeSocket(HARD_CAP + 1);
        assert.equal(c_guard.fn_safeSend(ws, 'cmd', false, c_guard.CLASS_MUST_DELIVER), 'terminated');
        // no latest_wins types configured -> everything is must_deliver
        assert.equal(c_guard.fn_getMessageClass(1002), c_guard.CLASS_MUST_DELIVER);
    } finally {
        teardown();
    }
});
