"use strict";

/**
 * Unit tests for comm_server/chat_server/js_chat_news.js
 *
 * Mirrors test/unit/tasks.test.js's approach: guard behavior (no backend /
 * missing accountID) plus the NewsPush fan-out helper, all with a stubbed
 * DBProxyClient and js_andruav_chat_account_rooms.
 */

const test = require('node:test');
const assert = require('node:assert');

const { installFakeGlobals, restoreGlobals, makeFakeWs } = require('../helpers/test_globals.js');
const c_CONSTANTS = require('../../src/js_constants.js');

const c_news = require('../../src/comm_server/chat_server/js_chat_news.js');
const c_dbProxyClient = require('../../src/comm_server/server_to_server/js_db_proxy_client.js');
const c_ChatAccountRooms = require('../../src/comm_server/chat_server/js_andruav_chat_account_rooms.js');


test.afterEach(() => {
    restoreGlobals();
    c_dbProxyClient.fn_isConnected = () => false;
    c_dbProxyClient.fn_sendRequest = () => Promise.reject(new Error('DBProxyClient is not connected to a storage server'));
});


function makeNewsWs(accountId, opts) {
    const ws = makeFakeWs(opts);
    ws.m_loginRequest.m_accountID = accountId;
    return ws;
}


// ---------------------------------------------------------------------------
// Handler guards (no DB backend initialized)
// ---------------------------------------------------------------------------

test('fn_handleLoadNews replies with storage-not-connected error when DB proxy is offline', () => {
    installFakeGlobals({});
    const ws = makeNewsWs('acc1', { name: 'unit1' });
    c_news.fn_handleLoadNews({ ms: {} }, ws);
    assert.strictEqual(ws.sent.length, 1);
    const reply = JSON.parse(ws.sent[0]);
    assert.strictEqual(reply.mt, c_CONSTANTS.CONST_TYPE_AndruavSystem_LoadNews);
    assert.ok(reply.ms.includes('Storage server not connected'));
});

test('fn_handleLoadNews is a no-op when accountID is missing', () => {
    installFakeGlobals({});
    const ws = makeFakeWs({ name: 'unit1' }); // no m_accountID
    assert.doesNotThrow(() => c_news.fn_handleLoadNews({ ms: {} }, ws));
    assert.strictEqual(ws.sent.length, 0);
});

test('fn_handleSaveNews replies with storage-not-connected error when DB proxy is offline', () => {
    installFakeGlobals({});
    const ws = makeNewsWs('acc1', { name: 'unit1' });
    c_news.fn_handleSaveNews({ ms: { body: 'hello' } }, ws);
    assert.strictEqual(ws.sent.length, 1);
    const reply = JSON.parse(ws.sent[0]);
    assert.strictEqual(reply.mt, c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveNews);
    assert.ok(reply.ms.includes('Storage server not connected'));
});

test('fn_handleSaveNews is a no-op when body is missing/empty', () => {
    installFakeGlobals({});
    const ws = makeNewsWs('acc1', { name: 'unit1' });
    assert.doesNotThrow(() => c_news.fn_handleSaveNews({ ms: { body: '' } }, ws));
    assert.strictEqual(ws.sent.length, 0);
});

test('fn_handleSaveNews always forces scope to "account" (regular GCS write path)', async () => {
    installFakeGlobals({});
    let captured = null;
    c_dbProxyClient.fn_isConnected = () => true;
    c_dbProxyClient.fn_saveNews = (scope, accountId, title, body, priority, authorId, expiresAt, newsId) => {
        captured = { scope, accountId, title, body, priority, authorId, expiresAt, newsId };
        return Promise.resolve({ success: true, ms: { newsId: 'n1' } });
    };

    const ws = makeNewsWs('acc1', { name: 'unit1', senderID: 'unit1' });
    // Attempting to sneak scope='global' in the payload must be ignored.
    c_news.fn_handleSaveNews({ ms: { scope: 'global', title: 't', body: 'hello', priority: 1 } }, ws);
    await new Promise(resolve => setImmediate(resolve));

    assert.ok(captured, 'request was forwarded to storage server');
    assert.strictEqual(captured.scope, c_CONSTANTS.CONST_NEWS_SCOPE_ACCOUNT);
    assert.strictEqual(captured.accountId, 'acc1');
    assert.strictEqual(captured.authorId, 'unit1');

    assert.strictEqual(ws.sent.length, 1);
    const reply = JSON.parse(ws.sent[0]);
    assert.strictEqual(reply.mt, c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveNews);
    assert.strictEqual(reply.ms.newsId, 'n1');
});

