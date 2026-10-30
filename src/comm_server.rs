//! AUTH-facing side of the comm server (js_andruav_comm_server.js): logins
//! announced by AUTH wait here for their party to connect, and AUTH's
//! dashboard commands (UDP proxies, unit traffic) are answered.

use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;

use crate::constants::*;
use crate::server::Server;
use crate::util::{is_true, js_parse_bytes, js_stringify, uuid_v4};

/// A login waits this long for its party to connect.
pub const WAIT_PARTY_TO_CONNECT_TIMEOUT: Duration = Duration::from_millis(10000);

/// Adds a login AUTH announced; it is forgotten after the timeout.
pub fn add_waiting_account(p_server: &Arc<Server>, p_key: String, p_login: Value) {
    p_server.hub().waiting.insert(p_key.clone(), p_login);
    let v_server = p_server.clone();
    tokio::spawn(async move {
        tokio::time::sleep(WAIT_PARTY_TO_CONNECT_TIMEOUT).await;
        v_server.hub().waiting.remove(&p_key);
    });
}

/// fn_generateLoginRequestReply: the temp key plus this server's public
/// host and port, for AUTH to hand to the party.
pub fn login_request_reply(p_server: &Server, p_cmd: &Value) -> Value {
    let v_d = p_cmd.get("d");
    let mut v_reply_d = Map::new();
    if let Some(r) = v_d.and_then(|d| d.get(CONST_CS_REQUEST_ID)) {
        v_reply_d.insert(CONST_CS_REQUEST_ID.into(), r.clone());
    }
    v_reply_d.insert(CONST_CS_ERROR.into(), json!(CONST_ERROR_NON));
    if let Some(h) = p_server.cfg.get("public_host") {
        v_reply_d.insert(CONST_CS_SERVER_PUBLIC_HOST.into(), h.clone());
    }
    if let Some(p) = p_server.server_port_value() {
        v_reply_d.insert(CONST_CS_SERVER_PORT.into(), p);
    }
    if let Some(f) = v_d.and_then(|d| d.get(CONST_CS_LOGIN_TEMP_KEY)) {
        v_reply_d.insert(CONST_CS_LOGIN_TEMP_KEY.into(), f.clone());
    }
    json!({ "c": CONST_CS_CMD_LOGIN_REQUEST, "d": Value::Object(v_reply_d) })
}

/// fn_handleLoginResponses: gives the login a temp key, keeps it waiting and
/// replies to AUTH.
fn handle_login_request(p_server: &Arc<Server>, mut p_cmd: Value) {
    let v_key = uuid_v4().replace('-', "");
    if let Some(Value::Object(d)) = p_cmd.get_mut("d") {
        d.insert(CONST_CS_LOGIN_TEMP_KEY.into(), Value::String(v_key.clone()));
    }
    add_waiting_account(p_server, v_key, p_cmd["d"].clone());
    println!("New Party: {}", js_stringify(&p_cmd));
    let v_reply = login_request_reply(p_server, &p_cmd);
    p_server.auth.send_message(js_stringify(&v_reply));
}

/// fn_AuthServerMessagesHandler: one message from AUTH.
pub fn on_auth_message(p_server: &Arc<Server>, p_data: &[u8]) {
    let Some(v_cmd) = js_parse_bytes(p_data) else {
        println!("SyntaxError: Unexpected token in JSON");
        return;
    };
    let Some(v_c) = v_cmd.as_object().and_then(|o| o.get("c")) else { return };
    match v_c.as_str() {
        Some(CONST_CS_CMD_LOGIN_REQUEST) => {
            // d must be an object without its own 'd'
            match v_cmd.get("d") {
                Some(Value::Object(d)) if !d.contains_key("d") => handle_login_request(p_server, v_cmd.clone()),
                Some(Value::Object(_)) | None => {}
                Some(v_d) => println!(
                    "err: fn_AuthServerMessagesHandler: TypeError: Cannot create property 'f' on {}",
                    crate::util::js_stringify(v_d)
                ),
            }
        }
        Some(CONST_CS_CMD_LOGOUT_REQUEST) => {
            // not implemented (as in the JavaScript server)
        }
        Some(CONST_CS_CMD_QUERY_UDP_PROXIES) => {
            let v_server = p_server.clone();
            tokio::spawn(async move { handle_query_udp_proxies(&v_server, &v_cmd).await });
        }
        Some(CONST_CS_CMD_SET_UDP_PROXY_STATE) => {
            let v_server = p_server.clone();
            tokio::spawn(async move { handle_set_udp_proxy_state(&v_server, &v_cmd).await });
        }
        Some(CONST_CS_CMD_SET_UNIT_TRAFFIC_STATE) => handle_set_unit_traffic_state(p_server, &v_cmd),
        _ => {}
    }
}

