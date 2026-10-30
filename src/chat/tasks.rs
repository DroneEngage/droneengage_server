//! Task and mission commands (js_chat_tasks.js): LoadTasks / SaveTasks /
//! DeleteTasks / DisableTasks and LoadMission / SaveMission / DeleteMission,
//! forwarded to the storage server.
//!
//! The legacy MySQL task store (js_andruavTasks_v2.js) is not ported: the
//! JavaScript handlers only reached it from code after a `return`, so every
//! task request already went to the storage server (or got the "Storage
//! server not connected" reply).

use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::chat::{dump_debug, dump_error, parse_message_body, spawn, Ctx, Routing};
use crate::constants::*;
use crate::server::Server;
use crate::util::{field, field_cloned, js_missing_or_empty, js_nullish, set_field, set_field_opt, uuid_v4};

/// fn_fillTaskParams: the optional task-query fields shared by every task
/// command (kept for the storage-less code path and its tests).
pub fn fill_task_params(p_mms: &Value, p_params: &mut Map<String, Value>, p_include_lts: bool, p_include_task: bool) {
    let fn_copy = |p_params: &mut Map<String, Value>, from: &str, to: &str| {
        if let Some(v) = field(p_mms, from) {
            p_params.insert(to.into(), v.clone());
        }
    };
    if p_include_lts {
        fn_copy(p_params, "lts", "largerThan_SID");
    }
    fn_copy(p_params, "ac", "accessCode");
    fn_copy(p_params, "ai", "accountID");
    fn_copy(p_params, "ps", "party_sid");
    fn_copy(p_params, "gn", "groupName");
    fn_copy(p_params, "s", "sender");
    fn_copy(p_params, "r", "receiver");
    fn_copy(p_params, "mt", "messageType");
    fn_copy(p_params, "ip", "isPermanent");
    if p_include_task {
        if let Some(t) = field(p_mms, "t") {
            p_params.insert("task".into(), Value::String(crate::util::js_stringify(t)));
        }
    }
}

fn not_connected(p_server: &Server) -> Value {
    json!(format!("Error: Storage server not connected (state: {})", p_server.db.connection_state()))
}

fn body_or_throw(p_jmsg: &Value) -> Option<Value> {
    let v_mms = parse_message_body(p_jmsg);
    if v_mms.is_none() {
        dump_error("TypeError: Cannot read properties of undefined (reading 'ai')");
    }
    v_mms
}

pub fn handle_load_tasks(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    println!("load tasks command");
    let Some(v_mms) = body_or_throw(&p_jmsg) else { return };
    if js_missing_or_empty(field(&v_mms, "ai")) {
        return;
    }
    dump_debug(&v_mms);
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(
            p_server,
            &mut p_jmsg,
            Routing::Individual,
            Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_LOAD_TASKS)),
            Some(v_ms),
        );
        return;
    }
    let v_server = p_server.clone();
    spawn(async move {
        let v_req = json!({ "unitId": field_cloned(&v_mms, "ai") });
        match v_server.db.send_request(CONST_TYPE_ANDRUAV_SYSTEM_LOAD_TASKS, v_req).await {
            Ok(v_resp) => {
                if v_resp.get("success") != Some(&Value::Bool(true)) {
                    return;
                }
                let Some(v_tasks) = v_resp.get("ms").filter(|m| !m.is_null()).and_then(|m| m.get("tasks")) else {
                    return;
                };
                let Value::Array(v_tasks) = v_tasks else { return };
                for v_task in v_tasks {
                    set_field(&mut p_jmsg, "ty", json!(CONST_WS_MSG_ROUTING_INDIVIDUAL));
                    set_field_opt(&mut p_jmsg, "tg", p_ctx.name_now(&v_server));
                    set_field(&mut p_jmsg, "sd", json!(CONST_WS_SENDER_COMM_SERVER));
                    set_field_opt(&mut p_jmsg, "sid", field_cloned(v_task, "taskId"));
                    set_field_opt(&mut p_jmsg, "ms", field_cloned(v_task, "data"));
                    let v_mt = match field(v_task, "messageType") {
                        Some(v) if !v.is_null() => Some(v.clone()),
                        _ => field_cloned(&v_mms, "messageType"),
                    };
                    set_field_opt(&mut p_jmsg, "mt", v_mt);
                    p_ctx.handle.send_json(&p_jmsg);
                }
            }
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                p_ctx.reply(
                    &v_server,
                    &mut p_jmsg,
                    Routing::Individual,
                    Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_LOAD_TASKS)),
                    Some(json!("Error: Failed to load tasks from storage server")),
                );
            }
        }
    });
}

