"use strict";

/**
 * Unit tests for server/chat_server/js_chat_tasks.js
 *
 * Focus is the de-duplicated helpers that replaced the four near-identical
 * task command blocks, plus the early-return guards of the handlers.
 *
 * NOTE: A live DB backend is not exercised here. The handlers are tested only
 * for their guard behavior (no backend / missing accountID). Backend-driven
 * paths will be covered by integration tests once DI is introduced (Phase 2).
 */

const test = require('node:test');
const assert = require('node:assert');

const { installFakeGlobals, restoreGlobals, makeFakeWs } = require('../helpers/test_globals.js');
const c_CONSTANTS = require('../../js_constants.js');

const c_tasks = require('../../server/chat_server/js_chat_tasks.js');


test.afterEach(() => restoreGlobals());


// ---------------------------------------------------------------------------
// fn_parseMessageBody
// ---------------------------------------------------------------------------

test('fn_parseMessageBody returns object body unchanged', () => {
    const body = { ai: 'acc@x.com', mt: 1 };
    assert.strictEqual(c_tasks.fn_parseMessageBody({ ms: body }), body);
});

test('fn_parseMessageBody parses a stringified body (backward compatible)', () => {
    const body = { ai: 'acc@x.com', mt: 1 };
    const result = c_tasks.fn_parseMessageBody({ ms: JSON.stringify(body) });
    assert.deepStrictEqual(result, body);
});


// ---------------------------------------------------------------------------
// fn_fillTaskParams
// ---------------------------------------------------------------------------

test('fn_fillTaskParams maps all shared optional fields', () => {
    const mms = { ac: 'code', ai: 'acc', ps: 'p1', gn: 'grp', s: 'snd', r: 'rcv', mt: 7, ip: true };
    const params = {};
    c_tasks.fn_fillTaskParams(mms, params, {});

    assert.strictEqual(params.accessCode, 'code');
    assert.strictEqual(params.accountID, 'acc');
    assert.strictEqual(params.party_sid, 'p1');
    assert.strictEqual(params.groupName, 'grp');
    assert.strictEqual(params.sender, 'snd');
    assert.strictEqual(params.receiver, 'rcv');
    assert.strictEqual(params.messageType, 7);
    assert.strictEqual(params.isPermanent, true);
});

test('fn_fillTaskParams omits fields that are absent', () => {
    const params = {};
    c_tasks.fn_fillTaskParams({ ai: 'acc' }, params, {});
    assert.deepStrictEqual(Object.keys(params), ['accountID']);
});

test('fn_fillTaskParams includes largerThan_SID only when includeLts is set', () => {
    const withLts = {};
    c_tasks.fn_fillTaskParams({ lts: 42 }, withLts, { includeLts: true });
    assert.strictEqual(withLts.largerThan_SID, 42);

    const withoutLts = {};
    c_tasks.fn_fillTaskParams({ lts: 42 }, withoutLts, {});
    assert.strictEqual(withoutLts.largerThan_SID, undefined);
});

test('fn_fillTaskParams stringifies task only when includeTask is set', () => {
    const taskObj = { a: 1, b: [2, 3] };

    const withTask = {};
    c_tasks.fn_fillTaskParams({ t: taskObj }, withTask, { includeTask: true });
    assert.strictEqual(withTask.task, JSON.stringify(taskObj));

    const withoutTask = {};
    c_tasks.fn_fillTaskParams({ t: taskObj }, withoutTask, {});
    assert.strictEqual(withoutTask.task, undefined);
});


// ---------------------------------------------------------------------------
// fn_makeDoneResultFunc
// ---------------------------------------------------------------------------

test('fn_makeDoneResultFunc (length mode) sends "Done" reply when rows present', () => {
    const ws = makeFakeWs({ name: 'unit1' });
    const v_jmsg = {};
    const params = c_tasks.fn_makeDoneResultFunc(ws, v_jmsg, c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveTasks, false);

    params.resultfunc([{ SID: 1 }]); // non-empty

    assert.strictEqual(ws.sent.length, 1);
    const reply = JSON.parse(ws.sent[0]);
    assert.strictEqual(reply.ty, c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL);
    assert.strictEqual(reply.tg, 'unit1');
    assert.strictEqual(reply.sd, c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER);
    assert.strictEqual(reply.mt, c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveTasks);
    assert.strictEqual(reply.ms, 'Done');
});

test('fn_makeDoneResultFunc (length mode) sends nothing when result is empty', () => {
    const ws = makeFakeWs({ name: 'unit1' });
    const params = c_tasks.fn_makeDoneResultFunc(ws, {}, '9003', false);

    params.resultfunc([]); // empty

    assert.strictEqual(ws.sent.length, 0);
});

test('fn_makeDoneResultFunc (affectedRows mode) uses res.affectedRows', () => {
    const ws = makeFakeWs({ name: 'unit1' });
    const params = c_tasks.fn_makeDoneResultFunc(ws, {}, '9003', true);

    params.resultfunc({ affectedRows: 0 });
    assert.strictEqual(ws.sent.length, 0, 'no reply when affectedRows == 0');

    params.resultfunc({ affectedRows: 3 });
    assert.strictEqual(ws.sent.length, 1, 'reply when affectedRows > 0');
    assert.strictEqual(JSON.parse(ws.sent[0]).mt, '9003');
});


// ---------------------------------------------------------------------------
// Handler guards (no DB backend initialized)
// ---------------------------------------------------------------------------

test('fn_handleLoadTasks returns without replying when tasks backend is not initialized', () => {
    installFakeGlobals({});
    const ws = makeFakeWs({ name: 'unit1' });
    c_tasks.fn_handleLoadTasks({ ms: { ai: 'acc@x.com' } }, ws);
    assert.strictEqual(ws.sent.length, 0);
});

test('fn_handleSaveTasks returns without replying when tasks backend is not initialized', () => {
    installFakeGlobals({});
    const ws = makeFakeWs({ name: 'unit1' });
    c_tasks.fn_handleSaveTasks({ ms: { ai: 'acc@x.com' } }, ws);
    assert.strictEqual(ws.sent.length, 0);
});

test('fn_handleDeleteTasks returns without replying when tasks backend is not initialized', () => {
    installFakeGlobals({});
    const ws = makeFakeWs({ name: 'unit1' });
    c_tasks.fn_handleDeleteTasks({ ms: { ai: 'acc@x.com' } }, ws);
    assert.strictEqual(ws.sent.length, 0);
});

test('task handlers ignore requests with a missing/empty accountID (ai)', () => {
    installFakeGlobals({});
    const ws = makeFakeWs({ name: 'unit1' });
    // No ai -> early return, no throw, no reply (DisableTasks guards on ai before touching backend).
    assert.doesNotThrow(() => c_tasks.fn_handleDisableTasks({ ms: { mt: 1 } }, ws));
    assert.strictEqual(ws.sent.length, 0);
});

// Characterization test: documents a PRE-EXISTING behavior preserved by the split.
// fn_handleDisableTasks lacks the `v_andruavTasks == null` guard that the other
// handlers have, so with a valid accountID but no backend it throws.
// Phase 2 (DI/async refactor) should add the guard and replace this assertion.
test('[characterization] fn_handleDisableTasks throws when ai present but backend uninitialized', () => {
    installFakeGlobals({});
    const ws = makeFakeWs({ name: 'unit1' });
    assert.throws(() => c_tasks.fn_handleDisableTasks({ ms: { ai: 'acc@x.com' } }, ws));
});
