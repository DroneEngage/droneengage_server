"use strict";

/**
 * Unit tests for src/comm_server/js_andruav_comm_server.js
 *
 * Covers fn_cancelLoginRequestBySenderID, which referenced the undefined
 * variable `c_CommServerManagerClient` (the module is imported as
 * `m_commServerManagerClient`) and would throw a ReferenceError when the
 * request id matched a waiting account.
 */

const test = require('node:test');
const assert = require('node:assert');

const { installFakeGlobals, restoreGlobals } = require('../helpers/test_globals.js');

const c_CONSTANTS = require('../../src/js_constants.js');

function setup() {
    installFakeGlobals({});
    return require('../../src/comm_server/js_andruav_comm_server.js');
}

function teardown() {
    restoreGlobals();
}

test('fn_cancelLoginRequestBySenderID removes matching login without ReferenceError', () => {
    const c_srv = setup();
    try {
        const c_requestID = 'req-123';
        const c_loginRequest = {};
        c_loginRequest[c_CONSTANTS.CONST_CS_REQUEST_ID.toString()] = c_requestID;

        c_srv.fn_addWaitingAccount('tempkey1', c_loginRequest);
        assert.equal(c_srv.isLoginExist('tempkey1'), true);

        assert.doesNotThrow(() => c_srv.fn_cancelLoginRequestBySenderID(c_requestID));
        assert.equal(c_srv.isLoginExist('tempkey1'), false, 'waiting login should be removed');
    } finally {
        teardown();
    }
});

test('fn_cancelLoginRequestBySenderID leaves non-matching logins intact', () => {
    const c_srv = setup();
    try {
        const c_loginRequest = {};
        c_loginRequest[c_CONSTANTS.CONST_CS_REQUEST_ID.toString()] = 'req-other';

        c_srv.fn_addWaitingAccount('tempkey2', c_loginRequest);

        assert.doesNotThrow(() => c_srv.fn_cancelLoginRequestBySenderID('req-no-match'));
        assert.equal(c_srv.isLoginExist('tempkey2'), true, 'unrelated login must stay');
    } finally {
        teardown();
    }
});

test('fn_cancelLoginRequestBySenderID ignores null request id', () => {
    const c_srv = setup();
    try {
        assert.doesNotThrow(() => c_srv.fn_cancelLoginRequestBySenderID(null));
    } finally {
        teardown();
    }
});
