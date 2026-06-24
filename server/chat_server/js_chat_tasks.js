"use strict";

/**
 * Task command module.
 * Handles LoadTasks / SaveTasks / DeleteTasks / DisableTasks system commands.
 * Extracted from js_andruav_chat_server.js (behavior-preserving) with the
 * duplicated parameter-parsing and "Done" reply blocks factored out.
 * Owns the v_andruavTasks instance and its initialization.
 */

const c_dumpError = require("../../dumperror.js");
const c_CONSTANTS = require("../../js_constants.js");


let v_andruavTasks = null;


function fn_initTasks() {
    if (global.m_serverconfig.m_configuration.ignoreLoadingTasks === false) {
        v_andruavTasks = require("../js_andruavTasks_v2.js");
        v_andruavTasks.fn_initTasks();
    }
}


/**
 * Normalizes v_jmsg.ms into an object (backward compatible with stringified bodies).
 */
function fn_parseMessageBody(v_jmsg) {
    let mms = v_jmsg.ms;
    if (typeof mms === 'string' || mms instanceof String) {
        // backword compatible
        mms = JSON.parse(v_jmsg.ms); //Internal p_message JSON
    }
    return mms;
}


/**
 * Copies the optional task-query fields shared by every task command.
 * @param {*} mms parsed message body
 * @param {*} v_params target params object
 * @param {{includeLts?:boolean, includeTask?:boolean}} p_opts
 */
function fn_fillTaskParams(mms, v_params, p_opts) {
    p_opts = p_opts || {};
    if (p_opts.includeLts === true && mms.hasOwnProperty("lts")) v_params.largerThan_SID = mms.lts;
    if (mms.hasOwnProperty("ac")) v_params.accessCode = mms.ac;
    if (mms.hasOwnProperty("ai")) v_params.accountID = mms.ai;
    if (mms.hasOwnProperty("ps")) v_params.party_sid = mms.ps;
    if (mms.hasOwnProperty("gn")) v_params.groupName = mms.gn;
    if (mms.hasOwnProperty("s")) v_params.sender = mms.s;
    if (mms.hasOwnProperty("r")) v_params.receiver = mms.r;
    if (mms.hasOwnProperty("mt")) v_params.messageType = mms.mt;
    if (mms.hasOwnProperty("ip")) v_params.isPermanent = mms.ip;
    if (p_opts.includeTask === true && mms.hasOwnProperty("t")) v_params.task = JSON.stringify(mms.t);
}


/**
 * Builds the standard single-"Done" resultfunc/errfunc pair used by
 * Save / Delete / Disable commands.
 * @param {*} p_ws
 * @param {*} v_jmsg
 * @param {*} p_messageType value assigned to v_jmsg.mt in the reply
 * @param {boolean} p_useAffectedRows true to test res.affectedRows, false to test res.length
 */
function fn_makeDoneResultFunc(p_ws, v_jmsg, p_messageType, p_useAffectedRows) {
    return {
        resultfunc: function (res) {
            const c_count = p_useAffectedRows === true ? res.affectedRows : res.length;
            if (c_count == 0) {
                // no data
                c_dumpError.fn_dumpdebug("Data Found:" + c_count);
            }
            else {
                // data found
                c_dumpError.fn_dumpdebug((p_useAffectedRows === true ? "Rows Length:" : "Rows Length:") + c_count);
                v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
                v_jmsg.tg = p_ws.name; // sender = target
                v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                v_jmsg.mt = p_messageType;
                v_jmsg.ms = "Done";
                p_ws.send(JSON.stringify(v_jmsg));
            }
        },
        errfunc: function (err) {
            c_dumpError.fn_dumperror(err);
        }
    };
}


