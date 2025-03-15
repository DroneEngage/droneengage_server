"use strict";
const path = require('path');


const c_commandProcessors = [];

function fn_initPlugins() {
    
    if (global.m_serverconfig.m_configuration?.command_plugin) {
        global.m_serverconfig.m_configuration.command_plugin.forEach((pluginPath) => {
            try {
                const baseDir = path.join(__dirname, '..');
                const resolvedPath = path.resolve(baseDir, pluginPath);
                const processor = require(resolvedPath);

                if (processor?.fn_processCommand) {
                    c_commandProcessors.push(processor);
                } else {
                    console.warn(`Plugin at ${pluginPath} is loaded, but fn_processCommand is not available.`);
                }
            } catch (error) {
                console.error(`Failed to load plugin at ${pluginPath}:`, error);
            }
        });
    } else {
        console.warn("No command plugins specified in configuration.");
    }
}

function fn_processPlugins (p_ws, p_message, v_jmsg, nullIndex, callback)
{
    // Call Plugins if Available
    c_commandProcessors.forEach(element => {
        element.fn_processCommand(p_ws, p_message, v_jmsg, nullIndex,callback);
        });
}

module.exports = {
    fn_initPlugins,
    fn_processPlugins
};