pub fn handle_save_tasks(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    println!("save tasks command");
    let Some(mut v_mms) = body_or_throw(&p_jmsg) else { return };
    dump_debug(&v_mms);
    if js_missing_or_empty(field(&v_mms, "ai")) {
        return;
    }
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(
            p_server,
            &mut p_jmsg,
            Routing::Individual,
            Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_SAVE_TASKS)),
            Some(v_ms),
        );
        return;
    }
    // the legacy protocol carries one task (mms.t); mms.t.id is the stable id
    let v_task_id: Value = match v_mms.get_mut("t") {
        None | Some(Value::Null) => json!(uuid_v4()),
        Some(Value::Object(t)) => {
            if js_nullish(t.get("id")) {
                t.insert("id".into(), json!(uuid_v4()));
            }
            t["id"].clone()
        }
        Some(Value::Array(_)) => json!(uuid_v4()),
        Some(_) => {
            dump_error("TypeError: Cannot create property 'id' on primitive");
            return;
        }
    };
    let mut v_task = Map::new();
    v_task.insert("taskId".into(), v_task_id);
    if let Some(n) = field(&v_mms, "messageType") {
        v_task.insert("name".into(), n.clone());
    }
    if let Some(t) = field(&v_mms, "t") {
        v_task.insert("data".into(), t.clone());
    }
    let mut v_req = Map::new();
    if let Some(a) = field(&v_mms, "ai") {
        v_req.insert("unitId".into(), a.clone());
    }
    v_req.insert("tasks".into(), json!([Value::Object(v_task)]));
    let v_server = p_server.clone();
    spawn(async move {
        match v_server.db.send_request(CONST_TYPE_ANDRUAV_SYSTEM_SAVE_TASKS, Value::Object(v_req)).await {
            Ok(v_resp) => {
                if v_resp.get("success") != Some(&Value::Bool(true)) {
                    return;
                }
                p_ctx.reply(
                    &v_server,
                    &mut p_jmsg,
                    Routing::Individual,
                    Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_SAVE_TASKS)),
                    Some(json!("Done")),
                );
            }
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                p_ctx.reply(
                    &v_server,
                    &mut p_jmsg,
                    Routing::Individual,
                    Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_SAVE_TASKS)),
                    Some(json!("Error: Failed to save tasks to storage server")),
                );
            }
        }
    });
}

/// DeleteTasks / DisableTasks: one task by mms.t.id.
fn handle_task_ids(
    p_server: &Arc<Server>,
    p_ctx: Ctx,
    mut p_jmsg: Value,
    p_mt: i64,
    p_label: &'static str,
    p_verb: &'static str,
) {
    println!("{} tasks command", p_label);
    let Some(v_mms) = body_or_throw(&p_jmsg) else { return };
    if p_mt == CONST_TYPE_ANDRUAV_SYSTEM_DELETE_TASKS {
        dump_debug(&v_mms);
    }
    if js_missing_or_empty(field(&v_mms, "ai")) {
        return;
    }
    if p_mt == CONST_TYPE_ANDRUAV_SYSTEM_DISABLE_TASKS {
        dump_debug(&v_mms);
    }
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(p_server, &mut p_jmsg, Routing::Individual, Some(json!(p_mt)), Some(v_ms));
        return;
    }
    let v_task_id = field(&v_mms, "t").filter(|t| !t.is_null()).and_then(|t| field(t, "id")).cloned();
    let Some(v_task_id) = v_task_id.filter(|v| !v.is_null()) else { return };
    let mut v_req = Map::new();
    if let Some(a) = field(&v_mms, "ai") {
        v_req.insert("unitId".into(), a.clone());
    }
    v_req.insert("taskIds".into(), json!([v_task_id]));
    let v_server = p_server.clone();
    spawn(async move {
        match v_server.db.send_request(p_mt, Value::Object(v_req)).await {
            Ok(v_resp) => {
                if v_resp.get("success") != Some(&Value::Bool(true)) {
                    return;
                }
                p_ctx.reply(&v_server, &mut p_jmsg, Routing::Individual, Some(json!(p_mt)), Some(json!("Done")));
            }
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                let v_text = format!("Error: Failed to {} tasks {} storage server", p_label, p_verb);
                p_ctx.reply(&v_server, &mut p_jmsg, Routing::Individual, Some(json!(p_mt)), Some(json!(v_text)));
            }
        }
    });
}

pub fn handle_delete_tasks(p_server: &Arc<Server>, p_ctx: Ctx, p_jmsg: Value) {
    handle_task_ids(p_server, p_ctx, p_jmsg, CONST_TYPE_ANDRUAV_SYSTEM_DELETE_TASKS, "delete", "from");
}

