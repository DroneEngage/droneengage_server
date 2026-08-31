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

"use strict";

const common = require("droneengage_server_common");
const path = require("path");

module.exports = common.create({
    configDir: path.join(__dirname, '..'),
    enableHashHandling: false   // comm server has no $$HASH$$ feature
});
