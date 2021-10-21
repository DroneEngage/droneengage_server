/*************************************************************************************
 * 
 *   A N D R U A V -  Server Configuration File      JAVASCRIPT  LIB
 * 
 *   Author: Mohammad S. Hefny
 * 
 *   Date:   08 Sep 2016
 * 
 * 
 * 
 */



const c_commentStripper = require("./js_3rd_StripJsonComments.js");
var c_dumpError 		= require ("./js_dumperror.js");
const v_configFileName_default = "server.config";
const c_path = require('path');
const c_fs = require('fs');

var Me = this;
var v_configFileName = v_configFileName_default;
exports.JSONconfig = null;
exports.fn_getFileName = function ()
{
		return v_configFileName;
}

exports.fn_init = function fn_init (configFileName)
{
		if (configFileName != null)
		{
			v_configFileName = configFileName;
		}
		
        try
        {
        var filestring = c_fs.readFileSync(c_path.join(__dirname,v_configFileName)).toString();			
        }
        catch (err)
        {
            console.log ('FATAL: could not find ' + v_configFileName);
            c_dumpError.fn_dumperror(err);
            process.exit(1);
        }

         try
        {
            Me.JSONconfig = JSON.parse(c_commentStripper(filestring));
        }
        catch (err)
        {
            console.log ('FATAL: Bad File Format ' + v_configFileName);
            c_dumpError.fn_dumperror(err);
            process.exit(1);
        }
}
