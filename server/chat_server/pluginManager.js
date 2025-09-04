const path = require('path');

const c_commandProcessors = [];

function fn_initPlugins() {
    if (global.m_serverconfig.m_configuration?.command_plugin) {
        global.m_serverconfig.m_configuration.command_plugin.forEach((pluginPath) => {
            try {
                const baseDir = path.join(__dirname, '..');
                const resolvedPath = path.resolve(baseDir, pluginPath);
                const processor = require(resolvedPath);

                if (processor?.processCommand) {
                    c_commandProcessors.push(processor);
                    console.log(`Loaded plugin at ${pluginPath} with processCommand method.`);
                } else {
                    console.warn(`Plugin at ${pluginPath} is loaded, but processCommand method is not available.`);
                }
            } catch (error) {
                console.error(`Failed to load plugin at ${pluginPath}: ${error.message}`);
            }
        });
    } else {
        console.warn("No command plugins specified in configuration.");
    }
}

async function fn_processPlugins(p_ws, p_message, v_jmsg, nullIndex, callback) {
    for (const processor of c_commandProcessors) {
        try {
            await processor.processCommand(p_ws, p_message, v_jmsg, nullIndex, callback);
        } catch (error) {
            console.error(`Error processing plugin command at ${processor.constructor?.name || 'unknown plugin'}: ${error.message}`);
        }
    }
}

module.exports = {
    fn_initPlugins,
    fn_processPlugins
};