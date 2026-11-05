//! UdpRelayClient (js_udp_relay_client.js): with `udp_relay` configured the
//! comm server brokers 9008 requests to standalone UDP relay servers
//! (droneengage_udp_relay_server, wiki/UdpRelayServer.md) instead of binding
//! UDP sockets itself, and replies to the unit with the relay's sockets in
//! today's 9008 shape.
//!
//! Config (server.config):
//!   udp_relay            : [{host, port, id, ssl?}]  absent or [] -> no UDP proxy (9008 gets en:false)
//!   udp_relay_select     : "hash" (default) | "first_healthy" | "least_loaded"
//!   udp_relay_pending_s  : hold an open request this long while relays reconnect (default 30)
//!   udp_relay_failover_s : a relay down this long has its units moved (default 120)
//!
//! Every relay gets its own S2S link (Ed25519 or HMAC per the challenge
//! `method`, fail closed). A unit's relay is chosen when it first asks and
//! remembered; it stays there while that relay is healthy, and nothing moves
//! back after a failover. Units that want a proxy are remembered, so every
//! (re)auth sends `sync`, pairs a relay lost are re-opened and pushed to the
//! unit as an unsolicited 9008, and a request made while no relay was up is
//! opened when one comes back. GCS-side pinning: `open` carries the group's
//! GCS IPs, a joining GCS is `peer_allow`ed, and GCSes use 9021.

use futures_util::StreamExt;
use indexmap::IndexMap;
use rand::Rng;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};

use crate::chat::Ctx;
use crate::config::Config;
use crate::constants::*;
use crate::hub::Login;
use crate::net::{cfg_str, frame_bytes, split_with_writer, ws_connect, TlsVerify};
use crate::s2s::s2s_auth::{parse_envelope, S2S_AUTH_CHALLENGE, S2S_AUTH_HANDSHAKE_TIMEOUT_MS};
use crate::server::Server;
use crate::util::{
    field, js_key, js_number, js_parse_bytes, js_strict_eq, js_stringify, js_to_string, js_truthy, now_ms, uuid_v4,
};

const REQUEST_TIMEOUT: Duration = Duration::from_millis(10000);
const HEARTBEAT_INTERVAL_MS: u64 = 30000;
const MAX_RECONNECT_DELAY_MS: f64 = 60000.0;
const INITIAL_RECONNECT_DELAY_MS: f64 = 1000.0;
const DEFAULT_PENDING_S: f64 = 30.0;
const DEFAULT_FAILOVER_S: f64 = 120.0;

pub const SELECT_HASH: &str = "hash";
pub const SELECT_FIRST_HEALTHY: &str = "first_healthy";
pub const SELECT_LEAST_LOADED: &str = "least_loaded";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitState {
    /// has (or had) a pair on its relay
    Relay,
    /// waiting for a relay to come up
    Pending,
}

impl UnitState {
    fn name(self) -> &'static str {
        match self {
            UnitState::Relay => "relay",
            UnitState::Pending => "pending",
        }
    }
}

fn log(p_text: &str) {
    println!("{}[UdpRelay] {}{}", crate::colors::FG_YELLOW, p_text, crate::colors::RESET);
}

/// `String(account) + '\0' + String(unit)`
pub fn unit_key(p_account: Option<&Value>, p_unit: Option<&Value>) -> String {
    format!("{}\u{0}{}", js_key(p_account), js_key(p_unit))
}

/// fn_selectRelayId: the relay id for (account, unit) among p_ids (healthy
/// relays in config order). Pure, so the choice is the same on every comm
/// server and across restarts.
///   hash          rendezvous hash: highest sha256(id\naccount\nunit)
///   first_healthy the first id
///   least_loaded  the lowest p_loads[id] (unknown = 0), ties in config order
pub fn select_relay_id(
    p_account: &str,
    p_unit: &str,
    p_ids: &[String],
    p_mode: &str,
    p_loads: &HashMap<String, f64>,
) -> Option<String> {
    if p_ids.is_empty() {
        return None;
    }
    if p_mode == SELECT_FIRST_HEALTHY {
        return Some(p_ids[0].clone());
    }
    if p_mode == SELECT_LEAST_LOADED {
        let mut v_best = &p_ids[0];
        let mut v_best_load = f64::INFINITY;
        for v_id in p_ids {
            let v_load = p_loads.get(v_id).copied().filter(|l| !l.is_nan()).unwrap_or(0.0);
            if v_load < v_best_load {
                v_best = v_id;
                v_best_load = v_load;
            }
        }
        return Some(v_best.clone());
    }
    let mut v_best: Option<(&String, String)> = None;
    for v_id in p_ids {
        let v_score: String = Sha256::digest(format!("{}\n{}\n{}", v_id, p_account, p_unit).as_bytes())
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        if v_best.as_ref().map(|(_, s)| v_score > *s).unwrap_or(true) {
            v_best = Some((v_id, v_score));
        }
    }
    v_best.map(|(id, _)| id.clone())
}

// ----------------------------------------------------------------------
// one relay link
// ----------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub host: Value,
    pub port: Value,
    pub id: String,
    pub ssl: Option<Value>,
}

struct ConnState {
    tx: Option<UnboundedSender<Message>>,
    open: bool,
    authenticated: bool,
    state: &'static str,
    last_error: Option<String>,
    pending: HashMap<String, oneshot::Sender<Result<Value, String>>>,
    reconnect_attempts: u32,
    stopped: bool,
}

pub struct RelayConn {
    pub endpoint: Endpoint,
    m_st: Mutex<ConnState>,
}

impl RelayConn {
    fn new(p_endpoint: Endpoint) -> RelayConn {
        RelayConn {
            endpoint: p_endpoint,
            m_st: Mutex::new(ConnState {
                tx: None,
                open: false,
                authenticated: false,
                state: CONST_STORAGE_STATE_DISCONNECTED,
                last_error: None,
                pending: HashMap::new(),
                reconnect_attempts: 0,
                stopped: false,
            }),
        }
    }

    pub fn is_connected(&self) -> bool {
        let st = self.m_st.lock().unwrap();
        st.open && st.authenticated && st.tx.is_some()
    }