fn request_id(p_cmd: &Value) -> Option<Value> {
    match p_cmd.get("d") {
        Some(Value::Object(d)) => d.get(CONST_CS_REQUEST_ID).cloned(),
        _ => None,
    }
}

/// Built-in proxies plus the relay pairs this comm server owns.
pub async fn all_udp_proxies(p_server: &Arc<Server>) -> Vec<Value> {
    let mut v_rows = p_server.udp.active_proxies_list(p_server).await;
    if p_server.relay.is_enabled() {
        v_rows.extend(p_server.relay.relay_proxies_list(p_server).await);
    }
    v_rows
}

/// `e`: replies `f` with the open UDP proxies.
pub async fn handle_query_udp_proxies(p_server: &Arc<Server>, p_cmd: &Value) {
    let mut v_d = Map::new();
    v_d.insert("proxies".into(), Value::Array(all_udp_proxies(p_server).await));
    if let Some(r) = request_id(p_cmd) {
        v_d.insert(CONST_CS_REQUEST_ID.into(), r);
    }
    let v_reply = json!({ "c": CONST_CS_CMD_REPORT_UDP_PROXIES, "d": Value::Object(v_d) });
    p_server.auth.send_message(js_stringify(&v_reply));
}

/// `g` {name, blocked, account?}: blocks/resumes forwarding on a named
/// proxy (and the relay pairs of that unit); replies `f`.
pub async fn handle_set_udp_proxy_state(p_server: &Arc<Server>, p_cmd: &Value) {
    let v_d = p_cmd.get("d").filter(|d| crate::util::js_truthy(Some(d)));
    let v_name = v_d.and_then(|d| crate::util::field(d, "name")).cloned();
    let v_blocked = v_d.map(|d| is_true(crate::util::field(d, "blocked"))).unwrap_or(false);
    let v_blocked_out = blocked_value(p_cmd, v_blocked);
    let v_account = v_d.and_then(|d| crate::util::field(d, "account")).cloned();
    let mut v_applied = false;
    if let Some(Value::String(v_n)) = &v_name {
        v_applied = p_server.udp.set_proxy_blocked(v_n, v_blocked, v_account.as_ref()).await;
        if p_server.relay.is_enabled() {
            let v_relay_applied =
                p_server.relay.set_relay_proxy_blocked(p_server, v_n, v_blocked, v_account.as_ref()).await;
            v_applied = v_applied || v_relay_applied;
        }
    }
    if !v_applied {
        println!("[WARN] SET_UDP_PROXY_STATE for unknown proxy '{}'", crate::util::js_key(v_name.as_ref()));
    }
    let mut v_reply_d = Map::new();
    v_reply_d.insert("proxies".into(), Value::Array(all_udp_proxies(p_server).await));
    insert_name(&mut v_reply_d, v_d.is_some(), v_name.clone());
    if let Some(b) = v_blocked_out {
        v_reply_d.insert("blocked".into(), b);
    }
    v_reply_d.insert("applied".into(), json!(v_applied));
    if let Some(a) = v_account.filter(|a| !a.is_null()) {
        v_reply_d.insert("account".into(), a);
    }
    if let Some(r) = request_id(p_cmd) {
        v_reply_d.insert(CONST_CS_REQUEST_ID.into(), r);
    }
    let v_reply = json!({ "c": CONST_CS_CMD_REPORT_UDP_PROXIES, "d": Value::Object(v_reply_d) });
    p_server.auth.send_message(js_stringify(&v_reply));
}

