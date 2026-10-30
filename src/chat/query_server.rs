//! QueryServer (js_chat_query_server.js): a unit asks for server state with
//! a subcommand in ms.sc; the reply is a StateServer (9014) message.
//!
//!   1 Is_Storage_Server_Connected -> { sc: 1, v: <bool>, st: <state code> }

use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::chat::{Ctx, Routing};
use crate::constants::*;
use crate::server::Server;
use crate::util::{field, js_strict_eq};

pub fn handle_query_server(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    let v_mms = match p_jmsg.get("ms") {
        Some(Value::String(s)) => match crate::util::js_parse(s) {
            Some(v) => v,
            None => return,
        },
        Some(v) => v.clone(),
        None => return,
    };
    if v_mms.is_null() {
        return;
    }
    let v_sub = field(&v_mms, "sc").cloned();
    let mut v_reply = Map::new();
    if let Some(s) = &v_sub {
        v_reply.insert("sc".into(), s.clone());
    }
    if js_strict_eq(
        v_sub.as_ref(),
        Some(&json!(CONST_TYPE_ANDRUAV_SYSTEM_QUERY_SERVER_SUB_CMD_IS_STORAGE_SERVER_CONNECTED)),
    ) {
        v_reply.insert("v".into(), json!(p_server.db.is_connected()));
        v_reply.insert("st".into(), json!(p_server.db.connection_state()));
    } else {
        v_reply.insert("v".into(), json!(false));
        v_reply.insert("st".into(), json!("unknown_subcommand"));
    }
    p_ctx.reply(
        p_server,
        &mut p_jmsg,
        Routing::System,
        Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_STATE_SERVER)),
        Some(Value::Object(v_reply)),
    );
}
