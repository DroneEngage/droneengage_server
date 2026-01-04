"use strict";

const v_pjson           = require('./package.json');
const hlp_string        = require('./helpers/hlp_strings.js');
global.Colors           = require ("./helpers/js_colors.js").Colors;
global.m_serverconfig   = require ('./js_serverConfig.js'); 



let v_configFileName = global.m_serverconfig.getFileName();



const m_andruav_comm_server = require ('./server/js_andruav_comm_server.js')
const m_udp_proxy = require('./server/js_udp_proxy.js');
global.m_andruav_channel_parent_server = require ('./server/server_to_server/js_parent_comm_server.js');
global.m_andruav_channel_child_socket = require ('./server/server_to_server/js_child_comm_server.js');


process.on('SIGINT', function() {
    if (global.m_logger) global.m_logger.Warn('SIGINT.');
    process.exit(0);
});


function checkMemory()
    {
        const used = process.memoryUsage();
        let readings = "";
        for (let key in used) {
            readings += `${key} ${Math.round(used[key] / 1024 / 1024 * 100) / 100} MB - `;
        }
        console.log(readings);

        // Check memory limit
        if (global.m_serverconfig.m_configuration.memory_max != null) {
            const mem = Math.round(used.rss / 1024 / 1024 * 100) / 100;
            if (global.m_serverconfig.m_configuration.memory_max < mem) {
                console.log("Memory is " + global.Colors.FgYellow + mem + global.Colors.Error + ' RESTART' + global.Colors.Reset);
                process.exit(1);
            }
        }
    }

function fn_displayHelp ()
{
    console.log ("==================================")
    console.log (global.Colors.Bright + "Andruav Communication Server version " +  JSON.stringify(v_pjson.version) + global.Colors.Reset);
    console.log ("----------------------------------");
    console.log ("--config=config_filename config file ");
    console.log ("-h help ");
    console.log ("-v version");
    console.log ("==================================");
}


function fn_displayInfo ()
{
    console.log ("=============================================")
    console.log (global.Colors.Bright + "DE Communication Server version " +  JSON.stringify(v_pjson.version) + global.Colors.Reset);
    console.log ("---------------------------------------------");
    console.log ("Server Name  " + global.Colors.BSuccess +  global.m_serverconfig.m_configuration.server_id + global.Colors.Reset);
    console.log ("listening on ip: " + global.Colors.BSuccess +  global.m_serverconfig.m_configuration.server_ip + global.Colors.Reset + " port: " + global.Colors.BSuccess + global.m_serverconfig.m_configuration.server_port + global.Colors.Reset);
    console.log ("Auth Server ip: " + global.Colors.BSuccess +  global.m_serverconfig.m_configuration.s2s_ws_target_ip + global.Colors.Reset + " port: " + global.Colors.BSuccess + global.m_serverconfig.m_configuration.s2s_ws_target_port + global.Colors.Reset);
    if (global.m_serverconfig.m_configuration.ignoreLog!==false)
    {
        console.log ("logging is " + global.Colors.FgYellow + 'disabled' + global.Colors.Reset);
    }
    else
    {

        global.m_logger         = require ('node-file-logger');

        const options = {
            timeZone: global.m_serverconfig.m_configuration.log_timeZone==null?'GMT':global.m_serverconfig.m_configuration.log_timeZone,      
            folderPath: global.m_serverconfig.m_configuration.log_directory==null?'./log':global.m_serverconfig.m_configuration.log_directory,      
            dateBasedFileNaming: true,
            // Required only if dateBasedFileNaming is set to false
            fileName: 'All_Logs',   
            // Required only if dateBasedFileNaming is set to true
            fileNamePrefix: 'Logs_',
            fileNameSuffix: '',
            fileNameExtension: '.log',     
            
            dateFormat: 'YYYY-MM-DD',
            timeFormat: 'HH:mm:ss.SSS',
            // Allowed values - debug, prod, prod-trace (Details below)
            // prod: Only 'warn', 'info', 'error' and 'fatal' messages are logged. 'debug' and 'trace' messages are not logged.
            logLevel: global.m_serverconfig.m_configuration.log_detailed==true?'debug':'prod',
            // If set to false then messages are logged to console as well
            onlyFileLogging: true
          };
        
        global.m_logger.SetUserOptions(options); 

        console.log ("logging is " + global.Colors.FgYellow + 'enabled' + global.Colors.Reset);

        if (global.m_logger) global.m_logger.Info('System Started.');
    }
    
    console.log ("Datetime: %s", new Date());
    console.log ("=====================================================================");
}




function fn_parseArgs()
{
    const c_args = require ('./helpers/hlp_args.js');

    var cmds = c_args.getArgs();
    if (cmds.hasOwnProperty('h') || cmds.hasOwnProperty('help'))
    {

        fn_displayHelp();
        
        process.exit(0);
    }

    if (cmds.hasOwnProperty('v') || cmds.hasOwnProperty('version'))
    {

        console.log ("Andruav Authentication Server version: " + JSON.stringify(v_pjson.version));
        
        process.exit(0);
    }


    if (cmds.hasOwnProperty('config') )
    {
        v_configFileName = cmds.config;
    }
}



function fn_initSingletons()
{
    global.m_chat_server_singelton_get_instance = function ()
    {
        if (global.m_chat_server_singelton_instance === undefined)
        {
            global.m_chat_server_singelton_instance = require ('./server/chat_server/js_andruav_chat_server.js');
        }
        
        return global.m_chat_server_singelton_instance;
    }
}

/**
 * Start Server
 */
function fn_startServer ()
{
    // checking memory
    setInterval(checkMemory, 10000);

    // parse input arguments
    fn_parseArgs();

    // Singletons init
    fn_initSingletons();

    // load server configuration
    global.m_serverconfig.init(v_configFileName);
        
    // display info
    fn_displayInfo();
    m_andruav_comm_server.fn_startServer();

    m_udp_proxy.checkAndFixKernelBuffers();

    if (global.m_serverconfig.m_configuration.enable_super_server === true)
    {
        m_andruav_channel_parent_server.getInstance(
            global.m_serverconfig.m_configuration.s2s_super_server_ip,
            global.m_serverconfig.m_configuration.s2s_super_server_port
        );
    }


    if (global.m_serverconfig.m_configuration.enable_persistant_relay === true)
    {
        m_andruav_channel_child_socket.getInstance(
            global.m_serverconfig.m_configuration.s2s_relay_to_super_server_ip,
            global.m_serverconfig.m_configuration.s2s_relay_to_super_server_port
        );
    }
    
   
}


fn_startServer();