test('fn_handleDeleteNews replies with storage-not-connected error when DB proxy is offline', () => {
    installFakeGlobals({});
    const ws = makeNewsWs('acc1', { name: 'unit1' });
    c_news.fn_handleDeleteNews({ ms: { newsId: 'n1' } }, ws);
    assert.strictEqual(ws.sent.length, 1);
    const reply = JSON.parse(ws.sent[0]);
    assert.strictEqual(reply.mt, c_CONSTANTS.CONST_TYPE_AndruavSystem_DeleteNews);
    assert.ok(reply.ms.includes('Storage server not connected'));
});

test('fn_handleDeleteNews is a no-op when newsId is missing', () => {
    installFakeGlobals({});
    const ws = makeNewsWs('acc1', { name: 'unit1' });
    assert.doesNotThrow(() => c_news.fn_handleDeleteNews({ ms: {} }, ws));
    assert.strictEqual(ws.sent.length, 0);
});


// ---------------------------------------------------------------------------
// fn_onNewsPush fan-out
// ---------------------------------------------------------------------------

test('fn_onNewsPush broadcasts global news to all GCS everywhere', () => {
    const calls = [];
    const originalAll = c_ChatAccountRooms.fn_sendToAllGCS;
    const originalAccount = c_ChatAccountRooms.fn_sendToAllGCSInAccount;
    c_ChatAccountRooms.fn_sendToAllGCS = (...args) => calls.push({ fn: 'all', args });
    c_ChatAccountRooms.fn_sendToAllGCSInAccount = (...args) => calls.push({ fn: 'account', args });

    try {
        c_news.fn_onNewsPush({ news: { id: 'n1', scope: c_CONSTANTS.CONST_NEWS_SCOPE_GLOBAL, body: 'hi' } });
        assert.strictEqual(calls.length, 1);
        assert.strictEqual(calls[0].fn, 'all');
    } finally {
        c_ChatAccountRooms.fn_sendToAllGCS = originalAll;
        c_ChatAccountRooms.fn_sendToAllGCSInAccount = originalAccount;
    }
});

test('fn_onNewsPush broadcasts account news only within that account', () => {
    const calls = [];
    const originalAll = c_ChatAccountRooms.fn_sendToAllGCS;
    const originalAccount = c_ChatAccountRooms.fn_sendToAllGCSInAccount;
    c_ChatAccountRooms.fn_sendToAllGCS = (...args) => calls.push({ fn: 'all', args });
    c_ChatAccountRooms.fn_sendToAllGCSInAccount = (...args) => calls.push({ fn: 'account', args });

    try {
        c_news.fn_onNewsPush({ news: { id: 'n1', scope: c_CONSTANTS.CONST_NEWS_SCOPE_ACCOUNT, account_id: 'acc1', body: 'hi' } });
        assert.strictEqual(calls.length, 1);
        assert.strictEqual(calls[0].fn, 'account');
        assert.strictEqual(calls[0].args[3], 'acc1');
    } finally {
        c_ChatAccountRooms.fn_sendToAllGCS = originalAll;
        c_ChatAccountRooms.fn_sendToAllGCSInAccount = originalAccount;
    }
});

test('fn_onNewsPush falls back to broadcasting everywhere for a disable/delete with no scope', () => {
    const calls = [];
    const originalAll = c_ChatAccountRooms.fn_sendToAllGCS;
    c_ChatAccountRooms.fn_sendToAllGCS = (...args) => calls.push(args);

    try {
        c_news.fn_onNewsPush({ news: { id: 'n1', disabled: 1 } });
        assert.strictEqual(calls.length, 1);
    } finally {
        c_ChatAccountRooms.fn_sendToAllGCS = originalAll;
    }
});
