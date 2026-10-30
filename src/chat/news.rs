//! News commands (js_chat_news.js): LoadNews / SaveNews / DeleteNews through
//! the storage server, and the NewsPush fan-out that delivers news live to
//! the connected GCS clients.
//!
//! The WS write path is always scoped to the caller's own account: only the
//! storage server's admin dashboard writes scope='global' news.

use serde_json::{json, Value};
use std::sync::Arc;

use crate::chat::outbound_guard::MsgClass;
use crate::chat::{dump_error, parse_message_body, spawn, Ctx, Routing};
use crate::constants::*;
use crate::hub::Payload;
use crate::server::Server;
use crate::util::{field, field_cloned, js_length, js_nullish, js_stringify, js_truthy};

fn not_connected(p_server: &Server) -> Value {
    json!(format!("Error: Storage server not connected (state: {})", p_server.db.connection_state()))
}

pub fn handle_load_news(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    println!("load news command");
    let Some(v_account) = p_ctx.account_id().filter(|a| !a.is_null()) else {
        dump_error("load news: missing accountID");
        return;
    };
    let v_mt = Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_LOAD_NEWS));
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(p_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
        return;
    }
    let v_server = p_server.clone();
    spawn(async move {
        match v_server.db.load_news(v_account).await {
            Ok(v_resp) => {
                if v_resp.get("success") != Some(&Value::Bool(true)) {
                    return;
                }
                let Some(v_ms) = v_resp.get("ms").filter(|m| !m.is_null()).cloned() else { return };
                p_ctx.reply(&v_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
            }
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                p_ctx.reply(
                    &v_server,
                    &mut p_jmsg,
                    Routing::System,
                    v_mt,
                    Some(json!("Error: Failed to load news from storage server")),
                );
            }
        }
    });
}

pub fn handle_save_news(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    println!("save news command");
    let Some(v_mms) = parse_message_body(&p_jmsg) else {
        dump_error("TypeError: Cannot read properties of undefined (reading 'body')");
        return;
    };
    let v_body = field(&v_mms, "body");
    if js_nullish(v_body) || js_length(v_body) == Some(0) {
        return;
    }
    let Some(v_account) = p_ctx.account_id().filter(|a| !a.is_null()) else {
        dump_error("save news: missing accountID");
        return;
    };
    let v_author =
        p_ctx.login.as_ref().and_then(|l| l.sender_id.clone()).filter(|s| js_truthy(Some(s))).unwrap_or(Value::Null);
    let v_mt = Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_SAVE_NEWS));
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(p_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
        return;
    }
    let v_server = p_server.clone();
    spawn(async move {
        let v_res = v_server
            .db
            .save_news(
                CONST_NEWS_SCOPE_ACCOUNT,
                v_account,
                field_cloned(&v_mms, "title"),
                field_cloned(&v_mms, "body"),
                field_cloned(&v_mms, "priority"),
                v_author,
                field_cloned(&v_mms, "expiresAt"),
                field_cloned(&v_mms, "newsId"),
            )
            .await;
        let v_failed = json!("Error: Failed to save news to storage server");
        match v_res {
            Ok(v_resp) => {
                if v_resp.get("success") != Some(&Value::Bool(true)) {
                    return;
                }
                // p_response.ms.newsId throws when ms is missing -> the catch path
                match v_resp.get("ms") {
                    None | Some(Value::Null) => {
                        dump_error("TypeError: Cannot read properties of undefined (reading 'newsId')");
                        p_ctx.reply(&v_server, &mut p_jmsg, Routing::System, v_mt, Some(v_failed));
                    }
                    Some(v_ms) => {
                        let mut v_out = serde_json::Map::new();
                        v_out.insert("s".into(), json!("OK:save"));
                        if let Some(id) = field(v_ms, "newsId") {
                            v_out.insert("newsId".into(), id.clone());
                        }
                        p_ctx.reply(&v_server, &mut p_jmsg, Routing::System, v_mt, Some(Value::Object(v_out)));
                    }
                }
            }
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                p_ctx.reply(&v_server, &mut p_jmsg, Routing::System, v_mt, Some(v_failed));
            }
        }
    });
}

pub fn handle_delete_news(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    println!("delete news command");
    let Some(v_mms) = parse_message_body(&p_jmsg) else {
        dump_error("TypeError: Cannot read properties of undefined (reading 'newsId')");
        return;
    };
    let Some(v_news_id) = field(&v_mms, "newsId").filter(|v| !v.is_null()).cloned() else { return };
    if js_nullish(p_ctx.account_id().as_ref()) {
        dump_error("delete news: missing accountID");
        return;
    }
    let v_mt = Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_DELETE_NEWS));
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(p_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
        return;
    }
    let v_server = p_server.clone();
    spawn(async move {
        match v_server.db.delete_news(Some(v_news_id.clone())).await {
            Ok(_) => {
                let v_ms = json!({ "s": "OK:delete", "newsId": v_news_id });
                p_ctx.reply(&v_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
            }
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                p_ctx.reply(
                    &v_server,
                    &mut p_jmsg,
                    Routing::System,
                    v_mt,
                    Some(json!("Error: Failed to delete news from storage server")),
                );
            }
        }
    });
}

/// fn_onNewsPush: an unsolicited news change from the storage server
/// ({news}), fanned out to the GCS clients connected here that should see
/// it. A push without scope/account_id (e.g. a disable) goes to every GCS.
pub fn on_news_push(p_server: &Arc<Server>, p_payload: Option<&Value>) {
    let Some(v_news) = p_payload.filter(|p| js_truthy(Some(p))).and_then(|p| field(p, "news")).filter(|n| !n.is_null())
    else {
        return;
    };
    let v_msg = json!({
        "ty": CONST_WS_MSG_ROUTING_SYSTEM,
        "sd": CONST_WS_SENDER_COMM_SERVER,
        "mt": CONST_TYPE_ANDRUAV_SYSTEM_NEWS_PUSH,
        "ms": v_news,
    });
    let v_payload = Payload::text(js_stringify(&v_msg));
    let v_sender = json!(CONST_WS_SENDER_COMM_SERVER);
    let v_scope_global = field(v_news, "scope").and_then(Value::as_str) == Some(CONST_NEWS_SCOPE_GLOBAL);
    let v_account = field(v_news, "account_id");
    let mut v_hub = p_server.hub();
    if v_scope_global || js_nullish(v_account) {
        v_hub.send_to_all_gcs(&p_server.guard, &v_payload, false, Some(&v_sender), MsgClass::MustDeliver);
    } else {
        v_hub.send_to_all_gcs_in_account(
            &p_server.guard,
            &v_payload,
            false,
            Some(&v_sender),
            v_account,
            MsgClass::MustDeliver,
        );
    }
}
