//! System commands (js_chat_system_commands.js): messages routed 's' — UDP
//! proxy (9008) and its GCS-side pin (9021), ping, logout, and the task /
//! mission / query / news commands.

use serde_json::{json, Value};
use std::sync::Arc;

use crate::chat::{dump_error, news, query_server, tasks, Ctx};
use crate::constants::*;
use crate::server::Server;
use crate::util::{field, js_interp, js_truthy, now_ms, set_field};

/// onSystemMessage
pub fn on_system_message(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    let v_mt = p_jmsg.get(CONST_WS_MESSAGE_ID).cloned();
    let v_mt_num = match &v_mt {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    };
    if v_mt.as_ref().and_then(Value::as_str) == Some("ver") {
        // `version` is not defined in the JavaScript handler: it throws before replying
        dump_error("ReferenceError: version is not defined");
        return;
    }
    let Some(v_mt_num) = v_mt_num else { return };
    if v_mt_num.fract() != 0.0 {
        return;
    }
    match v_mt_num as i64 {
        CONST_TYPE_ANDRUAV_SYSTEM_UDP_PROXY => {
            let v_ms = p_jmsg.get("ms");
            let v_en = if js_truthy(v_ms) { field(v_ms.unwrap(), "en").cloned() } else { v_ms.cloned() };
            let v_actor = p_ctx.login.as_ref().map(|l| l.actor_type.clone()).unwrap_or(None);
            let v_name = js_interp(p_ctx.name.as_ref());
            println!(
                "UdpProxy request from '{}' en={} actorType={}",
                v_name,
                js_interp(v_en.as_ref()),
                if p_ctx.login.is_some() { js_interp(v_actor.as_ref()) } else { "null".into() }
            );
            if !p_ctx.login.as_ref().map(|l| l.actor_is(CONST_ACTOR_TYPE_DRONE)).unwrap_or(false) {
                // only a vehicle can create a udp proxy
                println!("UdpProxy request from '{}' REJECTED: not a drone actorType", v_name);
                return;
            }
            if p_server.relay.is_enabled() {
                let v_server = p_server.clone();
                tokio::spawn(async move {
                    crate::s2s::udp_relay_client::handle_unit_request(&v_server, p_ctx, p_jmsg).await;
                });
            } else {
                // no relay configured: this server offers no UDP proxy
                let v_server = p_server.clone();
                tokio::spawn(async move {
                    crate::s2s::udp_relay_client::reply_disabled(&v_server, &p_ctx, p_jmsg);
                });
            }
        }
        CONST_TYPE_ANDRUAV_SYSTEM_UDP_PROXY_PEER => {
            let v_server = p_server.clone();
            tokio::spawn(async move {
                crate::s2s::udp_relay_client::handle_gcs_peer_request(&v_server, p_ctx, p_jmsg).await;
            });
        }
        CONST_TYPE_ANDRUAV_SYSTEM_PING => {
            match p_jmsg.get_mut("ms") {
                Some(Value::Object(ms)) => {
                    ms.insert("s".into(), json!("OK:pong"));
                    // CLOCK.md T2: the server clock, for the units' fleet clock sync
                    ms.insert("ts".into(), json!(now_ms()));
                }
                Some(Value::Array(_)) => {}
                _ => {
                    dump_error("TypeError: Cannot set properties of undefined (setting 's')");
                    return;
                }
            }
            p_ctx.handle.send_json(&p_jmsg);
        }
        CONST_TYPE_ANDRUAV_SYSTEM_LOGOUT_COMM_SERVER => {
            p_server.hub().del_member_from_group(p_ctx.conn);
            set_field(&mut p_jmsg, "ms", json!({ "s": "OK:dell" }));
            p_ctx.handle.send_json(&p_jmsg);
        }
        CONST_TYPE_ANDRUAV_SYSTEM_LOAD_TASKS => tasks::handle_load_tasks(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_SAVE_TASKS => tasks::handle_save_tasks(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_DELETE_TASKS => tasks::handle_delete_tasks(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_DISABLE_TASKS => tasks::handle_disable_tasks(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_LOAD_MISSION => tasks::handle_load_mission(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_SAVE_MISSION => tasks::handle_save_mission(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_DELETE_MISSION => tasks::handle_delete_mission(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_QUERY_SERVER => query_server::handle_query_server(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_LOAD_NEWS => news::handle_load_news(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_SAVE_NEWS => news::handle_save_news(p_server, p_ctx, p_jmsg),
        CONST_TYPE_ANDRUAV_SYSTEM_DELETE_NEWS => news::handle_delete_news(p_server, p_ctx, p_jmsg),
        _ => {}
    }
}
