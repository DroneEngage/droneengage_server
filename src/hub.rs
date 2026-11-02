//! Shared connection state of the comm server.
//!
//! The JavaScript server kept this state on the ws socket objects
//! (`m_loginRequest`, `m_status`, `name`, `m__group`, `m__terminated`) and in
//! module-level maps (js_andruav_active_senders.js, the account rooms of
//! js_andruav_chat_account_rooms.js, the waiting logins of
//! js_andruav_comm_server.js). Here it all lives in one `Hub` behind a mutex
//! that is never held across an await. A local message takes it twice, once to
//! read what the sender may do and once to deliver; parsing and building the
//! frame run in between, unlocked, so other sockets can act between the two.
//! A socket's own messages stay in order because its read loop handles them
//! one at a time.
//!
//! A `ConnHandle` is what other code keeps of a socket: its outbound queue,
//! the number of bytes queued and not yet written (ws.bufferedAmount), its
//! ready state and terminate().

use indexmap::{IndexMap, IndexSet};
use serde_json::Value;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::{Bytes, Utf8Bytes};

use crate::util::{js_key, js_strict_eq};

pub const READY_CONNECTING: u8 = 0;
pub const READY_OPEN: u8 = 1;
pub const READY_CLOSING: u8 = 2;
pub const READY_CLOSED: u8 = 3;

/// One outbound frame.
#[derive(Debug, Clone)]
pub enum Out {
    Text(Utf8Bytes),
    Binary(Bytes),
    /// close frame without a status code (ws.close())
    Close,
}

