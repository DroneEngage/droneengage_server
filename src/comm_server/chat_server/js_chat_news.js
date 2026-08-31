"use strict";

/**
 * News command module.
 * Handles LoadNews / SaveNews / DeleteNews system commands (mirrors
 * js_chat_tasks.js's mission handlers) plus the NewsPush fan-out that
 * delivers news live to connected GCS clients.
 *
 * Write path (SaveNews/DeleteNews from a GCS client) is always scoped to the
 * caller's own accountID here - only the storage server's admin dashboard can
 * write scope='global' news (see droneengage_storage_server/src/dashboard).
 *
 * TODO: once role/permission info (super user) is available on the login
 * session, allow verified super users to also write scope='global' news
 * from this WS path, and verify ownership before allowing a DeleteNews.
 */

const c_dumpError = require("../../dumperror.js");
const c_CONSTANTS = require("../../js_constants.js");
const c_dbProxyClient = require("../server_to_server/js_db_proxy_client.js");
const c_ChatAccountRooms = require("./js_andruav_chat_account_rooms.js");


/**
 * Normalizes v_jmsg.ms into an object (backward compatible with stringified bodies).
 */
function fn_parseMessageBody(v_jmsg) {
    let mms = v_jmsg.ms;
    if (typeof mms === 'string' || mms instanceof String) {
        mms = JSON.parse(v_jmsg.ms);
    }
    return mms;
}

function fn_sendReply(v_jmsg, p_ws, p_messageType, p_ms) {
    v_jmsg.ty = c_CONSTANTS.CONST_WS_MSG_ROUTING_SYSTEM;
    v_jmsg.tg = p_ws.name;
    v_jmsg.sd = c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER;
    v_jmsg.mt = p_messageType;
    v_jmsg.ms = p_ms;
    p_ws.send(JSON.stringify(v_jmsg));
}


function fn_handleLoadNews(v_jmsg, p_ws) {
    c_dumpError.fn_dumpdebug("load news command");

    const c_accountId = p_ws.m_loginRequest ? p_ws.m_loginRequest.m_accountID : null;
    if (c_accountId == null) {
        c_dumpError.fn_dumperror("load news: missing accountID");
        return;
    }

    if (c_dbProxyClient.fn_isConnected()) {
        c_dbProxyClient.fn_loadNews(c_accountId)
            .then(function (p_response) {
                if ((p_response.success !== true) || (p_response.ms == null)) return;
                fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_LoadNews, p_response.ms);
            })
            .catch(function (err) {
                c_dumpError.fn_dumperror(err);
                fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_LoadNews, "Error: Failed to load news from storage server");
            });
        return;
    }

    const c_connectionState = c_dbProxyClient.fn_getConnectionState();
    fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_LoadNews, `Error: Storage server not connected (state: ${c_connectionState})`);
}


function fn_handleSaveNews(v_jmsg, p_ws) {
    c_dumpError.fn_dumpdebug("save news command");
    const mms = fn_parseMessageBody(v_jmsg);

    if ((mms.body == null) || (mms.body.length === 0)) {
        return;
    }

    const c_accountId = p_ws.m_loginRequest ? p_ws.m_loginRequest.m_accountID : null;
    if (c_accountId == null) {
        c_dumpError.fn_dumperror("save news: missing accountID");
        return;
    }

    // Regular GCS users can only write account-scoped news for their own account.
    // Global news is only writable from the storage server admin dashboard for now.
    const c_scope = c_CONSTANTS.CONST_NEWS_SCOPE_ACCOUNT;
    const c_authorId = p_ws.m_loginRequest.m_senderID || null;

    if (c_dbProxyClient.fn_isConnected()) {
        c_dbProxyClient.fn_saveNews(c_scope, c_accountId, mms.title, mms.body, mms.priority, c_authorId, mms.expiresAt, mms.newsId)
            .then(function (p_response) {
                if (p_response.success !== true) return;
                fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveNews, { s: 'OK:save', newsId: p_response.ms.newsId });
                // Live delivery to other GCS clients happens via the storage server's
                // NewsPush broadcast (see fn_onNewsPush) - not here - so the WS write
                // path and the admin dashboard write path share one delivery mechanism.
            })
            .catch(function (err) {
                c_dumpError.fn_dumperror(err);
                fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveNews, "Error: Failed to save news to storage server");
            });
        return;
    }

    const c_connectionState = c_dbProxyClient.fn_getConnectionState();
    fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_SaveNews, `Error: Storage server not connected (state: ${c_connectionState})`);
}


function fn_handleDeleteNews(v_jmsg, p_ws) {
    c_dumpError.fn_dumpdebug("delete news command");
    const mms = fn_parseMessageBody(v_jmsg);

    if (mms.newsId == null) {
        return;
    }

    const c_accountId = p_ws.m_loginRequest ? p_ws.m_loginRequest.m_accountID : null;
    if (c_accountId == null) {
        c_dumpError.fn_dumperror("delete news: missing accountID");
        return;
    }

    if (c_dbProxyClient.fn_isConnected()) {
        c_dbProxyClient.fn_deleteNews(mms.newsId)
            .then(function (p_response) {
                fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_DeleteNews, { s: 'OK:delete', newsId: mms.newsId });
            })
            .catch(function (err) {
                c_dumpError.fn_dumperror(err);
                fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_DeleteNews, "Error: Failed to delete news from storage server");
            });
        return;
    }

    const c_connectionState = c_dbProxyClient.fn_getConnectionState();
    fn_sendReply(v_jmsg, p_ws, c_CONSTANTS.CONST_TYPE_AndruavSystem_DeleteNews, `Error: Storage server not connected (state: ${c_connectionState})`);
}


/**
 * Registered with js_db_proxy_client.fn_setNewsPushCallback(). Called whenever
 * the storage server pushes an unsolicited news change (created via this or
 * any other comm server's WS path, or via the storage server admin dashboard).
 * Fans it out to the GCS clients connected to THIS comm server that should see it.
 * @param {{news: object}} p_payload
 */
function fn_onNewsPush(p_payload) {
    try {
        const c_news = p_payload && p_payload.news;
        if (c_news == null) return;

        const v_jmsg = {
            ty: c_CONSTANTS.CONST_WS_MSG_ROUTING_SYSTEM,
            sd: c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER,
            mt: c_CONSTANTS.CONST_TYPE_AndruavSystem_NewsPush,
            ms: c_news
        };
        const c_message = JSON.stringify(v_jmsg);

        // A disable/delete push may only carry {id, disabled}, without scope/account_id.
        // Broadcasting it to every GCS is harmless (clients ignore unknown ids).
        if ((c_news.scope === c_CONSTANTS.CONST_NEWS_SCOPE_GLOBAL) || (c_news.account_id == null)) {
            c_ChatAccountRooms.fn_sendToAllGCS(c_message, false, c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER);
        } else {
            c_ChatAccountRooms.fn_sendToAllGCSInAccount(c_message, false, c_CONSTANTS.CONST_WS_SENDER_COMM_SERVER, c_news.account_id);
        }
    } catch (e) {
        c_dumpError.fn_dumperror(e);
    }
}


module.exports = {
    fn_handleLoadNews,
    fn_handleSaveNews,
    fn_handleDeleteNews,
    fn_onNewsPush
};
