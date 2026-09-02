"use strict";

// Backward-compatibility shim.
//
// v5.5.0 moved the dumperror helpers into the droneengage_server_common
// package (lib/dumperror.js), which re-exports all historical function names
// (dumperror, fn_dumperror, dumperror2, fn_dumpdebug) for backward
// compatibility. This file keeps the old src/comm_server/js_dumperror.js path
// alive so that plugins (e.g. command_processing) and any legacy code that
// still require("../../comm_server/js_dumperror.js") continue to resolve.
//
// New code should require("droneengage_server_common").dumperror directly.

module.exports = require("droneengage_server_common").dumperror;