    pub fn state(&self) -> &'static str {
        self.m_st.lock().unwrap().state
    }

    /// fn_setState: the error is always recorded; a change is reported to AUTH.
    fn set_state(self: &Arc<Self>, p_server: &Arc<Server>, p_state: &'static str, p_error: Option<String>) {
        let v_changed = {
            let mut st = self.m_st.lock().unwrap();
            let v_changed = st.state != p_state;
            st.state = p_state;
            st.last_error = p_error;
            v_changed
        };
        if v_changed {
            p_server.relay.send_relay_status(p_server, self);
        }
    }

    /// fn_request: `{mt, ms, rid}`; resolves with the reply envelope.
    pub async fn request(&self, p_mt: &str, p_ms: Value) -> Result<Value, String> {
        let v_rid = uuid_v4();
        let v_wait = {
            let mut st = self.m_st.lock().unwrap();
            let Some(v_tx) = st.tx.clone().filter(|_| st.open && st.authenticated) else {
                return Err("relay not connected".into());
            };
            let (v_res, v_wait) = oneshot::channel();
            st.pending.insert(v_rid.clone(), v_res);
            let v_frame = js_stringify(&json!({ "mt": p_mt, "ms": p_ms, "rid": v_rid }));
            if v_tx.send(Message::Text(Utf8Bytes::from(v_frame))).is_err() {
                st.pending.remove(&v_rid);
                return Err("relay not connected".into());
            }
            v_wait
        };
        match tokio::time::timeout(REQUEST_TIMEOUT, v_wait).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err("relay connection closed".into()),
            Err(_) => {
                self.m_st.lock().unwrap().pending.remove(&v_rid);
                Err("relay request timed out".into())
            }
        }
    }

    fn start(self: &Arc<Self>, p_server: &Arc<Server>) {
        // fn_connect sets 'connecting' synchronously: the first status AUTH
        // gets (fn_setSendToAuthCallback) already says so
        self.m_st.lock().unwrap().state = CONST_STORAGE_STATE_CONNECTING;
        let v_me = self.clone();
        let v_server = p_server.clone();
        tokio::spawn(async move {
            loop {
                if v_me.m_st.lock().unwrap().stopped {
                    return;
                }
                v_me.run_once(&v_server).await;
                if v_me.m_st.lock().unwrap().stopped {
                    return;
                }
                // exponential backoff 1 s -> 60 s with +/-25 % jitter
                let v_delay = {
                    let mut st = v_me.m_st.lock().unwrap();
                    st.reconnect_attempts += 1;
                    let v_base = (INITIAL_RECONNECT_DELAY_MS * 2f64.powi(st.reconnect_attempts as i32 - 1))
                        .min(MAX_RECONNECT_DELAY_MS);
                    (v_base + v_base * 0.25 * (rand::thread_rng().gen::<f64>() * 2.0 - 1.0)).floor().max(0.0) as u64
                };
                tokio::time::sleep(Duration::from_millis(v_delay)).await;
            }
        });
    }

    async fn run_once(self: &Arc<Self>, p_server: &Arc<Server>) {
        self.set_state(p_server, CONST_STORAGE_STATE_CONNECTING, None);
        let v_tls = if self.endpoint.ssl.as_ref() == Some(&Value::Bool(false)) { None } else { Some(TlsVerify::None) };
        let v_ws =
            match ws_connect(&cfg_str(Some(&self.endpoint.host)), &cfg_str(Some(&self.endpoint.port)), v_tls.as_ref())
                .await
            {
                Ok(w) => w,
                Err(e) => {
                    self.set_state(p_server, CONST_STORAGE_STATE_ERROR, Some(e));
                    self.on_close(p_server);
                    return;
                }
            };
        let (v_tx, mut v_rx) = split_with_writer(v_ws);
        {
            let mut st = self.m_st.lock().unwrap();
            st.tx = Some(v_tx.clone());
            st.open = true;
            st.authenticated = false;
            st.reconnect_attempts = 0;
        }
        // fail closed: a relay that does not finish the handshake is dropped
        let v_auth_timer = {
            let v_me = self.clone();
            let v_tx = v_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(S2S_AUTH_HANDSHAKE_TIMEOUT_MS)).await;
                if !v_me.m_st.lock().unwrap().authenticated {
                    log(&format!(
                        "relay {}: S2S auth not completed within {}ms — closing (fail closed)",
                        v_me.endpoint.id, S2S_AUTH_HANDSHAKE_TIMEOUT_MS
                    ));
                    let _ = v_tx.send(Message::Close(None));
                }
            })
        };
        while let Some(v_msg) = v_rx.next().await {
            match v_msg {
                Ok(m) => {
                    if let Some(v_data) = frame_bytes(&m) {
                        self.on_message(p_server, &v_tx, v_data);
                    }
                }
                Err(e) => {
                    if !crate::net::is_abrupt_close(&e) {
                        self.set_state(p_server, CONST_STORAGE_STATE_ERROR, Some(e.to_string()));
                    }
                    break;
                }
            }
        }
        v_auth_timer.abort();
        self.on_close(p_server);
    }

    fn on_message(self: &Arc<Self>, p_server: &Arc<Server>, p_tx: &UnboundedSender<Message>, p_data: &[u8]) {
        if let Some(v_env) = parse_envelope(p_data) {
            if v_env.get("s2s_auth").and_then(Value::as_str) == Some(S2S_AUTH_CHALLENGE) {
                match p_server.s2s.answer_challenge(&v_env) {
                    Ok(r) => {
                        let _ = p_tx.send(Message::Text(Utf8Bytes::from(r)));
                    }
                    Err(e) => {
                        log(&format!("relay {}: S2S handshake failed: Error: {}", self.endpoint.id, e));
                        self.set_state(p_server, CONST_STORAGE_STATE_ERROR, Some(e));
                        let _ = p_tx.send(Message::Close(None));
                    }
                }
            }
            return;
        }
        let Some(v_msg) = js_parse_bytes(p_data).filter(|v| v.is_object()) else { return };
        if v_msg.get("type").and_then(Value::as_str) == Some("auth_success") {
            self.m_st.lock().unwrap().authenticated = true;
            log(&format!(
                "authenticated with relay {} ({}:{})",
                self.endpoint.id,
                cfg_str(Some(&self.endpoint.host)),
                cfg_str(Some(&self.endpoint.port))
            ));
            self.set_state(p_server, CONST_STORAGE_STATE_CONNECTED, None);
            let (v_server, v_me) = (p_server.clone(), self.clone());
            tokio::spawn(async move { on_relay_authenticated(&v_server, &v_me).await });
            return;
        }
        if v_msg.get("type").and_then(Value::as_str) == Some("error") {
            log(&format!("relay {} error: {}", self.endpoint.id, js_key(v_msg.get("error"))));
            return;
        }
        if let Some(v_rid) = v_msg.get("rid").filter(|r| !r.is_null()) {
            let v_pending = self.m_st.lock().unwrap().pending.remove(&js_to_string(v_rid));
            if let Some(p) = v_pending {
                let _ = p.send(Ok(v_msg));
                return;
            }
        }
        if v_msg.get("rid").map(Value::is_null).unwrap_or(true)
            && v_msg.get("mt").map(|m| !m.is_null()).unwrap_or(false)
        {
            on_relay_push(p_server, self, &v_msg);
        }
    }

    fn on_close(self: &Arc<Self>, p_server: &Arc<Server>) {
        let (v_was_authenticated, v_pending, v_set_disconnected, v_error) = {
            let mut st = self.m_st.lock().unwrap();
            let v_was = st.authenticated;
            st.authenticated = false;
            st.open = false;
            st.tx = None;
            let v_pending = std::mem::take(&mut st.pending);
            (v_was, v_pending, st.state != CONST_STORAGE_STATE_ERROR || st.stopped, st.last_error.clone())
        };
        for (_, p) in v_pending {
            let _ = p.send(Err("relay connection closed".into()));
        }
        if v_set_disconnected {
            self.set_state(p_server, CONST_STORAGE_STATE_DISCONNECTED, v_error);
        }
        if v_was_authenticated {
            on_relay_lost(p_server, self);
        }
    }
}

// ----------------------------------------------------------------------
// broker
// ----------------------------------------------------------------------

struct UnitEntry {
    /// object identity: a new request replaces the entry
    serial: u64,
    account: Option<Value>,
    group: Option<Value>,
    unit: Option<Value>,
    req: Value,
    cap: Option<Value>,
    state: UnitState,
    relay: Option<Arc<RelayConn>>,
    timer: Option<JoinHandle<()>>,
    waiting: Option<(Ctx, Value)>,
    opening: bool,
    /// unit token from the last open reply; the changed push carries it too
    tk: Option<Value>,
}

#[derive(Default)]
struct BrokerState {
    units: IndexMap<String, UnitEntry>,
    failover_timers: HashMap<String, JoinHandle<()>>,
    loads: HashMap<String, f64>,
}

pub struct UdpRelayClient {
    m_enabled: bool,
    m_relays: Mutex<Vec<Arc<RelayConn>>>,
    m_st: Mutex<BrokerState>,
    m_serial: AtomicU64,
}

fn same_relay(p_a: &Option<Arc<RelayConn>>, p_b: &Arc<RelayConn>) -> bool {
    p_a.as_ref().map(|a| Arc::ptr_eq(a, p_b)).unwrap_or(false)
}

impl UdpRelayClient {
    pub fn new(p_cfg: &Config) -> UdpRelayClient {
        let v_enabled = matches!(p_cfg.get("udp_relay"), Some(Value::Array(a)) if !a.is_empty());
        UdpRelayClient {
            m_enabled: v_enabled,
            m_relays: Mutex::new(Vec::new()),
            m_st: Mutex::new(BrokerState::default()),
            m_serial: AtomicU64::new(1),
        }
    }

    /// fn_isEnabled: udp_relay is a non-empty list.
    pub fn is_enabled(&self) -> bool {
        self.m_enabled
    }

    fn relays(&self) -> Vec<Arc<RelayConn>> {
        self.m_relays.lock().unwrap().clone()
    }

    fn healthy(&self) -> Vec<Arc<RelayConn>> {
        self.relays().into_iter().filter(|r| r.is_connected()).collect()
    }

    /// fn_isConnected: at least one relay is connected and authenticated.
    pub fn is_connected(&self) -> bool {
        self.relays().iter().any(|r| r.is_connected())
    }

