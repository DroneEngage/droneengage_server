//! The chat server: client connections, routing, rooms, system commands,
//! S2S mesh forwarding and command plugins (src/comm_server/chat_server/*).

pub mod connection;
pub mod news;
pub mod outbound_guard;
pub mod plugins;
pub mod query_server;
pub mod relay;
pub mod rooms;
pub mod routing;
pub mod system_commands;
pub mod tasks;

use serde_json::Value;
use std::sync::Arc;

use crate::constants::{CONST_WS_MSG_ROUTING_INDIVIDUAL, CONST_WS_MSG_ROUTING_SYSTEM, CONST_WS_SENDER_COMM_SERVER};
use crate::hub::{ConnHandle, Hub, Login};
use crate::server::Server;
use crate::util::{set_field, set_field_opt};

/// The socket a system command came from (what the JavaScript handlers kept
/// as p_ws, possibly across an await).
#[derive(Clone)]
pub struct Ctx {
    pub conn: u64,
    pub handle: Arc<ConnHandle>,
    pub name: Option<Value>,
    pub login: Option<Login>,
}

impl Ctx {
    pub fn from_hub(p_hub: &Hub, p_conn: u64) -> Option<Ctx> {
        let v_client = p_hub.client(p_conn)?;
        Some(Ctx {
            conn: p_conn,
            handle: v_client.handle.clone(),
            name: v_client.name.clone(),
            login: v_client.login.clone(),
        })
    }

    /// p_ws.name now (it can change while a request is in flight).
    pub fn name_now(&self, p_server: &Server) -> Option<Value> {
        match p_server.hub().client(self.conn) {
            Some(c) => c.name.clone(),
            None => self.name.clone(),
        }
    }

    /// p_ws.m_loginRequest.m_accountID, null when there is no login.
    pub fn account_id(&self) -> Option<Value> {
        self.login.as_ref().and_then(|l| l.account_id.clone())
    }

    /// Replies on this socket after setting ty/tg/sd/mt/ms on the request
    /// object (the JavaScript handlers reused v_jmsg for the reply).
    pub fn reply(
        &self,
        p_server: &Server,
        p_jmsg: &mut Value,
        p_routing: Routing,
        p_mt: Option<Value>,
        p_ms: Option<Value>,
    ) {
        let v_ty = match p_routing {
            Routing::Individual => CONST_WS_MSG_ROUTING_INDIVIDUAL,
            Routing::System => CONST_WS_MSG_ROUTING_SYSTEM,
        };
        set_field(p_jmsg, "ty", Value::String(v_ty.into()));
        set_field_opt(p_jmsg, "tg", self.name_now(p_server));
        set_field(p_jmsg, "sd", Value::String(CONST_WS_SENDER_COMM_SERVER.into()));
        set_field_opt(p_jmsg, "mt", p_mt);
        set_field_opt(p_jmsg, "ms", p_ms);
        self.handle.send_json(p_jmsg);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Routing {
    Individual,
    System,
}

/// fn_parseMessageBody: v_jmsg.ms, JSON-parsed when it is a string
/// (backward compatible). None where JavaScript would throw: an unparsable
/// string, or a null/undefined body whose fields are then read.
pub fn parse_message_body(p_jmsg: &Value) -> Option<Value> {
    match p_jmsg.get("ms") {
        Some(Value::String(s)) => match crate::util::js_parse(s) {
            Some(Value::Null) | None => None,
            Some(v) => Some(v),
        },
        None | Some(Value::Null) => None,
        Some(v) => Some(v.clone()),
    }
}

/// dumperror.fn_dumpdebug for a value: strings are printed, objects only
/// when they carry a message.
pub fn dump_debug(p_value: &Value) {
    match p_value {
        Value::String(s) => println!("{}", s),
        Value::Object(m) => {
            if let Some(v) = m.get("message").filter(|v| crate::util::js_truthy(Some(v))) {
                println!("\nMessage: {}", crate::util::js_to_string(v));
            }
        }
        _ => {}
    }
}

/// A thrown error inside a command handler (caught and printed by
/// onSystemMessage's try/catch).
pub fn dump_error(p_text: &str) {
    println!("{}", p_text);
}

/// Spawns an async command handler.
pub fn spawn<F>(p_fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(p_fut);
}