impl Out {
    pub fn len(&self) -> usize {
        match self {
            Out::Text(t) => t.len(),
            Out::Binary(b) => b.len(),
            Out::Close => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A message as routed: text or binary, shared between recipients.
#[derive(Debug, Clone)]
pub enum Payload {
    Text(Utf8Bytes),
    Binary(Bytes),
}

impl Payload {
    pub fn text(p_text: String) -> Payload {
        Payload::Text(Utf8Bytes::from(p_text))
    }

    pub fn binary(p_bytes: Vec<u8>) -> Payload {
        Payload::Binary(Bytes::from(p_bytes))
    }

    pub fn is_binary(&self) -> bool {
        matches!(self, Payload::Binary(_))
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Payload::Text(t) => t.as_bytes(),
            Payload::Binary(b) => b,
        }
    }

    /// ws.send(data, {binary}): a text payload sent as binary keeps its bytes.
    pub fn with_binary(&self, p_binary: bool) -> Out {
        match (self, p_binary) {
            (Payload::Text(t), false) => Out::Text(t.clone()),
            (Payload::Binary(b), true) => Out::Binary(b.clone()),
            (Payload::Text(t), true) => Out::Binary(Bytes::copy_from_slice(t.as_bytes())),
            (Payload::Binary(b), false) => match std::str::from_utf8(b) {
                Ok(s) => Out::Text(Utf8Bytes::from(s.to_string())),
                Err(_) => Out::Binary(b.clone()),
            },
        }
    }
}

/// What the rest of the server keeps of a WebSocket.
pub struct ConnHandle {
    pub id: u64,
    m_tx: UnboundedSender<Out>,
    m_buffered: Arc<AtomicUsize>,
    m_state: AtomicU8,
    m_kill: Notify,
    pub remote_addr: Option<SocketAddr>,
}

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

impl ConnHandle {
    /// A handle and the receiving end of its outbound queue. The writer
    /// subtracts each frame's length from `buffered()` once written.
    pub fn new(p_remote: Option<SocketAddr>) -> (Arc<ConnHandle>, UnboundedReceiver<Out>) {
        let (v_tx, v_rx) = unbounded_channel();
        let v_handle = ConnHandle {
            id: NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed),
            m_tx: v_tx,
            m_buffered: Arc::new(AtomicUsize::new(0)),
            m_state: AtomicU8::new(READY_OPEN),
            m_kill: Notify::new(),
            remote_addr: p_remote,
        };
        (Arc::new(v_handle), v_rx)
    }

    /// ws.readyState
    pub fn ready_state(&self) -> u8 {
        self.m_state.load(Ordering::SeqCst)
    }

    pub fn is_open(&self) -> bool {
        self.ready_state() == READY_OPEN
    }

    /// ws.bufferedAmount
    pub fn buffered(&self) -> usize {
        self.m_buffered.load(Ordering::SeqCst)
    }

    /// The counter the writer decrements.
    pub fn buffered_counter(&self) -> Arc<AtomicUsize> {
        self.m_buffered.clone()
    }

    /// Test hook: pretend this many bytes are queued.
    pub fn set_buffered(&self, p_bytes: usize) {
        self.m_buffered.store(p_bytes, Ordering::SeqCst);
    }

    /// ws.send(): queued while OPEN, silently dropped otherwise.
    pub fn send(&self, p_out: Out) {
        if !self.is_open() {
            return;
        }
        self.m_buffered.fetch_add(p_out.len(), Ordering::SeqCst);
        let _ = self.m_tx.send(p_out);
    }

    /// ws.send(JSON.stringify(obj))
    pub fn send_text(&self, p_text: String) {
        self.send(Out::Text(Utf8Bytes::from(p_text)));
    }

    /// ws.send(JSON.stringify(p_value))
    pub fn send_json(&self, p_value: &Value) {
        self.send_text(crate::util::js_stringify(p_value));
    }

    /// ws.close(): close frame, then the socket closes when the peer answers.
    pub fn close(&self) {
        if self.m_state.compare_exchange(READY_OPEN, READY_CLOSING, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            let _ = self.m_tx.send(Out::Close);
        }
    }

    /// The peer sent its close frame (answered by the WebSocket layer): no
    /// more sends.
    pub fn mark_closing(&self) {
        let _ = self.m_state.compare_exchange(READY_OPEN, READY_CLOSING, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// ws.terminate(): the socket is destroyed now.
    pub fn terminate(&self) {
        let v_old = self.m_state.swap(READY_CLOSED, Ordering::SeqCst);
        if v_old != READY_CLOSED {
            self.m_kill.notify_one();
        }
    }

    /// The connection task marks the socket closed when it ends.
    pub fn mark_closed(&self) {
        self.m_state.store(READY_CLOSED, Ordering::SeqCst);
    }

    /// Resolves when terminate() is called.
    pub async fn killed(&self) {
        self.m_kill.notified().await
    }
}

/// m_loginRequest: who a socket is, from AUTH's login request (or the local
/// defaults). Values keep their JSON type; None is `undefined`.
#[derive(Debug, Clone, Default)]
pub struct Login {
    pub sender_id: Option<Value>,
    pub account_id: Option<Value>,
    pub group_id: Option<Value>,
    pub request_id: Option<Value>,
    pub actor_type: Option<Value>,
    pub prm: Option<Value>,
    pub creation_date: i64,
}

impl Login {
    pub fn actor_is(&self, p_actor: &str) -> bool {
        self.actor_type.as_ref().and_then(Value::as_str) == Some(p_actor)
    }

    /// String(m_accountID)
    pub fn account_key(&self) -> String {
        js_key(self.account_id.as_ref())
    }

    /// String(m_senderID)
    pub fn sender_key(&self) -> String {
        js_key(self.sender_id.as_ref())
    }
}

/// m_status
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub ttx: u64,
    pub btx: u64,
    pub dropped: u64,
    pub terminated_for_backlog: u64,
}

/// p_ws.m__group: the group a socket is in. Two refs are the same group
/// object when their uid matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRef {
    // shared text: routing copies a socket's ref for every message
    pub account_key: Arc<str>,
    pub group_key: Arc<str>,
    pub uid: u64,
}

/// A connected socket (the ws object and the fields the server set on it).
pub struct Client {
    pub handle: Arc<ConnHandle>,
    pub login: Option<Login>,
    /// p_ws.name (the unit name, set when it joins a group)
    pub name: Option<Value>,
    pub group: Option<GroupRef>,
    pub status: Option<Status>,
    /// m__terminated: kicked out by a newer login with the same senderID
    pub terminated: bool,
    /// m_remoteIp
    pub remote_ip: Option<String>,
}

impl Client {
    pub fn new(p_handle: Arc<ConnHandle>, p_remote_ip: Option<String>) -> Client {
        Client {
            handle: p_handle,
            login: None,
            name: None,
            group: None,
            status: None,
            terminated: false,
            remote_ip: p_remote_ip,
        }
    }