    /// What is worth telling the operator about the UDP proxy settings at startup.
    pub fn config_warnings(p_cfg: &Config) -> Vec<&'static str> {
        let mut v_out = Vec::new();
        if matches!(p_cfg.get("udp_relay"), Some(Value::Array(a)) if !a.is_empty()) {
            // GL-0097: without a shared secret or an Ed25519 key the relay
            // link never authenticates and every drone silently gets
            // en:false — say so loudly at startup
            let v_env = std::env::var("de_s2s_shared_secret").map(|s| !s.trim().is_empty()).unwrap_or(false);
            let v_file = p_cfg
                .string("s2s_shared_secret_file")
                .map(|f| p_cfg.resolve(&f).exists())
                .unwrap_or(false);
            let v_key = p_cfg
                .string("s2s_my_private_key")
                .map(|f| p_cfg.resolve(&f).exists())
                .unwrap_or(false);
            if !v_env && !v_file && !v_key {
                v_out.push(
                    "UDP relay has no shared secret and no Ed25519 key: copy the relay's s2s_shared_secret.key here or set de_s2s_shared_secret — drones get en:false until then",
                );
            }
        } else {
            v_out.push("UDP proxy is off: udp_relay is empty, so drones get en:false");
        }
        if p_cfg.get("udp_relay_fallback").and_then(Value::as_str) == Some("builtin") {
            v_out.push("udp_relay_fallback is ignored: the built-in UDP proxy was removed");
        }
        v_out
    }

    fn seconds(p_server: &Server, p_key: &str, p_default: f64) -> Duration {
        let v = js_number(p_server.cfg.get(p_key));
        let v_s = if v.is_finite() && v >= 0.0 { v } else { p_default };
        Duration::from_millis((v_s * 1000.0) as u64)
    }

    fn select_mode(p_server: &Server) -> &'static str {
        match p_server.cfg.get("udp_relay_select").and_then(Value::as_str) {
            Some(SELECT_FIRST_HEALTHY) => SELECT_FIRST_HEALTHY,
            Some(SELECT_LEAST_LOADED) => SELECT_LEAST_LOADED,
            _ => SELECT_HASH,
        }
    }

    /// The relay a unit should go to now, or None when none is healthy.
    fn select_relay(
        &self,
        p_server: &Server,
        p_account: Option<&Value>,
        p_unit: Option<&Value>,
    ) -> Option<Arc<RelayConn>> {
        let v_healthy = self.healthy();
        let v_ids: Vec<String> = v_healthy.iter().map(|r| r.endpoint.id.clone()).collect();
        let v_loads = self.m_st.lock().unwrap().loads.clone();
        let v_id = select_relay_id(&js_key(p_account), &js_key(p_unit), &v_ids, Self::select_mode(p_server), &v_loads)?;
        v_healthy.into_iter().find(|r| r.endpoint.id == v_id)
    }

    /// fn_initialize: connects to every configured relay.
    pub fn start(p_server: &Arc<Server>) {
        let v_me = &p_server.relay;
        if !v_me.m_enabled || !v_me.m_relays.lock().unwrap().is_empty() {
            return;
        }
        let Some(Value::Array(v_list)) = p_server.cfg.get("udp_relay") else { return };
        let mut v_seen = HashSet::new();
        for v_entry in v_list {
            let (Some(v_host), Some(v_port)) =
                (field(v_entry, "host").filter(|v| !v.is_null()), field(v_entry, "port").filter(|v| !v.is_null()))
            else {
                continue;
            };
            let v_id = match field(v_entry, "id").filter(|v| js_truthy(Some(v))) {
                Some(id) => js_to_string(id),
                None => format!("{}:{}", js_to_string(v_host), js_to_string(v_port)),
            };
            if !v_seen.insert(v_id.clone()) {
                log(&format!("duplicate relay id {} ignored", v_id));
                continue;
            }
            log(&format!("relay {} at {}:{}", v_id, js_to_string(v_host), js_to_string(v_port)));
            v_me.m_relays.lock().unwrap().push(Arc::new(RelayConn::new(Endpoint {
                host: v_host.clone(),
                port: v_port.clone(),
                id: v_id,
                ssl: field(v_entry, "ssl").cloned(),
            })));
        }
        log(&format!(
            "{} relay(s), select {}, pending {}s, failover {}s",
            v_me.relays().len(),
            Self::select_mode(p_server),
            crate::util::js_number_fmt(
                Self::seconds(p_server, "udp_relay_pending_s", DEFAULT_PENDING_S).as_millis() as f64 / 1000.0
            ),
            crate::util::js_number_fmt(
                Self::seconds(p_server, "udp_relay_failover_s", DEFAULT_FAILOVER_S).as_millis() as f64 / 1000.0
            )
        ));
        for r in v_me.relays() {
            r.start(p_server);
        }
        let v_server = p_server.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(HEARTBEAT_INTERVAL_MS)).await;
                v_server.relay.heartbeat(&v_server);
            }
        });
        // fn_setSendToAuthCallback sends the current status right away
        v_me.send_status(p_server);
    }

    fn count_relay_units(&self, p_relay: &Arc<RelayConn>) -> usize {
        self.m_st
            .lock()
            .unwrap()
            .units
            .values()
            .filter(|e| e.state == UnitState::Relay && same_relay(&e.relay, p_relay))
            .count()
    }

    /// `j` UDP_RELAY_STATUS to AUTH for one relay.
    fn send_relay_status(&self, p_server: &Server, p_relay: &Arc<RelayConn>) {
        if !p_server.uses_auth() {
            return;
        }
        let (v_state, v_error) = {
            let st = p_relay.m_st.lock().unwrap();
            (st.state, st.last_error.clone())
        };
        let v_cmd = json!({
            "c": CONST_CS_CMD_UDP_RELAY_STATUS,
            "d": {
                "relay_id": p_relay.endpoint.id,
                "host": p_relay.endpoint.host,
                "port": p_relay.endpoint.port,
                "status": v_state,
                "pairs": self.count_relay_units(p_relay),
                "error": v_error,
                "timestamp": now_ms() / 1000,
            }
        });
        p_server.auth.send_message(js_stringify(&v_cmd));
    }

    fn send_status(&self, p_server: &Server) {
        for r in self.relays() {
            self.send_relay_status(p_server, &r);
        }
    }

    fn heartbeat(&self, p_server: &Arc<Server>) {
        self.send_status(p_server);
        if Self::select_mode(p_server) == SELECT_LEAST_LOADED {
            for r in self.healthy() {
                refresh_load(p_server, r);
            }
        }
    }

    /// Where each wanted unit is (tests, diagnostics).
    pub fn unit_placement(&self) -> Vec<Value> {
        self.m_st
            .lock()
            .unwrap()
            .units
            .values()
            .map(|e| {
                json!({
                    "account": e.account, "unit": e.unit, "state": e.state.name(),
                    "relay_id": e.relay.as_ref().map(|r| r.endpoint.id.clone()),
                })
            })
            .collect()
    }

    /// Each relay's link state (tests, diagnostics).
    pub fn relay_states(&self) -> Vec<Value> {
        self.relays()
            .iter()
            .map(|r| json!({ "id": r.endpoint.id, "status": r.state(), "connected": r.is_connected() }))
            .collect()
    }

    /// Pairs this comm server owns on every reachable relay (AUTH `e`).
    pub async fn relay_proxies_list(&self, p_server: &Arc<Server>) -> Vec<Value> {
        let v_lists = futures_util::future::join_all(self.healthy().into_iter().map(|r| list_relay(p_server, r))).await;
        v_lists.into_iter().flatten().map(|(v_row, _)| v_row).collect()
    }

    /// AUTH `g` on relay pairs: blocks/unblocks the pairs of unit p_name
    /// (optionally only in p_account). @returns true when one was applied.
    pub async fn set_relay_proxy_blocked(
        &self,
        p_server: &Arc<Server>,
        p_name: &str,
        p_blocked: bool,
        p_account: Option<&Value>,
    ) -> bool {
        let v_lists = futures_util::future::join_all(self.healthy().into_iter().map(|r| list_relay(p_server, r))).await;
        let mut v_applied = false;
        for (v_row, v_relay) in v_lists.into_iter().flatten() {
            if v_row.get("name").and_then(Value::as_str) != Some(p_name) {
                continue;
            }
            if let Some(a) = p_account.filter(|a| !a.is_null()) {
                if js_key(v_row.get("account")) != js_key(Some(a)) {
                    continue;
                }
            }
            let v_req = json!({ "account": v_row.get("account"), "unit": v_row.get("name"), "blocked": p_blocked });
            match v_relay.request("block", v_req).await {
                Ok(r) => {
                    if is_success(&r) && field(&r["ms"], "applied") == Some(&Value::Bool(true)) {
                        v_applied = true;
                    }
                }
                Err(e) => log(&format!("relay block for '{}' failed: {}", p_name, e)),
            }
        }
        v_applied
    }
}

fn is_success(p_rep: &Value) -> bool {
    p_rep.get("success") == Some(&Value::Bool(true)) && js_truthy(p_rep.get("ms"))
}