/// `blocked: (p_cmd.d && p_cmd.d.blocked === true)`: a falsy d itself
/// (undefined is omitted).
fn blocked_value(p_cmd: &Value, p_blocked: bool) -> Option<Value> {
    match p_cmd.get("d") {
        Some(d) if crate::util::js_truthy(Some(d)) => Some(json!(p_blocked)),
        Some(d) => Some(d.clone()),
        None => None,
    }
}

/// `name: p_cmd.d ? p_cmd.d.name : null` in a reply (undefined is omitted).
fn insert_name(p_d: &mut Map<String, Value>, p_has_d: bool, p_name: Option<Value>) {
    match (p_has_d, p_name) {
        (false, _) => {
            p_d.insert("name".into(), Value::Null);
        }
        (true, Some(n)) => {
            p_d.insert("name".into(), n);
        }
        (true, None) => {}
    }
}

/// `h` {name, blocked}: blocks/resumes inbound traffic of a connected unit;
/// replies `i` with the blocked units.
pub fn handle_set_unit_traffic_state(p_server: &Arc<Server>, p_cmd: &Value) {
    let v_d = p_cmd.get("d").filter(|d| crate::util::js_truthy(Some(d)));
    let v_name = v_d.and_then(|d| crate::util::field(d, "name")).cloned();
    let v_blocked = v_d.map(|d| is_true(crate::util::field(d, "blocked"))).unwrap_or(false);
    let v_blocked_out = blocked_value(p_cmd, v_blocked);
    let (v_applied, v_units) = {
        let mut v_hub = p_server.hub();
        let v_applied = match &v_name {
            Some(Value::String(n)) => v_hub.set_sender_blocked(n, v_blocked),
            _ => false,
        };
        (v_applied, v_hub.blocked_senders())
    };
    if !v_applied {
        println!("[WARN] SET_UNIT_TRAFFIC_STATE for unknown unit '{}'", crate::util::js_key(v_name.as_ref()));
    }
    let mut v_reply_d = Map::new();
    v_reply_d.insert("units".into(), json!(v_units));
    insert_name(&mut v_reply_d, v_d.is_some(), v_name);
    if let Some(b) = v_blocked_out {
        v_reply_d.insert("blocked".into(), b);
    }
    v_reply_d.insert("applied".into(), json!(v_applied));
    if let Some(r) = request_id(p_cmd) {
        v_reply_d.insert(CONST_CS_REQUEST_ID.into(), r);
    }
    let v_reply = json!({ "c": CONST_CS_CMD_REPORT_BLOCKED_UNITS, "d": Value::Object(v_reply_d) });
    p_server.auth.send_message(js_stringify(&v_reply));
}

/// fn_cancelLoginRequestBySenderID: drops the waiting login whose request id
/// is p_request_id (loose equality) and tells AUTH.
pub fn cancel_login_request_by_sender_id(p_server: &Arc<Server>, p_request_id: Option<&Value>) {
    let Some(v_request_id) = p_request_id.filter(|v| !v.is_null()) else { return };
    let v_removed = {
        let mut v_hub = p_server.hub();
        let v_key = v_hub
            .waiting
            .iter()
            .find(|(_, l)| {
                let v_r = l.get(CONST_CS_REQUEST_ID);
                v_r.map(|r| crate::util::js_to_string(r) == crate::util::js_to_string(v_request_id)).unwrap_or(false)
            })
            .map(|(k, _)| k.clone());
        v_key.map(|k| v_hub.waiting.remove(&k)).is_some()
    };
    if v_removed && p_server.uses_auth() {
        p_server.update_auth_server();
    }
}
