//! DBProxyClient (js_db_proxy_client.js): the comm server's link OUT to the
//! storage server (storage_server_host:storage_server_port, always wss with
//! self-signed certificates accepted). Task, mission and news requests are
//! sent as `{mt, ms, rid}` and resolved by the reply with the same rid
//! (10 s timeout). The link answers the storage server's S2S challenge
//! (Ed25519 or HMAC, per the challenge `method`) and fails closed when no
//! `auth_success` arrives in time. Connection status is reported to AUTH as
//! `d` STORAGE_STATUS; unsolicited NewsPush (9018) messages go to the news
//! fan-out.

use futures_util::StreamExt;
use rand::Rng;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};

use crate::colors as c;
use crate::constants::*;
use crate::net::{cfg_str, frame_bytes, split_with_writer, ws_connect, TlsVerify};
use crate::s2s::s2s_auth::{parse_envelope, S2S_AUTH_CHALLENGE, S2S_AUTH_HANDSHAKE_TIMEOUT_MS};
use crate::server::Server;
use crate::util::{js_parse_bytes, js_stringify, now_ms, uuid_v4};

const REQUEST_TIMEOUT: Duration = Duration::from_millis(10000);
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(30000);
const MAX_RECONNECT_DELAY_MS: f64 = 60000.0;
const INITIAL_RECONNECT_DELAY_MS: f64 = 1000.0;

type Pending = oneshot::Sender<Result<Value, String>>;

struct DbState {
    tx: Option<UnboundedSender<Message>>,
    open: bool,
    authenticated: bool,
    endpoint: Option<(Value, Value)>,
    state: &'static str,
    pending: HashMap<String, Pending>,
    reconnect_attempts: u32,
    last_sent_status: Option<String>,
    heartbeat: Option<JoinHandle<()>>,
}

pub struct DbProxyClient {
    m_state: Mutex<DbState>,
}

impl Default for DbProxyClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DbProxyClient {
    pub fn new() -> DbProxyClient {
        DbProxyClient {
            m_state: Mutex::new(DbState {
                tx: None,
                open: false,
                authenticated: false,
                endpoint: None,
                state: CONST_STORAGE_STATE_DISCONNECTED,
                pending: HashMap::new(),
                reconnect_attempts: 0,
                last_sent_status: None,
                heartbeat: None,
            }),
        }
    }

    /// fn_isConnected: open and authenticated.
    pub fn is_connected(&self) -> bool {
        let st = self.m_state.lock().unwrap();
        st.open && st.authenticated && st.tx.is_some()
    }