fn refresh_load(p_server: &Arc<Server>, p_relay: Arc<RelayConn>) {
    let v_server = p_server.clone();
    tokio::spawn(async move {
        if let Ok(r) = p_relay.request("stats", json!({})).await {
            if is_success(&r) {
                if let Some(Value::Number(n)) = r["ms"].get("pairs") {
                    if let Some(f) = n.as_f64().filter(|f| f.is_finite()) {
                        v_server.relay.m_st.lock().unwrap().loads.insert(p_relay.endpoint.id.clone(), f);
                    }
                }
            }
        }
    });
}

// ----------------------------------------------------------------------
// unit side
// ----------------------------------------------------------------------

fn zero_socket() -> Value {
    json!({ "address": "0.0.0.0", "port": 0 })
}

/// The 9008 payload for the unit from a relay open reply: {en, socket1,
/// socket2} plus extra relay fields (e.g. tk), without relay-internal ids.
pub fn unit_payload(p_relay_ms: &Value) -> Value {
    let mut v_ms: Map<String, Value> = p_relay_ms.as_object().cloned().unwrap_or_default();
    v_ms.shift_remove("relay_id");
    v_ms.shift_remove("pair_id");
    v_ms.shift_remove("error");
    let v_en = field(p_relay_ms, "en") == Some(&Value::Bool(true));
    v_ms.insert("en".into(), json!(v_en));
    if !v_en {
        v_ms.insert("socket1".into(), zero_socket());
        v_ms.insert("socket2".into(), zero_socket());
    }
    Value::Object(v_ms)
}

pub fn disabled_payload() -> Value {
    json!({ "en": false, "socket1": zero_socket(), "socket2": zero_socket() })
}

/// 9008 from a drone when no relay is configured.
pub fn reply_disabled(p_server: &Server, p_ctx: &Ctx, mut p_jmsg: Value) {
    reply_to_unit(p_server, p_ctx, &mut p_jmsg, disabled_payload());
}

/// A 9008 reply on the unit's socket, reusing the request object.
fn reply_to_unit(p_server: &Server, p_ctx: &Ctx, p_jmsg: &mut Value, p_ms: Value) {
    crate::util::set_field(p_jmsg, "ms", p_ms);
    crate::util::set_field(p_jmsg, "ty", json!(CONST_WS_MSG_ROUTING_INDIVIDUAL));
    crate::util::set_field_opt(p_jmsg, "tg", p_ctx.name_now(p_server));
    crate::util::set_field(p_jmsg, "sd", json!(CONST_WS_SENDER_COMM_SERVER));
    p_ctx.handle.send_json(p_jmsg);
}

/// An unsolicited 9008 to a connected unit; skipped when it is gone or now
/// belongs to another account.
fn push_to_unit(p_server: &Server, p_account: Option<&Value>, p_unit: Option<&Value>, p_ms: Value) -> bool {
    let v_hub = p_server.hub();
    let Some(v_conn) = v_hub.active_sender(p_unit) else { return false };
    let v_client = &v_hub.clients[&v_conn];
    if !v_client.handle.is_open() {
        return false;
    }
    let Some(v_login) = &v_client.login else { return false };
    if v_login.account_key() != js_key(p_account) {
        return false;
    }
    let mut v_msg = Map::new();
    v_msg.insert(CONST_WS_MESSAGE_ID.into(), json!(CONST_TYPE_ANDRUAV_SYSTEM_UDP_PROXY));
    v_msg.insert(CONST_WS_MSG_ROUTING.into(), json!(CONST_WS_MSG_ROUTING_INDIVIDUAL));
    if let Some(u) = p_unit {
        v_msg.insert(CONST_WS_TARGET_ID.into(), u.clone());
    }
    v_msg.insert(CONST_WS_SENDER_ID.into(), json!(CONST_WS_SENDER_COMM_SERVER));
    v_msg.insert(CONST_WS_PAYLOAD.into(), p_ms);
    v_client.handle.send_json(&Value::Object(v_msg));
    true
}

/// An unsolicited 9021 to the same-account+group GCS connections holding
/// full control (the 9021 permission rule). @returns recipients.
fn push_to_gcs(p_server: &Server, p_account: Option<&Value>, p_group: Option<&Value>, p_ms: Value) -> usize {
    let v_hub = p_server.hub();
    let v_full = CONST_PERMISSION_ALLOW_GCS_FULL_CONTROL;
    let v_account_key = js_key(p_account);
    let v_group_key = js_key(p_group);
    let mut v_n = 0;
    for v_c in v_hub.clients.values() {
        let Some(v_login) = &v_c.login else { continue };
        if !v_login.actor_is(CONST_ACTOR_TYPE_GCS)
            || v_login.account_key() != v_account_key
            || js_key(v_login.group_id.as_ref()) != v_group_key
            || js_and(js_number(v_login.prm.as_ref()), v_full) != v_full
            || !v_c.handle.is_open()
        {
            continue;
        }
        let mut v_msg = Map::new();
        v_msg.insert(CONST_WS_MESSAGE_ID.into(), json!(CONST_TYPE_ANDRUAV_SYSTEM_UDP_PROXY_PEER));
        v_msg.insert(CONST_WS_MSG_ROUTING.into(), json!(CONST_WS_MSG_ROUTING_INDIVIDUAL));
        v_msg.insert(CONST_WS_SENDER_ID.into(), json!(CONST_WS_SENDER_COMM_SERVER));
        v_msg.insert(CONST_WS_PAYLOAD.into(), p_ms.clone());
        v_c.handle.send_json(&Value::Object(v_msg));
        v_n += 1;
    }
    // DE-0064: a push nobody gets was silent - say who was skipped
    if v_n == 0 {
        log(&format!(
            "9021 push '{}' reached no GCS (account '{}', group '{}', {} client(s) connected)",
            js_key(p_ms.get("act")),
            v_account_key,
            v_group_key,
            v_hub.clients.len()
        ));
    }
    v_n
}

/// IPs of the GCS sockets in the same account+group as the unit's socket.
fn gcs_ips_of_group(p_server: &Server, p_account: Option<&Value>, p_unit: Option<&Value>) -> Vec<String> {
    let v_hub = p_server.hub();
    let Some(v_conn) = v_hub.active_sender(p_unit) else { return Vec::new() };
    let v_client = &v_hub.clients[&v_conn];
    match &v_client.login {
        Some(l) if l.account_key() == js_key(p_account) => {}
        _ => return Vec::new(),
    }
    let Some(v_group) = v_client.group.as_ref().and_then(|g| v_hub.group(g)) else { return Vec::new() };
    let mut v_ips: Vec<String> = Vec::new();
    for v_member in v_group.units.values() {
        let Some(c) = v_hub.clients.get(v_member) else { continue };
        let v_gcs = c.login.as_ref().map(|l| l.actor_is(CONST_ACTOR_TYPE_GCS)).unwrap_or(false);
        if let (true, Some(ip)) = (v_gcs, c.remote_ip.as_ref().filter(|ip| !ip.is_empty())) {
            if !v_ips.contains(ip) {
                v_ips.push(ip.clone());
            }
        }
    }
    v_ips
}

fn open_params(p_server: &Server, p_entry: &UnitEntry) -> Value {
    let mut v_ms = Map::new();
    for (k, v) in [("account", &p_entry.account), ("group", &p_entry.group), ("unit", &p_entry.unit)] {
        if let Some(v) = v {
            v_ms.insert(k.into(), v.clone());
        }
    }
    v_ms.insert("req".into(), p_entry.req.clone());
    if let Some(cap) = &p_entry.cap {
        v_ms.insert("cap".into(), cap.clone());
    }
    let v_ips = gcs_ips_of_group(p_server, p_entry.account.as_ref(), p_entry.unit.as_ref());
    if !v_ips.is_empty() {
        v_ms.insert("gcs_ips".into(), json!(v_ips));
    }
    Value::Object(v_ms)
}

/// Runs p_fn on the entry under p_key if it is still entry p_serial.
fn with_entry<R>(p_server: &Server, p_key: &str, p_serial: u64, p_fn: impl FnOnce(&mut UnitEntry) -> R) -> Option<R> {
    let mut st = p_server.relay.m_st.lock().unwrap();
    st.units.get_mut(p_key).filter(|e| e.serial == p_serial).map(p_fn)
}

fn is_current(p_server: &Server, p_key: &str, p_serial: u64) -> bool {
    with_entry(p_server, p_key, p_serial, |_| ()).is_some()
}