    /// p_ws.name as a string key (String(name))
    pub fn name_key(&self) -> String {
        js_key(self.name.as_ref())
    }
}

pub struct Group {
    /// m_ID (the group id as AUTH sent it)
    pub id: Value,
    pub uid: u64,
    /// unit name -> socket
    pub units: IndexMap<String, u64>,
    pub creation_date: i64,
}

pub struct Account {
    /// m_accountID
    pub id: Value,
    pub groups: IndexMap<String, Group>,
}

/// All connection state (see the module comment).
#[derive(Default)]
pub struct Hub {
    pub clients: HashMap<u64, Client>,
    /// account rooms: String(accountID) -> account
    pub accounts: IndexMap<String, Account>,
    /// active senders: String(senderID) -> socket
    pub active: HashMap<String, u64>,
    /// admin-blocked senders (in memory only)
    pub blocked: IndexSet<String>,
    /// logins announced by AUTH and not yet used: temp key -> login request
    pub waiting: HashMap<String, Value>,
    pub(crate) m_next_group_uid: u64,
}

impl Hub {
    pub fn new() -> Hub {
        Hub::default()
    }

    // ------------------------------------------------------------------
    // active senders (js_andruav_active_senders.js)
    // ------------------------------------------------------------------

    pub fn add_active_sender(&mut self, p_sender: Option<&Value>, p_conn: u64) {
        self.active.insert(js_key(p_sender), p_conn);
    }

    pub fn delete_active_sender(&mut self, p_sender: Option<&Value>) {
        if crate::util::js_nullish(p_sender) {
            return;
        }
        self.active.remove(&js_key(p_sender));
    }

    /// getActiveSender(senderID): the socket registered under String(senderID).
    pub fn active_sender(&self, p_sender: Option<&Value>) -> Option<u64> {
        self.active.get(&js_key(p_sender)).copied().filter(|id| self.clients.contains_key(id))
    }

    pub fn active_sender_by_name(&self, p_name: &str) -> Option<u64> {
        self.active.get(p_name).copied().filter(|id| self.clients.contains_key(id))
    }

    /// Blocks or resumes inbound traffic of a connected unit. False when the
    /// name is neither connected nor already blocked.
    pub fn set_sender_blocked(&mut self, p_sender: &str, p_blocked: bool) -> bool {
        if p_blocked && !self.active.contains_key(p_sender) && !self.blocked.contains(p_sender) {
            return false;
        }
        if p_blocked {
            self.blocked.insert(p_sender.to_string());
        } else {
            self.blocked.shift_remove(p_sender);
        }
        true
    }

    /// fn_isSenderBlocked: only string ids can be in the set.
    pub fn is_sender_blocked(&self, p_sender: Option<&Value>) -> bool {
        match p_sender {
            Some(Value::String(s)) => self.blocked.contains(s),
            _ => false,
        }
    }

    pub fn blocked_senders(&self) -> Vec<String> {
        self.blocked.iter().cloned().collect()
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    pub fn client(&self, p_id: u64) -> Option<&Client> {
        self.clients.get(&p_id)
    }

    pub fn client_mut(&mut self, p_id: u64) -> Option<&mut Client> {
        self.clients.get_mut(&p_id)
    }

    pub fn group(&self, p_ref: &GroupRef) -> Option<&Group> {
        self.accounts.get(&*p_ref.account_key)?.groups.get(&*p_ref.group_key).filter(|g| g.uid == p_ref.uid)
    }

    /// `p_a.m__group === p_b.m__group` (both undefined counts as equal).
    pub fn same_group(p_a: Option<&GroupRef>, p_b: Option<&GroupRef>) -> bool {
        match (p_a, p_b) {
            (None, None) => true,
            (Some(a), Some(b)) => a.uid == b.uid,
            _ => false,
        }
    }

    /// `socket.m__group.m_ID === p_group_id`
    pub fn group_id_is(&self, p_ref: Option<&GroupRef>, p_group_id: Option<&Value>) -> bool {
        match p_ref.and_then(|r| self.group(r)) {
            Some(g) => js_strict_eq(Some(&g.id), p_group_id),
            None => false,
        }
    }
}
