"use strict";

/**
 * Routes communication between different parties.
 * This is the Party-to-Party Module.
 *
 * NOTE: This file is now a thin facade/orchestrator. The implementation was
 * split (behavior-preserving) into focused modules:
 *   - js_chat_connection.js      : WebSocket lifecycle + server bootstrap
 *   - js_chat_routing.js         : message parsing & dispatch (local + relayed)
 *   - js_chat_system_commands.js : system command handling
 *   - js_chat_tasks.js           : task commands (de-duplicated)
 *   - js_chat_relay.js           : server-to-server mesh forwarding
 *
 * The public API (fn_startServer, fn_parseExternalMessage) is preserved for
 * server.js (singleton) and the parent/child comm-server relay transports.
 */

const c_PluginManager = require('./pluginManager.js');
const c_connection = require('./js_chat_connection.js');
const c_routing = require('./js_chat_routing.js');
const c_tasks = require('./js_chat_tasks.js');


function fn_initTasks() {
    c_tasks.fn_initTasks();
}


function fn_startServer() {
    console.log(global.Colors.BSuccess + "[OK] Comm Server Manager has Started at port " + global.m_serverconfig.m_configuration.server_port + global.Colors.Reset);
    c_PluginManager.fn_initPlugins();
    fn_initTasks();
    c_connection.fn_startChatServer();
}


module.exports = {
    fn_startServer: fn_startServer,
    fn_parseExternalMessage: c_routing.fn_parseExternalMessage
};