/// fn_openOnRelay: opens the entry's pair on p_relay, which then holds the
/// unit. Err when the request failed (link dropped, timeout).
async fn open_on_relay(
    p_server: &Arc<Server>,
    p_key: &str,
    p_serial: u64,
    p_relay: &Arc<RelayConn>,
) -> Result<Value, String> {
    let Some(v_params) = ({
        let st = p_server.relay.m_st.lock().unwrap();
        st.units
            .get(p_key)
            .filter(|e| e.serial == p_serial)
            .map(|e| (e.unit.clone(), e.account.clone(), e.group.clone(), e.req.clone(), e.cap.clone()))
    }) else {
        return Err("unit gone".into());
    };
    let (v_unit, ..) = v_params.clone();
    let v_entry_view = UnitEntry {
        serial: p_serial,
        account: v_params.1,
        group: v_params.2,
        unit: v_params.0,
        req: v_params.3,
        cap: v_params.4,
        state: UnitState::Pending,
        relay: None,
        timer: None,
        waiting: None,
        opening: false,
        tk: None,
    };
    let v_open = open_params(p_server, &v_entry_view);
    with_entry(p_server, p_key, p_serial, |e| e.opening = true);
    let v_rep = p_relay.request("open", v_open).await;
    with_entry(p_server, p_key, p_serial, |e| e.opening = false);
    let v_rep = v_rep?;
    if !(is_success(&v_rep) && field(&v_rep["ms"], "en") == Some(&Value::Bool(true))) {
        log(&format!(
            "relay {} refused open for '{}': {}",
            p_relay.endpoint.id,
            js_key(v_unit.as_ref()),
            v_rep.get("error").filter(|e| js_truthy(Some(e))).map(js_to_string).unwrap_or_else(|| "en:false".into())
        ));
        return Ok(disabled_payload());
    }
    with_entry(p_server, p_key, p_serial, |e| {
        e.state = UnitState::Relay;
        e.relay = Some(p_relay.clone());
        e.tk = field(&v_rep["ms"], "tk").filter(|t| js_truthy(Some(t))).cloned();
    });
    p_server.relay.send_relay_status(p_server, p_relay);
    Ok(unit_payload(&v_rep["ms"]))
}

/// fn_handleUnitRequest: 9008 from a drone when udp_relay is configured.
pub async fn handle_unit_request(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    let v_account = p_ctx.account_id();
    let v_group = p_ctx.login.as_ref().and_then(|l| l.group_id.clone());
    let v_unit = p_ctx.name_now(p_server);
    let v_key = unit_key(v_account.as_ref(), v_unit.as_ref());
    let v_req = p_jmsg.get("ms").filter(|m| js_truthy(Some(m))).cloned().unwrap_or_else(|| json!({}));
    let v_me = &p_server.relay;

    if field(&v_req, "en") != Some(&Value::Bool(true)) {
        // en:false -> close, wherever the unit is served
        let v_old = {
            let mut st = v_me.m_st.lock().unwrap();
            st.units.shift_remove(&v_key)
        };
        if let Some(t) = v_old.as_ref().and_then(|o| o.timer.as_ref()) {
            t.abort();
        }
        if let Some(v_relay) = v_old.and_then(|o| o.relay).filter(|r| r.is_connected()) {
            let v_close = json!({ "account": v_account, "unit": v_unit });
            if let Err(e) = v_relay.request("close", v_close).await {
                log(&format!("relay close for '{}' failed: {}", js_key(v_unit.as_ref()), e));
            }
            v_me.send_relay_status(p_server, &v_relay);
        }
        reply_to_unit(p_server, &p_ctx, &mut p_jmsg, disabled_payload());
        return;
    }

    let v_old_relay = {
        let st = v_me.m_st.lock().unwrap();
        st.units.get(&v_key).map(|o| {
            if let Some(t) = &o.timer {
                t.abort();
            }
            o.relay.clone()
        })
    };
    let v_relay = match v_old_relay.clone().flatten().filter(|r| r.is_connected()) {
        Some(r) => Some(r),
        None => v_me.select_relay(p_server, v_account.as_ref(), v_unit.as_ref()),
    };
    let fn_port = |s: &str| {
        let v = field(&v_req, s)
            .filter(|v| js_truthy(Some(v)))
            .and_then(|v| field(v, "port"))
            .filter(|v| js_truthy(Some(v)));
        v.cloned().unwrap_or(json!(0))
    };
    let v_serial = v_me.m_serial.fetch_add(1, Ordering::Relaxed);
    let v_entry = UnitEntry {
        serial: v_serial,
        account: v_account.clone(),
        group: v_group,
        unit: v_unit.clone(),
        req: json!({ "p1": fn_port("socket1"), "p2": fn_port("socket2") }),
        cap: field(&v_req, "cap").cloned(),
        state: UnitState::Pending,
        relay: v_old_relay.flatten(),
        timer: None,
        waiting: None,
        opening: false,
        tk: None,
    };
    v_me.m_st.lock().unwrap().units.insert(v_key.clone(), v_entry);

    let Some(v_relay) = v_relay else {
        hold_request(p_server, &v_key, v_serial, p_ctx, p_jmsg);
        return;
    };
    match open_on_relay(p_server, &v_key, v_serial, &v_relay).await {
        Ok(v_ms) => {
            if v_ms.get("en") != Some(&Value::Bool(true)) {
                let mut st = v_me.m_st.lock().unwrap();
                if st.units.get(&v_key).map(|e| e.serial == v_serial).unwrap_or(false) {
                    st.units.shift_remove(&v_key);
                }
            }
            reply_to_unit(p_server, &p_ctx, &mut p_jmsg, v_ms);
        }
        Err(e) => {
            // link dropped mid-request: hold it like a request made while down
            log(&format!("relay open for '{}' failed: {}", js_key(v_unit.as_ref()), e));
            hold_request(p_server, &v_key, v_serial, p_ctx, p_jmsg);
        }
    }
}

/// fn_holdRequest: no relay reachable — hold the request for
/// udp_relay_pending_s; answered when a relay authenticates in time, else
/// en:false now (en:true pushed later).
fn hold_request(p_server: &Arc<Server>, p_key: &str, p_serial: u64, p_ctx: Ctx, p_jmsg: Value) {
    let v_server = p_server.clone();
    let v_key = p_key.to_string();
    let v_wait = UdpRelayClient::seconds(p_server, "udp_relay_pending_s", DEFAULT_PENDING_S);
    let v_timer = tokio::spawn(async move {
        tokio::time::sleep(v_wait).await;
        let v_waiting = with_entry(&v_server, &v_key, p_serial, |e| {
            e.timer = None;
            e.waiting.take()
        })
        .flatten();
        let Some((v_ctx, mut v_jmsg)) = v_waiting else { return };
        let v_unit = js_key(v_ctx.name.as_ref());
        log(&format!(
            "no relay available for '{}', replying en:false (will push en:true when a relay is back)",
            v_unit
        ));
        reply_to_unit(&v_server, &v_ctx, &mut v_jmsg, disabled_payload());
    });
    let v_stale = with_entry(p_server, p_key, p_serial, |e| {
        e.state = UnitState::Pending;
        e.waiting = Some((p_ctx, p_jmsg));
        e.timer.replace(v_timer)
    });
    if let Some(Some(t)) = v_stale {
        t.abort();
    }
}

/// fn_onUnitDisconnected: the relay is told the unit is offline (its pair
/// idles out there) and the unit is forgotten.
pub fn on_unit_disconnected(p_server: &Arc<Server>, p_login: Option<&Login>, p_name: Option<&Value>) {
    let Some(v_login) = p_login else { return };
    let v_key = unit_key(v_login.account_id.as_ref(), p_name);
    let Some(v_entry) = p_server.relay.m_st.lock().unwrap().units.shift_remove(&v_key) else { return };
    if let Some(t) = &v_entry.timer {
        t.abort();
    }
    if v_entry.state != UnitState::Relay {
        return;
    }
    let Some(v_relay) = v_entry.relay.clone().filter(|r| r.is_connected()) else { return };
    let v_req = json!({ "account": v_login.account_id, "unit": p_name });
    let v_unit = js_key(p_name);
    let v_r = v_relay.clone();
    tokio::spawn(async move {
        if let Err(e) = v_r.request("unit_offline", v_req).await {
            log(&format!("relay unit_offline for '{}' failed: {}", v_unit, e));
        }
    });
    p_server.relay.send_relay_status(p_server, &v_relay);
}