    /// fn_getConnectionState
    pub fn connection_state(&self) -> &'static str {
        self.m_state.lock().unwrap().state
    }

    /// Test hook: pretend the link is up, replying to requests through p_tx.
    #[doc(hidden)]
    pub fn set_connected_for_test(&self, p_tx: Option<UnboundedSender<Message>>) {
        let mut st = self.m_state.lock().unwrap();
        st.open = p_tx.is_some();
        st.authenticated = p_tx.is_some();
        st.state = if p_tx.is_some() { CONST_STORAGE_STATE_CONNECTED } else { CONST_STORAGE_STATE_DISCONNECTED };
        st.tx = p_tx;
    }

    /// Test hook: deliver a message as if the storage server sent it.
    #[doc(hidden)]
    pub fn inject_message(&self, p_server: &Arc<Server>, p_data: &[u8]) {
        self.on_message(p_server, p_data);
    }

    /// fn_initialize: connects when enable_storage_server is true.
    pub fn start(p_server: &Arc<Server>) {
        if !p_server.cfg.is_true("enable_storage_server") {
            println!("{}[INFO] DBProxyClient: storage server disabled in config{}", c::FG_YELLOW, c::RESET);
            return;
        }
        let v_host = p_server.cfg.get("storage_server_host").cloned();
        let v_port = p_server.cfg.get("storage_server_port").cloned();
        if !crate::util::js_truthy(v_host.as_ref()) || !crate::util::js_truthy(v_port.as_ref()) {
            println!(
                "{}[ATTENTION!!] DBProxyClient: storage_server_host or storage_server_port not configured{}",
                c::ERROR,
                c::RESET
            );
            return;
        }
        let (v_host, v_port) = (v_host.unwrap(), v_port.unwrap());
        println!(
            "{}[INFO] DBProxyClient configured for storage server at {}:{}{}",
            c::FG_YELLOW,
            cfg_str(Some(&v_host)),
            cfg_str(Some(&v_port)),
            c::RESET
        );
        p_server.db.m_state.lock().unwrap().endpoint = Some((v_host, v_port));
        let v_server = p_server.clone();
        tokio::spawn(async move {
            loop {
                v_server.db.run_once(&v_server).await;
                let v_delay = v_server.db.next_backoff();
                println!(
                    "{}[INFO] DBProxyClient reconnecting in {}ms (attempt {}){}",
                    c::FG_YELLOW,
                    v_delay,
                    v_server.db.m_state.lock().unwrap().reconnect_attempts,
                    c::RESET
                );
                tokio::time::sleep(Duration::from_millis(v_delay)).await;
            }
        });
    }

    fn next_backoff(&self) -> u64 {
        let mut st = self.m_state.lock().unwrap();
        st.reconnect_attempts += 1;
        let v_base =
            (INITIAL_RECONNECT_DELAY_MS * 2f64.powi(st.reconnect_attempts as i32 - 1)).min(MAX_RECONNECT_DELAY_MS);
        let v_jitter = v_base * 0.25 * (rand::thread_rng().gen::<f64>() * 2.0 - 1.0);
        (v_base + v_jitter).floor().max(0.0) as u64
    }

    async fn run_once(&self, p_server: &Arc<Server>) {
        let Some((v_host, v_port)) = self.m_state.lock().unwrap().endpoint.clone() else { return };
        self.m_state.lock().unwrap().state = CONST_STORAGE_STATE_CONNECTING;
        let (v_host_s, v_port_s) = (cfg_str(Some(&v_host)), cfg_str(Some(&v_port)));
        println!(
            "{}DBProxyClient connecting to storage server at wss://{}:{}{}",
            c::FG_YELLOW,
            v_host_s,
            v_port_s,
            c::RESET
        );
        let v_ws = match ws_connect(&v_host_s, &v_port_s, Some(&TlsVerify::None)).await {
            Ok(w) => w,
            Err(e) => {
                self.on_error(p_server, &e);
                self.on_close(p_server);
                return;
            }
        };
        let (v_tx, mut v_rx) = split_with_writer(v_ws);
        {
            let mut st = self.m_state.lock().unwrap();
            st.tx = Some(v_tx.clone());
            st.open = true;
            st.authenticated = false;
            st.reconnect_attempts = 0;
            st.state = CONST_STORAGE_STATE_CONNECTING;
        }
        println!("{}[OK] DBProxyClient connected to storage server{}", c::B_SUCCESS, c::RESET);
        self.send_storage_status(p_server, CONST_STORAGE_STATE_CONNECTING, None, true);

        // fail closed: a peer that never completes the handshake is dropped
        let v_auth_timer = {
            let v_server = p_server.clone();
            let v_tx = v_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(S2S_AUTH_HANDSHAKE_TIMEOUT_MS)).await;
                let v_still = {
                    let st = v_server.db.m_state.lock().unwrap();
                    !st.authenticated && st.open
                };
                if v_still {
                    println!(
                        "{}ATTENTION!! DBProxyClient: S2S auth not completed within {}ms — closing connection (fail closed){}",
                        c::ERROR,
                        S2S_AUTH_HANDSHAKE_TIMEOUT_MS,
                        c::RESET
                    );
                    let _ = v_tx.send(Message::Close(None));
                }
            })
        };

        while let Some(v_msg) = v_rx.next().await {
            match v_msg {
                Ok(m) => {
                    if let Some(v_data) = frame_bytes(&m) {
                        self.on_message(p_server, v_data);
                    }
                }
                Err(e) => {
                    if !crate::net::is_abrupt_close(&e) {
                        self.on_error(p_server, &e.to_string());
                    }
                    break;
                }
            }
        }
        v_auth_timer.abort();
        self.on_close(p_server);
    }

    fn on_message(&self, p_server: &Arc<Server>, p_data: &[u8]) {
        if let Some(v_env) = parse_envelope(p_data) {
            if v_env.get("s2s_auth").and_then(Value::as_str) == Some(S2S_AUTH_CHALLENGE) {
                let v_tx = self.m_state.lock().unwrap().tx.clone();
                if let Some(v_tx) = v_tx {
                    match p_server.s2s.answer_challenge(&v_env) {
                        Ok(r) => {
                            let _ = v_tx.send(Message::Text(Utf8Bytes::from(r)));
                        }
                        Err(e) => {
                            println!(
                                "{}ATTENTION!! DBProxyClient S2S handshake failed: Error: {}{}",
                                c::ERROR,
                                e,
                                c::RESET
                            );
                            let _ = v_tx.send(Message::Close(None));
                        }
                    }
                }
            }
            return;
        }
        let Some(v_msg) = js_parse_bytes(p_data) else {
            println!("err: DBProxyClient could not parse message: SyntaxError: Unexpected token in JSON");
            return;
        };
        if v_msg.get("type").and_then(Value::as_str) == Some("auth_success") {
            {
                let mut st = self.m_state.lock().unwrap();
                st.authenticated = true;
                st.state = CONST_STORAGE_STATE_CONNECTED;
            }
            println!("{}[OK] DBProxyClient authenticated with storage server{}", c::B_SUCCESS, c::RESET);
            self.send_storage_status(p_server, CONST_STORAGE_STATE_CONNECTED, None, true);
            self.start_heartbeat(p_server);
            return;
        }
        if v_msg.get("type").and_then(Value::as_str) == Some("error") {
            println!(
                "{}DBProxyClient received error from storage server: {}{}",
                c::ERROR,
                crate::util::js_key(v_msg.get("error")),
                c::RESET
            );
            return;
        }
        if let Some(v_rid) = v_msg.get("rid").filter(|r| !r.is_null()) {
            let v_key = crate::util::js_to_string(v_rid);
            let v_pending = self.m_state.lock().unwrap().pending.remove(&v_key);
            if let Some(p) = v_pending {
                let _ = p.send(Ok(v_msg));
                return;
            }
        }
        if crate::util::js_strict_eq(v_msg.get("mt"), Some(&json!(CONST_TYPE_ANDRUAV_SYSTEM_NEWS_PUSH))) {
            crate::chat::news::on_news_push(p_server, v_msg.get("ms"));
        }
    }

    fn on_error(&self, p_server: &Arc<Server>, p_error: &str) {
        println!("{}DBProxyClient WebSocket error: Error: {}{}", c::ERROR, p_error, c::RESET);
        self.m_state.lock().unwrap().state = CONST_STORAGE_STATE_UNHEALTHY;
        self.send_storage_status(p_server, CONST_STORAGE_STATE_ERROR, Some(p_error), true);
    }

    fn on_close(&self, p_server: &Arc<Server>) {
        println!("{}ATTENTION!! DBProxyClient disconnected from storage server{}", c::ERROR, c::RESET);
        let v_pending = {
            let mut st = self.m_state.lock().unwrap();
            st.authenticated = false;
            st.open = false;
            st.tx = None;
            st.state = CONST_STORAGE_STATE_DISCONNECTED;
            if let Some(h) = st.heartbeat.take() {
                h.abort();
            }
            std::mem::take(&mut st.pending)
        };
        self.send_storage_status(p_server, CONST_STORAGE_STATE_DISCONNECTED, Some("Connection closed"), true);
        for (_, p) in v_pending {
            let _ = p.send(Err("DBProxyClient disconnected".into()));
        }
    }

    fn start_heartbeat(&self, p_server: &Arc<Server>) {
        let v_server = p_server.clone();
        let v_task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT_INTERVAL).await;
                let v_status = if v_server.db.m_state.lock().unwrap().authenticated {
                    CONST_STORAGE_STATE_CONNECTED
                } else {
                    CONST_STORAGE_STATE_DISCONNECTED
                };
                v_server.db.send_storage_status(&v_server, v_status, None, false);
            }
        });
        if let Some(old) = self.m_state.lock().unwrap().heartbeat.replace(v_task) {
            old.abort();
        }
    }

    /// `d` STORAGE_STATUS to AUTH; an unchanged status is only sent when forced.
    fn send_storage_status(&self, p_server: &Arc<Server>, p_status: &str, p_error: Option<&str>, p_force: bool) {
        if !p_server.uses_auth() {
            return;
        }
        let v_endpoint = {
            let mut st = self.m_state.lock().unwrap();
            if !p_force && st.last_sent_status.as_deref() == Some(p_status) {
                return;
            }
            st.last_sent_status = Some(p_status.to_string());
            st.endpoint.clone()
        };
        let mut v_d = Map::new();
        v_d.insert("status".into(), json!(p_status));
        v_d.insert("storage_host".into(), v_endpoint.as_ref().map(|e| e.0.clone()).unwrap_or(Value::Null));
        v_d.insert("storage_port".into(), v_endpoint.as_ref().map(|e| e.1.clone()).unwrap_or(Value::Null));
        v_d.insert("timestamp".into(), json!(now_ms() / 1000));
        v_d.insert("error".into(), p_error.map(|e| json!(e)).unwrap_or(Value::Null));
        let v_cmd = json!({ "c": CONST_CS_CMD_STORAGE_STATUS, "d": Value::Object(v_d) });
        p_server.auth.send_message(js_stringify(&v_cmd));
    }

    /// fn_sendRequest: `{mt, ms, rid}` to the storage server; resolves with
    /// the reply envelope `{mt, ms, success, error?}`.
    pub async fn send_request(&self, p_mt: i64, p_ms: Value) -> Result<Value, String> {
        let v_rid = uuid_v4();
        let v_wait = {
            let mut st = self.m_state.lock().unwrap();
            if !(st.open && st.authenticated) {
                return Err("DBProxyClient is not connected to a storage server".into());
            }
            let Some(v_tx) = st.tx.clone() else {
                return Err("DBProxyClient is not connected to a storage server".into());
            };
            let (v_res, v_wait) = oneshot::channel();
            st.pending.insert(v_rid.clone(), v_res);
            let v_frame = js_stringify(&json!({ "mt": p_mt, "ms": p_ms, "rid": v_rid }));
            if v_tx.send(Message::Text(Utf8Bytes::from(v_frame))).is_err() {
                st.pending.remove(&v_rid);
                return Err("WebSocket is not open: readyState 3 (CLOSED)".into());
            }
            v_wait
        };
        match tokio::time::timeout(REQUEST_TIMEOUT, v_wait).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("DBProxyClient disconnected".into()),
            Err(_) => {
                self.m_state.lock().unwrap().pending.remove(&v_rid);
                Err("DBProxyClient request timed out".into())
            }
        }
    }

    /// LoadMission (9010) with the optional P1F-01 list/paging fields.
    pub async fn load_mission(
        &self,
        p_unit: Option<Value>,
        p_account: Value,
        p_mission: Option<Value>,
        p_opts: &Value,
    ) -> Result<Value, String> {
        let mut v_ms = Map::new();
        put_opt(&mut v_ms, "unitId", p_unit);
        v_ms.insert("accountId".into(), p_account);
        if crate::util::js_truthy(p_mission.as_ref()) {
            v_ms.insert("missionId".into(), p_mission.unwrap());
        }
        if crate::util::is_true(p_opts.get("list")) {
            v_ms.insert("list".into(), json!(true));
        }
        for k in ["kind", "before", "rq"] {
            if let Some(v) = p_opts.get(k).filter(|v| !v.is_null()) {
                v_ms.insert(k.into(), v.clone());
            }
        }
        self.send_request(CONST_TYPE_ANDRUAV_SYSTEM_LOAD_MISSION, Value::Object(v_ms)).await
    }

    /// SaveMission (9011)
    #[allow(clippy::too_many_arguments)]
    pub async fn save_mission(
        &self,
        p_unit: Option<Value>,
        p_account: Value,
        p_mission: Option<Value>,
        p_name: Option<Value>,
        p_data: Option<Value>,
        p_opts: &Value,
    ) -> Result<Value, String> {
        let mut v_ms = Map::new();
        put_opt(&mut v_ms, "unitId", p_unit);
        v_ms.insert("accountId".into(), p_account);
        put_opt(&mut v_ms, "missionId", p_mission);
        put_opt(&mut v_ms, "name", p_name);
        put_opt(&mut v_ms, "data", p_data);
        for k in ["kind", "meta", "rq"] {
            if let Some(v) = p_opts.get(k).filter(|v| !v.is_null()) {
                v_ms.insert(k.into(), v.clone());
            }
        }
        self.send_request(CONST_TYPE_ANDRUAV_SYSTEM_SAVE_MISSION, Value::Object(v_ms)).await
    }

    /// DeleteMission (9012)
    pub async fn delete_mission(&self, p_mission: Option<Value>, p_account: Value) -> Result<Value, String> {
        let mut v_ms = Map::new();
        put_opt(&mut v_ms, "missionId", p_mission);
        v_ms.insert("accountId".into(), p_account);
        self.send_request(CONST_TYPE_ANDRUAV_SYSTEM_DELETE_MISSION, Value::Object(v_ms)).await
    }

    /// LoadNews (9015): global + the account's news.
    pub async fn load_news(&self, p_account: Value) -> Result<Value, String> {
        self.send_request(CONST_TYPE_ANDRUAV_SYSTEM_LOAD_NEWS, json!({ "accountId": p_account })).await
    }

    /// SaveNews (9016)
    #[allow(clippy::too_many_arguments)]
    pub async fn save_news(
        &self,
        p_scope: &str,
        p_account: Value,
        p_title: Option<Value>,
        p_body: Option<Value>,
        p_priority: Option<Value>,
        p_author: Value,
        p_expires_at: Option<Value>,
        p_news_id: Option<Value>,
    ) -> Result<Value, String> {
        let mut v_ms = Map::new();
        v_ms.insert("scope".into(), json!(p_scope));
        v_ms.insert("accountId".into(), p_account);
        put_opt(&mut v_ms, "title", p_title);
        put_opt(&mut v_ms, "body", p_body);
        put_opt(&mut v_ms, "priority", p_priority);
        v_ms.insert("authorId".into(), p_author);
        // `expiresAt = null` default: undefined becomes null
        v_ms.insert("expiresAt".into(), p_expires_at.unwrap_or(Value::Null));
        if crate::util::js_truthy(p_news_id.as_ref()) {
            v_ms.insert("newsId".into(), p_news_id.unwrap());
        }
        self.send_request(CONST_TYPE_ANDRUAV_SYSTEM_SAVE_NEWS, Value::Object(v_ms)).await
    }

    /// DeleteNews (9017)
    pub async fn delete_news(&self, p_news_id: Option<Value>) -> Result<Value, String> {
        let mut v_ms = Map::new();
        put_opt(&mut v_ms, "newsId", p_news_id);
        self.send_request(CONST_TYPE_ANDRUAV_SYSTEM_DELETE_NEWS, Value::Object(v_ms)).await
    }
}

/// `obj.k = v` where v may be undefined (JSON.stringify omits it).
fn put_opt(p_map: &mut Map<String, Value>, p_key: &str, p_value: Option<Value>) {
    if let Some(v) = p_value {
        p_map.insert(p_key.into(), v);
    }
}
