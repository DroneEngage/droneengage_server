//! Shared test helpers (the Rust counterpart of test/helpers/test_globals.js):
//! an in-process server for a configuration, fake connected clients whose
//! outbound frames are captured, and capture hooks for the mesh and the
//! storage link.

#![allow(dead_code)]

use andruavserver::hub::{Client, ConnHandle, Login, Out, Status};
use andruavserver::{Config, Env, Server};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tokio_tungstenite::tungstenite::Message;

pub const SECRET: &str = "test-shared-secret-0123456789abcdef";

/// A server for p_config merged over a small default (nothing started).
pub fn server(p_config: Value) -> Arc<Server> {
    let mut v_cfg = json!({ "server_id": "SrvTest", "enable_super_server": false, "enable_persistant_relay": false });
    if let (Value::Object(base), Value::Object(over)) = (&mut v_cfg, p_config) {
        for (k, v) in over {
            base.insert(k, v);
        }
    }
    Server::new(
        Config::from_value(v_cfg, env!("CARGO_MANIFEST_DIR")),
        Env { s2s_shared_secret: Some(SECRET.into()), ..Default::default() },
        None,
    )
}

/// One frame a fake client received.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Text(Value),
    /// JSON header and the bytes after its 0 terminator
    Binary(Value, Vec<u8>),
    Close,
}

impl Frame {
    pub fn json(&self) -> &Value {
        match self {
            Frame::Text(v) | Frame::Binary(v, _) => v,
            Frame::Close => &Value::Null,
        }
    }
}

pub fn decode(p_out: Out) -> Frame {
    match p_out {
        Out::Text(t) => Frame::Text(serde_json::from_str(t.as_str()).unwrap_or(Value::String(t.to_string()))),
        Out::Binary(b) => {
            let i = b.iter().position(|x| *x == 0).unwrap_or(b.len());
            let v_header = serde_json::from_slice(&b[..i]).unwrap_or(Value::Null);
            Frame::Binary(v_header, b.get(i + 1..).map(|t| t.to_vec()).unwrap_or_default())
        }
        Out::Close => Frame::Close,
    }
}

pub struct FakeClient {
    pub conn: u64,
    pub handle: Arc<ConnHandle>,
    pub rx: UnboundedReceiver<Out>,
}

impl FakeClient {
    /// Everything received so far.
    pub fn frames(&mut self) -> Vec<Frame> {
        let mut v_out = Vec::new();
        while let Ok(o) = self.rx.try_recv() {
            v_out.push(decode(o));
        }
        v_out
    }

    /// The JSON of everything received so far.
    pub fn sent(&mut self) -> Vec<Value> {
        self.frames().into_iter().map(|f| f.json().clone()).collect()
    }

    /// Waits up to 2 s for at least p_n frames.
    pub async fn wait_frames(&mut self, p_n: usize) -> Vec<Frame> {
        let mut v_out = Vec::new();
        let v_end = tokio::time::Instant::now() + Duration::from_secs(2);
        while v_out.len() < p_n {
            match tokio::time::timeout_at(v_end, self.rx.recv()).await {
                Ok(Some(o)) => v_out.push(decode(o)),
                _ => break,
            }
        }
        v_out
    }
}

/// Options of a fake logged-in party.
pub struct Party<'a> {
    pub name: &'a str,
    pub actor: &'a str,
    pub account: Value,
    pub group: Value,
    pub prm: Option<Value>,
    pub ip: Option<&'a str>,
}

impl<'a> Party<'a> {
    pub fn new(p_name: &'a str, p_actor: &'a str) -> Party<'a> {
        Party {
            name: p_name,
            actor: p_actor,
            account: json!("acc1"),
            group: json!("g1"),
            prm: Some(json!(0xffff_ffffu64)),
            ip: None,
        }
    }

    pub fn account(mut self, p_account: Value) -> Self {
        self.account = p_account;
        self
    }

    pub fn group(mut self, p_group: Value) -> Self {
        self.group = p_group;
        self
    }

    pub fn prm(mut self, p_prm: Value) -> Self {
        self.prm = Some(p_prm);
        self
    }

    /// A login AUTH granted no permission.
    pub fn no_prm(mut self) -> Self {
        self.prm = None;
        self
    }

    pub fn ip(mut self, p_ip: &'a str) -> Self {
        self.ip = Some(p_ip);
        self
    }
}