/// fn_reopenElsewhere: (re)opens an entry on a relay (failover, recovery)
/// and tells the unit — the held request is answered, else an unsolicited
/// 9008. Without a healthy relay the unit waits as pending.
async fn reopen_elsewhere(p_server: &Arc<Server>, p_key: &str, p_serial: u64, p_relay: Option<Arc<RelayConn>>) {
    let v_ids = {
        let st = p_server.relay.m_st.lock().unwrap();
        st.units.get(p_key).filter(|e| e.serial == p_serial).map(|e| (e.account.clone(), e.unit.clone()))
    };
    let Some((v_account, v_unit)) = v_ids else { return };
    let v_relay = p_relay.or_else(|| p_server.relay.select_relay(p_server, v_account.as_ref(), v_unit.as_ref()));
    let Some(v_relay) = v_relay else {
        with_entry(p_server, p_key, p_serial, |e| {
            e.state = UnitState::Pending;
            e.relay = None;
        });
        return;
    };
    let v_waiting = with_entry(p_server, p_key, p_serial, |e| {
        if let Some(t) = e.timer.take() {
            t.abort();
        }
        e.waiting.take()
    })
    .flatten();
    let v_ms = match open_on_relay(p_server, p_key, p_serial, &v_relay).await {
        Ok(m) => m,
        Err(e) => {
            log(&format!("relay re-open for '{}' on {} failed: {}", js_key(v_unit.as_ref()), v_relay.endpoint.id, e));
            with_entry(p_server, p_key, p_serial, |e| {
                e.state = UnitState::Pending;
                e.relay = None;
            });
            return;
        }
    };
    if !is_current(p_server, p_key, p_serial) {
        return;
    }
    let v_en = v_ms.get("en") == Some(&Value::Bool(true));
    if !v_en {
        p_server.relay.m_st.lock().unwrap().units.shift_remove(p_key);
    }
    match v_waiting {
        Some((v_ctx, mut v_jmsg)) => reply_to_unit(p_server, &v_ctx, &mut v_jmsg, v_ms),
        None if v_en => {
            push_to_unit(p_server, v_account.as_ref(), v_unit.as_ref(), v_ms);
        }
        None => {}
    }
}

/// fn_onRelayAuthenticated: on every (re)auth, sync the units it holds,
/// re-open the ones it no longer has, and place every pending unit.
async fn on_relay_authenticated(p_server: &Arc<Server>, p_relay: &Arc<RelayConn>) {
    let v_me = &p_server.relay;
    if let Some(t) = v_me.m_st.lock().unwrap().failover_timers.remove(&p_relay.endpoint.id) {
        t.abort();
    }
    if UdpRelayClient::select_mode(p_server) == SELECT_LEAST_LOADED {
        refresh_load(p_server, p_relay.clone());
    }
    // only units known now; a request arriving meanwhile opens its own pair
    let (v_candidates, v_relay_units) = {
        let st = v_me.m_st.lock().unwrap();
        let mut v_candidates = Vec::new();
        let mut v_relay_units = Vec::new();
        for (k, e) in &st.units {
            if e.state == UnitState::Relay && same_relay(&e.relay, p_relay) {
                v_candidates.push((k.clone(), e.serial, true));
                let mut v_u = Map::new();
                if let Some(a) = &e.account {
                    v_u.insert("account".into(), a.clone());
                }
                if let Some(u) = &e.unit {
                    v_u.insert("unit".into(), u.clone());
                }
                v_relay_units.push(Value::Object(v_u));
            } else if e.state == UnitState::Pending {
                v_candidates.push((k.clone(), e.serial, false));
            }
        }
        (v_candidates, v_relay_units)
    };
    let v_kept: HashSet<String> = match p_relay.request("sync", json!({ "units": v_relay_units })).await {
        Ok(r) => match r.get("ms").filter(|_| is_success(&r)).and_then(|m| m.get("kept")) {
            Some(Value::Array(k)) => k.iter().map(|u| unit_key(field(u, "account"), field(u, "unit"))).collect(),
            _ => HashSet::new(),
        },
        Err(e) => {
            log(&format!("relay {} sync failed: {}", p_relay.endpoint.id, e));
            return;
        }
    };
    for (v_key, v_serial, v_was_relay) in v_candidates {
        let Some((v_state, v_on_this, v_opening)) =
            with_entry(p_server, &v_key, v_serial, |e| (e.state, same_relay(&e.relay, p_relay), e.opening))
        else {
            continue;
        };
        if v_opening {
            continue;
        }
        if v_was_relay {
            // still there, or moved/re-requested meanwhile
            if v_state != UnitState::Relay || !v_on_this || v_kept.contains(&v_key) {
                continue;
            }
            if !p_relay.is_connected() {
                return;
            }
            reopen_elsewhere(p_server, &v_key, v_serial, Some(p_relay.clone())).await;
        } else {
            if v_state != UnitState::Pending {
                continue;
            }
            reopen_elsewhere(p_server, &v_key, v_serial, None).await;
        }
    }
    v_me.send_relay_status(p_server, p_relay);
}

/// fn_onRelayLost: still down after udp_relay_failover_s -> its units move.
fn on_relay_lost(p_server: &Arc<Server>, p_relay: &Arc<RelayConn>) {
    let v_server = p_server.clone();
    let v_relay = p_relay.clone();
    let v_wait = UdpRelayClient::seconds(p_server, "udp_relay_failover_s", DEFAULT_FAILOVER_S);
    let v_timer = tokio::spawn(async move {
        tokio::time::sleep(v_wait).await;
        v_server.relay.m_st.lock().unwrap().failover_timers.remove(&v_relay.endpoint.id);
        if v_relay.is_connected() {
            return;
        }
        failover(&v_server, &v_relay).await;
    });
    if let Some(old) = p_server.relay.m_st.lock().unwrap().failover_timers.insert(p_relay.endpoint.id.clone(), v_timer)
    {
        old.abort();
    }
}

async fn failover(p_server: &Arc<Server>, p_relay: &Arc<RelayConn>) {
    let v_moving: Vec<(String, u64)> = p_server
        .relay
        .m_st
        .lock()
        .unwrap()
        .units
        .iter()
        .filter(|(_, e)| e.state == UnitState::Relay && same_relay(&e.relay, p_relay))
        .map(|(k, e)| (k.clone(), e.serial))
        .collect();
    if v_moving.is_empty() {
        return;
    }
    log(&format!("relay {} down past failover: moving {} unit(s)", p_relay.endpoint.id, v_moving.len()));
    for (v_key, v_serial) in v_moving {
        let v_ok =
            with_entry(p_server, &v_key, v_serial, |e| same_relay(&e.relay, p_relay) && !e.opening).unwrap_or(false);
        if !v_ok {
            continue;
        }
        reopen_elsewhere(p_server, &v_key, v_serial, None).await;
    }
    p_server.relay.send_status(p_server);
}

/// fn_onRelayPush: pushes for pairs this comm server owns on that relay:
///   closed owner_lost -> the relay gave up on us: move the unit
///   closed otherwise  -> 9008 en:false to the unit
///   changed           -> unsolicited 9008 en:true with the new sockets
/// The open payload the 'changed' push forwards to the unit: en:true,
/// the new sockets, and the unit token tk — the push's tk wins, else the
/// one stored from the last open reply (DE-0071/DE-0072).
fn changed_open_payload(p_ms: &Value, p_stored_tk: &Option<Value>) -> Value {
    let mut v_open = Map::new();
    v_open.insert("en".into(), json!(true));
    for s in ["socket1", "socket2"] {
        if let Some(v) = field(p_ms, s) {
            v_open.insert(s.into(), v.clone());
        }
    }
    if let Some(t) = field(p_ms, "tk").filter(|t| js_truthy(Some(t))).cloned().or_else(|| p_stored_tk.clone()) {
        v_open.insert("tk".into(), t);
    }
    Value::Object(v_open)
}