pub fn handle_disable_tasks(p_server: &Arc<Server>, p_ctx: Ctx, p_jmsg: Value) {
    handle_task_ids(p_server, p_ctx, p_jmsg, CONST_TYPE_ANDRUAV_SYSTEM_DISABLE_TASKS, "disable", "in");
}

fn mission_body(p_jmsg: &Value) -> Option<Value> {
    let v_mms = parse_message_body(p_jmsg);
    if v_mms.is_none() {
        dump_error("TypeError: Cannot read properties of undefined (reading 'unitId')");
    }
    v_mms
}

pub fn handle_load_mission(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    println!("load mission command");
    let Some(v_mms) = mission_body(&p_jmsg) else { return };
    // P1F-01: metadata listing ({list:true}) is account-wide — unitId optional
    if !crate::util::is_true(field(&v_mms, "list")) && js_missing_or_empty(field(&v_mms, "unitId")) {
        return;
    }
    let Some(v_account) = p_ctx.account_id().filter(|a| !a.is_null()) else {
        dump_error("load mission: missing accountID");
        return;
    };
    dump_debug(&v_mms);
    let v_mt = Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_LOAD_MISSION));
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(p_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
        return;
    }
    let v_server = p_server.clone();
    spawn(async move {
        let v_res = v_server
            .db
            .load_mission(field_cloned(&v_mms, "unitId"), v_account, field_cloned(&v_mms, "missionId"), &v_mms)
            .await;
        match v_res {
            Ok(v_resp) => {
                // forwarded verbatim; an absent ms still answers the client
                let v_ms = match v_resp.get("ms") {
                    Some(m) if !m.is_null() => m.clone(),
                    _ => json!("Error: storage returned no payload"),
                };
                p_ctx.reply(&v_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
            }
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                p_ctx.reply(
                    &v_server,
                    &mut p_jmsg,
                    Routing::System,
                    v_mt,
                    Some(json!("Error: Failed to load mission from storage server")),
                );
            }
        }
    });
}

pub fn handle_save_mission(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    println!("save mission command");
    let Some(v_mms) = mission_body(&p_jmsg) else { return };
    if js_missing_or_empty(field(&v_mms, "unitId")) {
        return;
    }
    let Some(v_account) = p_ctx.account_id().filter(|a| !a.is_null()) else {
        dump_error("save mission: missing accountID");
        return;
    };
    dump_debug(&v_mms);
    let v_mt = Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_SAVE_MISSION));
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(p_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
        return;
    }
    let v_server = p_server.clone();
    spawn(async move {
        let v_res = v_server
            .db
            .save_mission(
                field_cloned(&v_mms, "unitId"),
                v_account,
                field_cloned(&v_mms, "missionId"),
                field_cloned(&v_mms, "name"),
                field_cloned(&v_mms, "data"),
                &v_mms,
            )
            .await;
        match v_res {
            Ok(v_resp) => {
                let v_ms = match v_resp.get("ms") {
                    Some(m) if !m.is_null() => m.clone(),
                    _ => json!("Done"),
                };
                p_ctx.reply(&v_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
            }
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                p_ctx.reply(
                    &v_server,
                    &mut p_jmsg,
                    Routing::System,
                    v_mt,
                    Some(json!("Error: Failed to save mission to storage server")),
                );
            }
        }
    });
}

pub fn handle_delete_mission(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    println!("delete mission command");
    let Some(v_mms) = parse_message_body(&p_jmsg) else {
        dump_error("TypeError: Cannot read properties of undefined (reading 'missionId')");
        return;
    };
    if js_nullish(field(&v_mms, "missionId")) {
        return;
    }
    let Some(v_account) = p_ctx.account_id().filter(|a| !a.is_null()) else {
        dump_error("delete mission: missing accountID");
        return;
    };
    dump_debug(&v_mms);
    let v_mt = Some(json!(CONST_TYPE_ANDRUAV_SYSTEM_DELETE_MISSION));
    if !p_server.db.is_connected() {
        let v_ms = not_connected(p_server);
        p_ctx.reply(p_server, &mut p_jmsg, Routing::System, v_mt, Some(v_ms));
        return;
    }
    let v_server = p_server.clone();
    spawn(async move {
        match v_server.db.delete_mission(field_cloned(&v_mms, "missionId"), v_account).await {
            Ok(_) => p_ctx.reply(&v_server, &mut p_jmsg, Routing::System, v_mt, Some(json!("Done"))),
            Err(e) => {
                dump_error(&format!("Error: {}", e));
                p_ctx.reply(
                    &v_server,
                    &mut p_jmsg,
                    Routing::System,
                    v_mt,
                    Some(json!("Error: Failed to delete mission from storage server")),
                );
            }
        }
    });
}