function fn_handleLoadTasks(v_jmsg, p_ws) {
    if (v_andruavTasks == null) return;
    c_dumpError.fn_dumpdebug("load tasks command");
    let v_params = {
        resultfunc: function (res) {
            if (res.length == 0) {
                // no data
                c_dumpError.fn_dumpdebug("Data Found:" + res.length);
            }
            else {
                // data found
                c_dumpError.fn_dumpdebug("Rows Length:" + res.length);
                v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_INDIVIDUAL;
                v_jmsg.tg = p_ws.name; // sender = target
                v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
                for (let i = 0; i < res.length; i++) {
                    v_jmsg.sid = res[i].SID; // add SID to p_message to point to a task.
                    v_jmsg.ms = res[i].task;
                    v_jmsg.mt = res[i].messageType; // rechange command to the saved command.
                    p_ws.send(JSON.stringify(v_jmsg));
                }
            }
        },
        errfunc: function (err) {
            c_dumpError.fn_dumperror(err);
        }
    };

    const mms = fn_parseMessageBody(v_jmsg);

    if ((mms.ai == null) || (mms.ai.length == 0)) {
        return; // error List all crosee accounts tasks ... ERROR
    }
    c_dumpError.fn_dumpdebug(mms);
    c_dumpError.fn_dumpdebug(mms.messageType);
    //{resultfunc,errfunc,largerThan_SID, party_sid,sender,receiver,messageType,task,isPermanent}
    fn_fillTaskParams(mms, v_params, { includeLts: true });
    c_dumpError.fn_dumpdebug(v_params);
    v_andruavTasks.fn_get_tasks(v_params);
}


function fn_handleSaveTasks(v_jmsg, p_ws) {
    if (v_andruavTasks == null) return;
    c_dumpError.fn_dumpdebug("save tasks command");
    let v_params = fn_makeDoneResultFunc(p_ws, v_jmsg, c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveTasks, false);

    const mms = fn_parseMessageBody(v_jmsg);
    c_dumpError.fn_dumpdebug(mms);
    if ((mms.ai == null) || (mms.ai.length == 0)) {
        return; // error List all crosee accounts tasks ... ERROR
    }

    fn_fillTaskParams(mms, v_params, { includeTask: true });
    c_dumpError.fn_dumpdebug(v_params);
    v_andruavTasks.fn_add_task(v_params);
}


function fn_handleDeleteTasks(v_jmsg, p_ws) {
    if (v_andruavTasks == null) return;
    c_dumpError.fn_dumpdebug("delete tasks command");
    let v_params = fn_makeDoneResultFunc(p_ws, v_jmsg, '9003', false);

    const mms = fn_parseMessageBody(v_jmsg);
    c_dumpError.fn_dumpdebug(mms);
    if ((mms.ai == null) || (mms.ai.length == 0)) {
        return; // error List all crosee accounts tasks ... ERROR
    }

    fn_fillTaskParams(mms, v_params, { includeTask: true });
    c_dumpError.fn_dumpdebug(v_params);
    //v_andruavTasks.delete_tasks(v_params);
}


function fn_handleDisableTasks(v_jmsg, p_ws) {
    c_dumpError.fn_dumpdebug("disable tasks command");
    let v_params = fn_makeDoneResultFunc(p_ws, v_jmsg, '9003', true);

    const mms = fn_parseMessageBody(v_jmsg);
    if ((mms.ai == null) || (mms.ai.length == 0)) {
        return; // No Global wide operation is allowed
    }

    c_dumpError.fn_dumpdebug(mms);
    fn_fillTaskParams(mms, v_params, { includeTask: true });
    c_dumpError.fn_dumpdebug(v_params);
    v_andruavTasks.fn_disable_tasks(v_params);
}


module.exports = {
    fn_initTasks,
    fn_handleLoadTasks,
    fn_handleSaveTasks,
    fn_handleDeleteTasks,
    fn_handleDisableTasks,
    // Exported for unit testing of the de-duplicated helpers.
    fn_parseMessageBody,
    fn_fillTaskParams,
    fn_makeDoneResultFunc
};