fn on_relay_push(p_server: &Arc<Server>, p_relay: &Arc<RelayConn>, p_msg: &Value) {
    let v_ms = p_msg.get("ms").filter(|m| js_truthy(Some(m))).cloned().unwrap_or_else(|| json!({}));
    if !js_strict_eq(field(&v_ms, "owner"), p_server.cfg.get("server_id")) {
        return;
    }
    let v_account = field(&v_ms, "account").cloned();
    let v_unit = field(&v_ms, "unit").cloned();
    let v_key = unit_key(v_account.as_ref(), v_unit.as_ref());
    let v_entry = {
        let st = p_server.relay.m_st.lock().unwrap();
        st.units.get(&v_key).filter(|e| same_relay(&e.relay, p_relay)).map(|e| {
            (e.serial, e.state, e.opening, e.group.clone(), e.tk.clone())
        })
    };
    // a unit that moved to another relay ignores its old relay's pushes
    let Some((v_serial, v_state, v_opening, v_group, v_tk)) = v_entry else { return };
    match p_msg.get("mt").and_then(Value::as_str) {
        Some("closed") => {
            // only pairs a unit still wants
            if v_state != UnitState::Relay || v_opening {
                return;
            }
            if field(&v_ms, "reason").and_then(Value::as_str) == Some("owner_lost") {
                let (v_server, v_key) = (p_server.clone(), v_key.clone());
                tokio::spawn(async move { reopen_elsewhere(&v_server, &v_key, v_serial, None).await });
                return;
            }
            p_server.relay.m_st.lock().unwrap().units.shift_remove(&v_key);
            if push_to_unit(p_server, v_account.as_ref(), v_unit.as_ref(), disabled_payload()) {
                log(&format!(
                    "notified unit '{}' that its relay pair was closed ({})",
                    js_key(v_unit.as_ref()),
                    js_key(field(&v_ms, "reason"))
                ));
            }
            p_server.relay.send_relay_status(p_server, p_relay);
        }
        Some("changed") => {
            let v_open = changed_open_payload(&v_ms, &v_tk);
            // GL-0110: the group's webclients learn the new socket2 port
            // after peer_change_port too
            let mut v_out = Map::new();
            if let Some(u) = v_unit.as_ref() {
                v_out.insert("tg_unit".into(), u.clone());
            }
            v_out.insert("act".into(), json!("changed"));
            for s in ["socket1", "socket2"] {
                v_out.insert(s.into(), v_open.get(s).cloned().unwrap_or(Value::Null));
            }
            push_to_unit(p_server, v_account.as_ref(), v_unit.as_ref(), unit_payload(&v_open));
            push_to_gcs(p_server, v_account.as_ref(), v_group.as_ref(), Value::Object(v_out));
        }
        // GL-0109: a stranger knocking, or the lock moving — the group's
        // full-control GCSs learn of it as a 9021 system message
        Some(v_mt @ ("gcs_attempt" | "gcs_switched")) => {
            let mut v_out = Map::new();
            if let Some(u) = v_unit.as_ref() {
                v_out.insert("tg_unit".into(), u.clone());
            }
            v_out.insert("act".into(), json!(if v_mt == "gcs_attempt" { "attempt" } else { "switched" }));
            for k in ["from_ip", "from_port", "others", "until_ms", "reason"] {
                if let Some(v) = v_ms.get(k) {
                    v_out.insert(k.into(), v.clone());
                }
            }
            push_to_gcs(p_server, v_account.as_ref(), v_group.as_ref(), Value::Object(v_out));
        }
        _ => {}
    }
}

// ----------------------------------------------------------------------
// GCS-side pinning
// ----------------------------------------------------------------------

/// fn_onGcsJoined: a GCS joined a group — allow its IP on the relay pairs of
/// that group's units.
pub fn on_gcs_joined(p_server: &Arc<Server>, p_conn: u64) {
    let (v_ip, v_login) = {
        let v_hub = p_server.hub();
        let Some(c) = v_hub.client(p_conn) else { return };
        let (Some(ip), Some(l)) = (c.remote_ip.clone().filter(|ip| !ip.is_empty()), c.login.clone()) else { return };
        (ip, l)
    };
    let v_targets: Vec<(Arc<RelayConn>, Option<Value>, Option<Value>)> = p_server
        .relay
        .m_st
        .lock()
        .unwrap()
        .units
        .values()
        .filter(|e| e.state == UnitState::Relay && e.relay.as_ref().map(|r| r.is_connected()).unwrap_or(false))
        .filter(|e| {
            js_key(e.account.as_ref()) == v_login.account_key()
                && js_key(e.group.as_ref()) == js_key(v_login.group_id.as_ref())
        })
        .map(|e| (e.relay.clone().unwrap(), e.account.clone(), e.unit.clone()))
        .collect();
    for (v_relay, v_account, v_unit) in v_targets {
        let v_req = json!({ "account": v_account, "unit": v_unit, "ips": [v_ip] });
        tokio::spawn(async move {
            if let Err(e) = v_relay.request("peer_allow", v_req).await {
                log(&format!("relay peer_allow for '{}' failed: {}", js_key(v_unit.as_ref()), e));
            }
        });
    }
}

/// The relay's pinned state of one pair, or null.
async fn relay_pinned(p_relay: &Arc<RelayConn>, p_account: Option<&Value>, p_unit: &str) -> Result<Value, String> {
    let v_rep = p_relay.request("list", json!({ "account": p_account })).await?;
    let Some(Value::Array(v_pairs)) = v_rep.get("ms").filter(|_| is_success(&v_rep)).and_then(|m| m.get("pairs"))
    else {
        return Ok(Value::Null);
    };
    Ok(v_pairs
        .iter()
        .find(|r| js_key(r.get("unit")) == p_unit)
        .and_then(|r| r.get("pinned").filter(|p| !p.is_null()).cloned())
        .unwrap_or(Value::Null))
}

/// Cap on the secs a GCS may pass to reject/kick (GL-0110); the relay
/// applies its own gcs_reject_max_s on top.
const GCS_REJECT_CAP_S: f64 = 3600.0;

/// ToInt32(x) & mask, as JavaScript's `&` does.
fn js_and(p_value: f64, p_mask: i64) -> i64 {
    if !p_value.is_finite() {
        return 0;
    }
    let v_int = (p_value.trunc() as i128).rem_euclid(1i128 << 32) as u32 as i32;
    (v_int as i64) & p_mask
}

