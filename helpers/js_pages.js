"use strict";

var v_config = require('../config/config');


module.exports.fn_getHeaderParams = function (p_url)
{
	var v_regex = /[?&]([^=#]+)=([^&#]*)/g,
    v_params = {},
    v_match;


	while(v_match = v_regex.exec(p_url)) {
		v_params[v_match[1]] = v_match[2]; 
	}
	
	return v_params;
}

module.exports.fn_renderPage = function (req,p_res,p_page, p_title, p_params) {
    p_res.render(p_page, {
        title: v_config.title + ' | ' + p_title,
        version: v_config.version,
        copyright: v_config.copyright,
        params: p_params
    });
}