/// A logged-in party, as accept() leaves it: active sender, in its group.
pub fn connect(p_server: &Arc<Server>, p_party: Party) -> FakeClient {
    let (v_handle, v_rx) = ConnHandle::new(None);
    let v_conn = v_handle.id;
    let mut v_client = Client::new(v_handle.clone(), p_party.ip.map(str::to_string));
    v_client.login = Some(Login {
        sender_id: Some(json!(p_party.name)),
        account_id: Some(p_party.account),
        group_id: Some(p_party.group),
        request_id: Some(json!(p_party.name)),
        actor_type: Some(json!(p_party.actor)),
        prm: p_party.prm,
        creation_date: 0,
    });
    v_client.status = Some(Status::default());
    let mut v_hub = p_server.hub();
    v_hub.clients.insert(v_conn, v_client);
    v_hub.add_active_sender(Some(&json!(p_party.name)), v_conn);
    v_hub.add_member_to_account_group(v_conn);
    drop(v_hub);
    FakeClient { conn: v_conn, handle: v_handle, rx: v_rx }
}

/// Sends a text message from a party (fn_parseMessage).
pub fn send(p_server: &Arc<Server>, p_from: &FakeClient, p_msg: Value) {
    andruavserver::chat::routing::parse_message(
        p_server,
        p_from.conn,
        andruavserver::util::js_stringify(&p_msg).as_bytes(),
        false,
    );
}

/// Sends a binary message: JSON header, 0, payload.
pub fn send_binary(p_server: &Arc<Server>, p_from: &FakeClient, p_header: Value, p_payload: &[u8]) {
    let mut v_data = andruavserver::util::js_stringify(&p_header).into_bytes();
    v_data.push(0);
    v_data.extend_from_slice(p_payload);
    andruavserver::chat::routing::parse_message(p_server, p_from.conn, &v_data, true);
}

/// Captures what is forwarded to the mesh: (to the children, to the parent).
pub struct MeshCapture {
    pub to_children: FakeClient,
    pub to_parent: UnboundedReceiver<Message>,
}

pub fn capture_mesh(p_server: &Arc<Server>) -> MeshCapture {
    let (v_handle, v_rx) = ConnHandle::new(None);
    let v_conn = p_server.parent.add_child_for_test(v_handle.clone(), true);
    let (v_tx, v_parent_rx) = unbounded_channel();
    p_server.child.set_link_for_test(Some(v_tx));
    MeshCapture { to_children: FakeClient { conn: v_conn, handle: v_handle, rx: v_rx }, to_parent: v_parent_rx }
}

/// The header of a relayed message (text or binary).
pub fn header(p_bytes: &[u8]) -> Value {
    let i = p_bytes.iter().position(|x| *x == 0).unwrap_or(p_bytes.len());
    serde_json::from_slice(&p_bytes[..i]).unwrap()
}

/// A fake, connected storage server link: requests arrive on the receiver.
pub fn storage_link(p_server: &Arc<Server>) -> UnboundedReceiver<Message> {
    let (v_tx, v_rx) = unbounded_channel();
    p_server.db.set_connected_for_test(Some(v_tx));
    v_rx
}

/// Waits for the next storage request and answers it with p_reply (rid added).
pub async fn answer_storage(p_server: &Arc<Server>, p_rx: &mut UnboundedReceiver<Message>, p_reply: Value) -> Value {
    let v_msg = tokio::time::timeout(Duration::from_secs(2), p_rx.recv()).await.expect("storage request").unwrap();
    let v_req: Value = serde_json::from_str(v_msg.to_text().unwrap()).unwrap();
    let mut v_reply = p_reply;
    v_reply["rid"] = v_req["rid"].clone();
    p_server.db.inject_message(p_server, v_reply.to_string().as_bytes());
    v_req
}

/// Lets spawned handlers run.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(30)).await;
}