/// fn_handleGcsPeerRequest: 9021 UdpProxyPeer from a GCS,
/// ms {tg_unit, act: 'allow'|'reset'|'query'}.
pub async fn handle_gcs_peer_request(p_server: &Arc<Server>, p_ctx: Ctx, mut p_jmsg: Value) {
    let v_req = p_jmsg.get("ms").filter(|m| m.is_object()).cloned().unwrap_or_else(|| json!({}));
    let v_tg_unit = field(&v_req, "tg_unit").cloned();
    let v_act = field(&v_req, "act").and_then(Value::as_str).map(str::to_string);
    let fn_reply = |p_jmsg: &mut Value, p_ms: Value| {
        let mut v_out = Map::new();
        if let Some(t) = &v_tg_unit {
            v_out.insert("tg_unit".into(), t.clone());
        }
        if let Value::Object(m) = p_ms {
            for (k, v) in m {
                v_out.insert(k, v);
            }
        }
        reply_to_unit(p_server, &p_ctx, p_jmsg, Value::Object(v_out));
    };
    let Some(v_login) = p_ctx.login.clone().filter(|l| l.actor_is(CONST_ACTOR_TYPE_GCS)) else { return };
    let v_act = match (v_tg_unit.as_ref().and_then(Value::as_str), v_act.as_deref()) {
        (Some(_), Some(a @ ("allow" | "reset" | "query" | "switch" | "reject" | "kick" | "change_port"))) => {
            a.to_string()
        }
        _ => {
            fn_reply(&mut p_jmsg, json!({ "error": "bad_request" }));
            return;
        }
    };
    let v_tg = v_tg_unit.as_ref().and_then(Value::as_str).unwrap().to_string();
    let v_unit_login = {
        let v_hub = p_server.hub();
        v_hub.active_sender_by_name(&v_tg).and_then(|c| v_hub.clients[&c].login.clone())
    };
    let v_unit_login = match v_unit_login {
        Some(l)
            if l.account_key() == v_login.account_key()
                && js_key(l.group_id.as_ref()) == js_key(v_login.group_id.as_ref()) =>
        {
            l
        }
        _ => {
            fn_reply(&mut p_jmsg, json!({ "error": "not_allowed" }));
            return;
        }
    };
    let v_full = CONST_PERMISSION_ALLOW_GCS_FULL_CONTROL;
    if v_act != "query" && js_and(js_number(v_login.prm.as_ref()), v_full) != v_full {
        fn_reply(&mut p_jmsg, json!({ "error": "permission_denied" }));
        return;
    }
    let v_account = v_unit_login.account_id.clone();
    let v_entry = {
        let st = p_server.relay.m_st.lock().unwrap();
        st.units.get(&unit_key(v_account.as_ref(), Some(&json!(v_tg)))).map(|e| (e.state, e.relay.clone()))
    };
    let v_relay = match v_entry {
        Some((_, r)) if p_server.relay.is_enabled() => r,
        _ => {
            fn_reply(&mut p_jmsg, json!({ "pinned": { "gcs": { "mode": "open" } } }));
            return;
        }
    };
    let Some(v_relay) = v_relay.filter(|r| r.is_connected()) else {
        fn_reply(&mut p_jmsg, json!({ "error": "relay_unavailable" }));
        return;
    };
    let v_ip = p_server.hub().client(p_ctx.conn).and_then(|c| c.remote_ip.clone());
    let v_result: Result<Value, String> = async {
        let mut v_applied: Option<bool> = None;
        let mut v_secs_echo: Option<Value> = None;
        if v_act == "allow" {
            let Some(ip) = v_ip.filter(|ip| !ip.is_empty()) else {
                return Ok(json!({ "error": "no_ip" }));
            };
            let r = v_relay.request("peer_allow", json!({ "account": v_account, "unit": v_tg, "ips": [ip] })).await?;
            v_applied = Some(is_success(&r) && field(&r["ms"], "applied") == Some(&Value::Bool(true)));
        } else if v_act == "reset" {
            let r = v_relay.request("peer_reset", json!({ "account": v_account, "unit": v_tg, "side": "gcs" })).await?;
            v_applied = Some(is_success(&r) && field(&r["ms"], "applied") == Some(&Value::Bool(true)));
        } else if v_act == "switch" {
            // approve the waiting attempt - the lock moves to {ip, port}
            let (Some(ip), Some(port)) = (field(&v_req, "ip").and_then(Value::as_str), field(&v_req, "port")) else {
                return Ok(json!({ "error": "bad_request" }));
            };
            let r = v_relay
                .request("peer_switch", json!({ "account": v_account, "unit": v_tg, "ip": ip, "port": port }))
                .await?;
            v_applied = Some(is_success(&r) && field(&r["ms"], "applied") == Some(&Value::Bool(true)));
        } else if v_act == "reject" {
            let Some(ip) = field(&v_req, "ip").and_then(Value::as_str) else {
                return Ok(json!({ "error": "bad_request" }));
            };
            let v_secs = js_number(field(&v_req, "secs"));
            let mut v_ms = json!({ "account": v_account, "unit": v_tg, "ip": ip });
            if v_secs.is_finite() && v_secs > 0.0 {
                v_ms["secs"] = json!(v_secs.min(GCS_REJECT_CAP_S));
            }
            let r = v_relay.request("peer_reject", v_ms).await?;
            v_applied = Some(is_success(&r) && field(&r["ms"], "applied") == Some(&Value::Bool(true)));
            v_secs_echo = field(&r["ms"], "secs").cloned();
        } else if v_act == "kick" {
            let v_secs = js_number(field(&v_req, "secs"));
            let mut v_ms = json!({ "account": v_account, "unit": v_tg });
            if v_secs.is_finite() && v_secs > 0.0 {
                v_ms["secs"] = json!(v_secs.min(GCS_REJECT_CAP_S));
            }
            let r = v_relay.request("peer_kick", v_ms).await?;
            v_applied = Some(is_success(&r) && field(&r["ms"], "applied") == Some(&Value::Bool(true)));
            v_secs_echo = field(&r["ms"], "secs").cloned();
        } else if v_act == "change_port" {
            let r = v_relay.request("peer_change_port", json!({ "account": v_account, "unit": v_tg })).await?;
            v_applied = Some(is_success(&r) && field(&r["ms"], "applied") == Some(&Value::Bool(true)));
        }
        let v_pinned = relay_pinned(&v_relay, v_account.as_ref(), &v_tg).await?;
        let mut v_ms = Map::new();
        v_ms.insert("pinned".into(), v_pinned);
        if let Some(a) = v_applied {
            v_ms.insert("applied".into(), json!(a));
        }
        if let Some(s) = v_secs_echo {
            v_ms.insert("secs".into(), s);
        }
        Ok(Value::Object(v_ms))
    }
    .await;
    match v_result {
        Ok(v_ms) => {
            // the requester's reply goes first, then the group's other
            // GCSs get the fresh state — an answered request clears
            // everywhere
            if v_ms.get("applied") == Some(&Value::Bool(true)) {
                let v_pinned = v_ms.get("pinned").cloned().unwrap_or(Value::Null);
                fn_reply(&mut p_jmsg, v_ms);
                let mut v_out = Map::new();
                v_out.insert("tg_unit".into(), json!(v_tg));
                v_out.insert("act".into(), json!("state"));
                v_out.insert("pinned".into(), v_pinned);
                push_to_gcs(
                    p_server,
                    v_unit_login.account_id.as_ref(),
                    v_unit_login.group_id.as_ref(),
                    Value::Object(v_out),
                );
            } else {
                fn_reply(&mut p_jmsg, v_ms);
            }
        }
        Err(e) => {
            log(&format!("9021 {} for '{}' failed: {}", v_act, v_tg, e));
            fn_reply(&mut p_jmsg, json!({ "error": "relay_unavailable" }));
        }
    }
}

// ----------------------------------------------------------------------
// AUTH dashboard (e / g)
// ----------------------------------------------------------------------

/// fn_listRelay: pairs this comm server owns on p_relay in the dashboard
/// row shape (plus account/group/owner/relay_id/online/rx/tx/pinned).
async fn list_relay(p_server: &Arc<Server>, p_relay: Arc<RelayConn>) -> Vec<(Value, Arc<RelayConn>)> {
    let v_rep = match p_relay.request("list", json!({})).await {
        Ok(r) => r,
        Err(e) => {
            log(&format!("relay {} list failed: {}", p_relay.endpoint.id, e));
            return Vec::new();
        }
    };
    let Some(Value::Array(v_pairs)) = v_rep.get("ms").filter(|_| is_success(&v_rep)).and_then(|m| m.get("pairs"))
    else {
        return Vec::new();
    };
    let v_relay_id = v_rep["ms"].get("relay_id").cloned();
    v_pairs
        .iter()
        .filter(|r| js_strict_eq(r.get("owner"), p_server.cfg.get("server_id")))
        .map(|r| {
            let mut v_row = Map::new();
            let fn_put = |m: &mut Map<String, Value>, k: &str, v: Option<&Value>| {
                if let Some(v) = v {
                    m.insert(k.into(), v.clone());
                }
            };
            fn_put(&mut v_row, "name", r.get("unit"));
            fn_put(&mut v_row, "account", r.get("account"));
            fn_put(&mut v_row, "group", r.get("group"));
            fn_put(&mut v_row, "owner", r.get("owner"));
            fn_put(&mut v_row, "relay_id", v_relay_id.as_ref());
            for s in ["socket1", "socket2"] {
                let mut v_s = Map::new();
                fn_put(&mut v_s, "address", r.get(s).and_then(|x| x.get("address")));
                fn_put(&mut v_s, "port", r.get(s).and_then(|x| x.get("port")));
                v_row.insert(s.into(), Value::Object(v_s));
            }
            for k in ["ready", "blocked", "created", "last_access", "online", "rx", "tx"] {
                fn_put(&mut v_row, k, r.get(k));
            }
            if let Some(p) = r.get("pinned").filter(|p| !p.is_null()) {
                v_row.insert("pinned".into(), p.clone());
            }
            (Value::Object(v_row), p_relay.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_push_payload_keeps_tk() {
        let v_open = json!({ "en": true, "socket1": {"address":"1.2.3.4","port":1}, "socket2": {"address":"1.2.3.4","port":2}, "tk": "abc" });
        let v_p = unit_payload(&v_open);
        assert_eq!(v_p["tk"], "abc");
        assert_eq!(v_p["socket2"]["port"], 2);
    }

    #[test]
    fn changed_branch_carries_push_tk() {
        // DE-0072: the changed branch of on_relay_push must put tk in the
        // message to the unit - the push's own tk wins
        let v_ms = json!({ "socket1": {"address":"1.2.3.4","port":1},
                           "socket2": {"address":"1.2.3.4","port":2},
                           "tk": "pushtk" });
        let v_open = changed_open_payload(&v_ms, &Some(json!("storedtk")));
        assert_eq!(v_open["tk"], "pushtk");
        assert_eq!(v_open["en"], true);
        assert_eq!(v_open["socket2"]["port"], 2);
        let v_p = unit_payload(&v_open);
        assert_eq!(v_p["tk"], "pushtk");
    }

    #[test]
    fn changed_branch_uses_stored_tk_when_push_has_none() {
        // DE-0072: a changed push without tk falls back to the token
        // stored from the last open reply
        let v_ms = json!({ "socket1": {"address":"1.2.3.4","port":1},
                           "socket2": {"address":"1.2.3.4","port":2} });
        let v_open = changed_open_payload(&v_ms, &Some(json!("storedtk")));
        assert_eq!(v_open["tk"], "storedtk");
        let v_p = unit_payload(&v_open);
        assert_eq!(v_p["tk"], "storedtk");
    }
}
