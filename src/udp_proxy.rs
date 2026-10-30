//! Built-in UDP proxy (js_udp_proxy.js).
//!
//! A drone's 9008 request gets a pair of UDP sockets on 0.0.0.0: a datagram
//! on socket1 goes out of socket2 to socket2's last sender and the other way
//! round. Pairs are keyed by `<accountID>/<unit name>`, so `drone1` in two
//! accounts gets two pairs. A pair can be blocked by the AUTH dashboard
//! (packets silently dropped).
//!
//! A reaper runs every minute while pairs exist:
//!   - a timer gap above 3 minutes means the host was suspended: every pair
//!     is recreated on its ports;
//!   - a pair that is not ready is recreated (the unit is told when that
//!     fails);
//!   - a pair idle for 5 minutes whose unit is disconnected is closed.

use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use crate::colors as c;
use crate::constants::*;
use crate::server::Server;
use crate::util::{js_interp, js_key, js_number, js_strict_eq, js_truthy, now_ms};

const PROXY_IDLE_TIMEOUT_MS: i64 = 5 * 60 * 1000;
const REAPER_INTERVAL_MS: i64 = 60 * 1000;
const UDP_RECV_BUFFER_SIZE: usize = 4 * 1024 * 1024;
const UDP_SEND_BUFFER_SIZE: usize = 4 * 1024 * 1024;

/// Checks (and raises, as root) the Linux kernel UDP buffer limits, printing
/// how to fix them otherwise. Skipped on other platforms.
pub fn check_and_fix_kernel_buffers() {
    if std::env::consts::OS != "linux" {
        return;
    }
    const RMEM_MAX_PATH: &str = "/proc/sys/net/core/rmem_max";
    const WMEM_MAX_PATH: &str = "/proc/sys/net/core/wmem_max";
    let fn_read = |p: &str| -> Result<i64, String> {
        let t = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
        Ok(crate::util::js_parse_int(Some(&json!(t.trim()))).unwrap_or(0))
    };
    let (v_rmem, v_wmem) = match (fn_read(RMEM_MAX_PATH), fn_read(WMEM_MAX_PATH)) {
        (Ok(r), Ok(w)) => (r, w),
        (Err(e), _) | (_, Err(e)) => {
            println!("{}UDP buffer check failed: {}{}", c::ERROR, e, c::RESET);
            return;
        }
    };
    let mut v_issues = Vec::new();
    if v_rmem < UDP_RECV_BUFFER_SIZE as i64 {
        v_issues.push(("rmem_max", v_rmem, UDP_RECV_BUFFER_SIZE, RMEM_MAX_PATH));
    }
    if v_wmem < UDP_SEND_BUFFER_SIZE as i64 {
        v_issues.push(("wmem_max", v_wmem, UDP_SEND_BUFFER_SIZE, WMEM_MAX_PATH));
    }
    if v_issues.is_empty() {
        println!(
            "UDP buffers {}OK{}: rmem_max={}{}{}, wmem_max={}{}{}",
            c::B_SUCCESS,
            c::RESET,
            c::B_SUCCESS,
            v_rmem,
            c::RESET,
            c::B_SUCCESS,
            v_wmem,
            c::RESET
        );
        return;
    }
    println!("==================================");
    println!("{}UDP BUFFER WARNING{}: Kernel limits are below requested socket buffer sizes.", c::FG_YELLOW, c::RESET);
    println!("Socket buffers will be clamped, potentially causing packet drops under load.");
    println!("----------------------------------");
    for (v_param, v_current, v_required, v_path) in v_issues {
        println!(
            "   {}: current={}{}{}, required={}{}{}",
            v_param,
            c::ERROR,
            v_current,
            c::RESET,
            c::B_SUCCESS,
            v_required,
            c::RESET
        );
        match std::fs::write(v_path, v_required.to_string()) {
            Ok(()) => println!("   {}Updated {} to {}{}", c::B_SUCCESS, v_param, v_required, c::RESET),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                println!("   {}Cannot update {} (requires root privileges){}", c::ERROR, v_param, c::RESET)
            }
            Err(e) => println!("   {}Failed to update {}: {}{}", c::ERROR, v_param, e, c::RESET),
        }
    }
    println!("----------------------------------");
    println!("To fix permanently, run as root:");
    println!("{}   echo {} > {}{}", c::BRIGHT, UDP_RECV_BUFFER_SIZE, RMEM_MAX_PATH, c::RESET);
    println!("{}   echo {} > {}{}", c::BRIGHT, UDP_SEND_BUFFER_SIZE, WMEM_MAX_PATH, c::RESET);
    println!("Or add to /etc/sysctl.conf:");
    println!("{}   net.core.rmem_max = {}{}", c::BRIGHT, UDP_RECV_BUFFER_SIZE, c::RESET);
    println!("{}   net.core.wmem_max = {}{}", c::BRIGHT, UDP_SEND_BUFFER_SIZE, c::RESET);
    println!("==================================");
}

/// The port dgram.bind uses for a value: Number(value), truncated and
/// wrapped to 16 bits (-1 -> 65535, 1.5 -> 1, "0x10" -> 16); NaN is 0, an
/// ephemeral port.
fn bind_port(p_port: &Value) -> u16 {
    let v = js_number(Some(p_port));
    if !v.is_finite() {
        return 0;
    }
    (v.trunc() as i128).rem_euclid(65536) as u16
}

fn bind_udp(p_port: u16) -> Option<std::net::UdpSocket> {
    let v_sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).ok()?;
    v_sock.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, p_port)).into()).ok()?;
    let _ = v_sock.set_recv_buffer_size(UDP_RECV_BUFFER_SIZE);
    let _ = v_sock.set_send_buffer_size(UDP_SEND_BUFFER_SIZE);
    v_sock.set_nonblocking(true).ok()?;
    Some(v_sock.into())
}

/// One side of a pair as the receive tasks see it.
#[derive(Default)]
struct SideShared {
    sock: Mutex<Option<Arc<UdpSocket>>>,
    caller: Mutex<Option<SocketAddr>>,
    last_access: AtomicI64,
}

#[derive(Default)]
struct ProxyShared {
    blocked: AtomicBool,
    sides: [SideShared; 2],
}

/// udp_proxy: two sockets and their receive tasks.
struct Proxy {
    shared: Arc<ProxyShared>,
    /// _port of each socket: the bound port once listening, else as requested
    ports: [Value; 2],
    ready: [bool; 2],
    tasks: Vec<JoinHandle<()>>,
}

impl Proxy {
    /// new udp_proxy("0.0.0.0", port1, "0.0.0.0", port2)
    fn new(p_port1: &Value, p_port2: &Value) -> Proxy {
        let v_shared = Arc::new(ProxyShared::default());
        let mut v_proxy = Proxy {
            shared: v_shared.clone(),
            ports: [Value::Null, Value::Null],
            ready: [false, false],
            tasks: Vec::new(),
        };
        for (i, v_req) in [p_port1, p_port2].into_iter().enumerate() {
            // port = port || 0
            let v_req = if js_truthy(Some(v_req)) { v_req.clone() } else { json!(0) };
            v_proxy.ports[i] = v_req.clone();
            let v_port = bind_port(&v_req);
            let Some(v_std) = bind_udp(v_port) else {
                println!("UDP Listener Cannot Open 0.0.0.0 at port {}", js_key(Some(&v_req)));
                println!("socket error: Error: bind EADDRINUSE 0.0.0.0:{}", v_port);
                continue;
            };
            let Ok(v_sock) = UdpSocket::from_std(v_std) else { continue };
            let v_sock = Arc::new(v_sock);
            let v_actual = v_sock.local_addr().map(|a| a.port()).unwrap_or(v_port);
            v_proxy.ports[i] = json!(v_actual);
            v_proxy.ready[i] = true;
            println!("UDP Listener Active 0.0.0.0 at port {}", v_actual);
            *v_shared.sides[i].sock.lock().unwrap() = Some(v_sock.clone());
            v_proxy.tasks.push(tokio::spawn(recv_loop(v_shared.clone(), i, v_sock)));
        }
        v_proxy
    }

    fn is_ready(&self) -> bool {
        self.ready[0] && self.ready[1]
    }

    /// Closes both sockets; they are released once the returned tasks are
    /// awaited (see close_and_wait).
    fn close(&mut self) -> Vec<JoinHandle<()>> {
        self.ready = [false, false];
        for s in &self.shared.sides {
            *s.sock.lock().unwrap() = None;
        }
        self.tasks.drain(..).inspect(|t| t.abort()).collect()
    }

    /// Closes both sockets and waits until their ports are free again.
    async fn close_and_wait(&mut self) {
        for t in self.close() {
            let _ = t.await;
        }
    }

    fn last_access(&self) -> i64 {
        self.shared.sides[0]
            .last_access
            .load(Ordering::Relaxed)
            .max(self.shared.sides[1].last_access.load(Ordering::Relaxed))
    }

    /// getConfig: {socket1: {address, port}, socket2: {...}}; the address
    /// is public_host (the sockets listen on 0.0.0.0).
    fn config(&self, p_public_host: Option<&Value>) -> Map<String, Value> {
        let fn_side = |i: usize| {
            let mut m = Map::new();
            if let Some(h) = p_public_host {
                m.insert("address".into(), h.clone());
            }
            m.insert("port".into(), self.ports[i].clone());
            Value::Object(m)
        };
        let mut v_out = Map::new();
        v_out.insert("socket1".into(), fn_side(0));
        v_out.insert("socket2".into(), fn_side(1));
        v_out
    }
}

async fn recv_loop(p_shared: Arc<ProxyShared>, p_side: usize, p_sock: Arc<UdpSocket>) {
    let mut v_buf = vec![0u8; 65536];
    loop {
        let Ok((n, v_from)) = p_sock.recv_from(&mut v_buf).await else {
            tokio::time::sleep(Duration::from_millis(1)).await;
            continue;
        };
        let v_me = &p_shared.sides[p_side];
        v_me.last_access.store(now_ms(), Ordering::Relaxed);
        *v_me.caller.lock().unwrap() = Some(v_from);
        if p_shared.blocked.load(Ordering::Relaxed) {
            continue;
        }
        let v_other = &p_shared.sides[1 - p_side];
        let v_peer = *v_other.caller.lock().unwrap();
        let v_sock = v_other.sock.lock().unwrap().clone();
        if let (Some(v_peer), Some(v_sock)) = (v_peer, v_sock) {
            let _ = v_sock.try_send_to(&v_buf[..n], v_peer);
        }
    }
}

struct Entry {
    name: Option<Value>,
    account: Option<Value>,
    created: i64,
    last_access: i64,
    socket1_port: Value,
    socket2_port: Value,
    blocked: bool,
    proxy: Proxy,
}

impl Entry {
    fn last_access(&self) -> i64 {
        self.last_access.max(self.proxy.last_access())
    }
}

#[derive(Default)]
struct UdpState {
    entries: IndexMap<String, Entry>,
    reaper: Option<JoinHandle<()>>,
    last_tick: i64,
}

/// The proxies; operations are async because a closed socket is only
/// released once its receive task has ended.
pub struct UdpProxy {
    m_state: tokio::sync::Mutex<UdpState>,
}

impl Default for UdpProxy {
    fn default() -> Self {
        Self::new()
    }
}

/// `<account>/<name>`
pub fn proxy_key(p_account: Option<&Value>, p_name: Option<&Value>) -> String {
    let v_account = match p_account {
        None | Some(Value::Null) => String::new(),
        Some(a) => js_key(Some(a)),
    };
    format!("{}/{}", v_account, js_key(p_name))
}

fn zero_sockets() -> Map<String, Value> {
    let mut v_ms = Map::new();
    v_ms.insert("socket1".into(), json!({ "address": "0.0.0.0", "port": 0 }));
    v_ms.insert("socket2".into(), json!({ "address": "0.0.0.0", "port": 0 }));
    v_ms
}

impl UdpProxy {
    pub fn new() -> UdpProxy {
        UdpProxy { m_state: tokio::sync::Mutex::new(UdpState::default()) }
    }

    fn public_host(p_server: &Server) -> Option<Value> {
        p_server.cfg.get("public_host").cloned()
    }

    /// The unit's live socket, only if it is this entry's unit (same account).
    fn entry_socket(
        p_server: &Server,
        p_name: Option<&Value>,
        p_account: Option<&Value>,
    ) -> Option<Arc<crate::hub::ConnHandle>> {
        let v_hub = p_server.hub();
        let v_conn = v_hub.active_sender(p_name)?;
        let v_client = v_hub.client(v_conn)?;
        if !v_client.handle.is_open() {
            return None;
        }
        if let (Some(l), Some(a)) = (v_client.login.as_ref(), p_account.filter(|a| !a.is_null())) {
            if l.account_key() != js_key(Some(a)) {
                return None;
            }
        }
        Some(v_client.handle.clone())
    }

    /// getUDPSocket: opens (or reuses) the pair of unit p_name in p_account.
    /// @returns the 9008 reply payload {socket1, socket2, en}
    pub async fn get_udp_socket(
        &self,
        p_server: &Arc<Server>,
        p_name: Option<&Value>,
        p_socket1: &Value,
        p_socket2: &Value,
        p_account: Option<&Value>,
    ) -> Map<String, Value> {
        let v_key = proxy_key(p_account, p_name);
        let v_p1 = crate::util::field(p_socket1, "port").cloned();
        let v_p2 = crate::util::field(p_socket2, "port").cloned();
        let v_host = Self::public_host(p_server);
        let mut st = self.m_state.lock().await;
        println!(
            "getUDPSocket('{}', socket1.port={}, socket2.port={}) existing={}",
            v_key,
            js_interp(v_p1.as_ref()),
            js_interp(v_p2.as_ref()),
            st.entries.contains_key(&v_key)
        );
        if let Some(v_entry) = st.entries.get_mut(&v_key) {
            let v_ms = v_entry.proxy.config(v_host.as_ref());
            let fn_same = |want: &Option<Value>, have: &Value| {
                js_strict_eq(want.as_ref(), Some(&json!(0))) || js_strict_eq(want.as_ref(), Some(have))
            };
            if fn_same(&v_p1, &v_ms["socket1"]["port"]) && fn_same(&v_p2, &v_ms["socket2"]["port"]) {
                if !v_entry.proxy.is_ready() {
                    let v_enabled = Self::recreate(&v_key, v_entry, "getUDPSocket refresh").await;
                    let mut v_ms = v_entry.proxy.config(v_host.as_ref());
                    v_ms.insert("en".into(), json!(v_enabled));
                    return v_ms;
                }
                v_entry.last_access = now_ms();
                let mut v_ms = v_ms;
                v_ms.insert("en".into(), json!(true));
                return v_ms;
            }
            // different ports: close the old pair, then open a new one
            if let Some(mut v_old) = st.entries.shift_remove(&v_key) {
                v_old.proxy.close_and_wait().await;
            }
        }
        let v_proxy = Proxy::new(v_p1.as_ref().unwrap_or(&Value::Null), v_p2.as_ref().unwrap_or(&Value::Null));
        let v_now = now_ms();
        let mut v_entry = Entry {
            name: p_name.cloned(),
            account: p_account.cloned(),
            created: v_now,
            last_access: v_now,
            socket1_port: if js_truthy(v_p1.as_ref()) { v_p1.clone().unwrap() } else { json!(0) },
            socket2_port: if js_truthy(v_p2.as_ref()) { v_p2.clone().unwrap() } else { json!(0) },
            blocked: false,
            proxy: v_proxy,
        };
        let mut v_ms = v_entry.proxy.config(v_host.as_ref());
        v_entry.socket1_port = v_ms["socket1"]["port"].clone();
        v_entry.socket2_port = v_ms["socket2"]["port"].clone();
        let v_enabled = v_entry.proxy.is_ready();
        v_ms.insert("en".into(), json!(v_enabled));
        println!(
            "getUDPSocket('{}') proxy ready={} socket1={} socket2={}",
            v_key,
            v_enabled,
            crate::util::js_stringify(&v_ms["socket1"]),
            crate::util::js_stringify(&v_ms["socket2"])
        );
        st.entries.insert(v_key, v_entry);
        self.start_reaper(p_server, &mut st);
        v_ms
    }

    /// closeUDPSocket: closes the pair of p_name in p_account.
    /// @returns {socket1, socket2, en:false}
    pub async fn close_udp_socket(
        &self,
        p_server: &Server,
        p_name: Option<&Value>,
        p_account: Option<&Value>,
    ) -> Map<String, Value> {
        let v_key = proxy_key(p_account, p_name);
        let mut st = self.m_state.lock().await;
        let mut v_ms = match st.entries.shift_remove(&v_key) {
            Some(mut e) => {
                let v_ms = e.proxy.config(p_server.cfg.get("public_host"));
                e.proxy.close_and_wait().await;
                v_ms
            }
            None => zero_sockets(),
        };
        v_ms.insert("en".into(), json!(false));
        v_ms
    }

    /// Closes both sockets and rebinds the sticky ports, keeping the block.
    /// @returns whether the new pair is ready.
    async fn recreate(p_key: &str, p_entry: &mut Entry, p_reason: &str) -> bool {
        let v_config = p_entry.proxy.config(None);
        let fn_sticky = |sticky: &Value, cur: &Value| {
            if js_truthy(Some(sticky)) {
                sticky.clone()
            } else if js_truthy(Some(cur)) {
                cur.clone()
            } else {
                json!(0)
            }
        };
        let v_p1 = fn_sticky(&p_entry.socket1_port, &v_config["socket1"]["port"]);
        let v_p2 = fn_sticky(&p_entry.socket2_port, &v_config["socket2"]["port"]);
        p_entry.proxy.close_and_wait().await;
        p_entry.proxy = Proxy::new(&v_p1, &v_p2);
        let v_enabled = p_entry.proxy.is_ready();
        if v_enabled {
            p_entry.socket1_port = p_entry.proxy.ports[0].clone();
            p_entry.socket2_port = p_entry.proxy.ports[1].clone();
            println!("UDP proxy '{}' recreated successfully ({}).", p_key, p_reason);
        } else {
            println!("UDP proxy '{}' recreate failed ({}).", p_key, p_reason);
        }
        // an admin-blocked proxy stays blocked across recreates
        p_entry.proxy.shared.blocked.store(p_entry.blocked, Ordering::Relaxed);
        p_entry.last_access = now_ms();
        v_enabled
    }

    /// Tells a connected unit its pair was closed (9008 en:false).
    fn notify_unit_proxy_closed(p_server: &Server, p_name: Option<&Value>, p_account: Option<&Value>, p_reason: &str) {
        let Some(v_handle) = Self::entry_socket(p_server, p_name, p_account) else { return };
        let mut v_msg = Map::new();
        v_msg.insert(CONST_WS_MESSAGE_ID.into(), json!(CONST_TYPE_ANDRUAV_SYSTEM_UDP_PROXY));
        v_msg.insert(CONST_WS_MSG_ROUTING.into(), json!(CONST_WS_MSG_ROUTING_INDIVIDUAL));
        if let Some(n) = p_name {
            v_msg.insert(CONST_WS_TARGET_ID.into(), n.clone());
        }
        v_msg.insert(CONST_WS_SENDER_ID.into(), json!(CONST_WS_SENDER_COMM_SERVER));
        let mut v_ms = Map::new();
        v_ms.insert("en".into(), json!(false));
        v_ms.extend(zero_sockets());
        v_msg.insert(CONST_WS_PAYLOAD.into(), Value::Object(v_ms));
        v_handle.send_json(&Value::Object(v_msg));
        println!("Reaper: Notified unit '{}' that its UDP proxy was closed ({}).", js_key(p_name), p_reason);
    }

    fn start_reaper(&self, p_server: &Arc<Server>, st: &mut UdpState) {
        if st.reaper.is_some() {
            return;
        }
        st.last_tick = now_ms();
        let v_server = p_server.clone();
        st.reaper = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(REAPER_INTERVAL_MS as u64)).await;
                v_server.udp.reap(&v_server, now_ms()).await;
            }
        }));
    }

    /// One reaper pass at time p_now (the timer calls it every minute).
    pub async fn reap(&self, p_server: &Arc<Server>, p_now: i64) {
        let mut v_notify: Vec<(Option<Value>, Option<Value>, &'static str)> = Vec::new();
        {
            let mut st = self.m_state.lock().await;
            let v_gap = p_now - st.last_tick;
            if st.last_tick > 0 && v_gap > REAPER_INTERVAL_MS * 3 && !st.entries.is_empty() {
                println!(
                    "Reaper: Detected possible system resume (timer gap {} ms). Recreating {} UDP proxy instance(s).",
                    v_gap,
                    st.entries.len()
                );
                for (k, e) in st.entries.iter_mut() {
                    Self::recreate(k, e, "resume recovery").await;
                }
            }
            st.last_tick = p_now;
            let v_keys: Vec<String> = st.entries.keys().cloned().collect();
            for v_key in v_keys {
                let Some(v_entry) = st.entries.get_mut(&v_key) else { continue };
                if !v_entry.proxy.is_ready() {
                    if !Self::recreate(&v_key, v_entry, "health check not ready").await {
                        v_notify.push((v_entry.name.clone(), v_entry.account.clone(), "health check recreate failed"));
                    }
                    continue;
                }
                if p_now - v_entry.last_access() <= PROXY_IDLE_TIMEOUT_MS {
                    continue;
                }
                // a connected unit keeps its proxy (telemetry may be paused)
                if Self::entry_socket(p_server, v_entry.name.as_ref(), v_entry.account.as_ref()).is_some() {
                    continue;
                }
                let mut v_entry = st.entries.shift_remove(&v_key).unwrap();
                println!(
                    "Reaper: Closing idle UDP proxy '{}' (idle for {}s, unit disconnected)",
                    v_key,
                    ((p_now - v_entry.last_access()) as f64 / 1000.0).round()
                );
                v_entry.proxy.close_and_wait().await;
                v_notify.push((v_entry.name.clone(), v_entry.account.clone(), "idle timeout (unit disconnected)"));
            }
            if st.entries.is_empty() {
                if let Some(t) = st.reaper.take() {
                    t.abort();
                }
                st.last_tick = 0;
            }
        }
        for (n, a, r) in v_notify {
            Self::notify_unit_proxy_closed(p_server, n.as_ref(), a.as_ref(), r);
        }
    }

    /// fn_getActiveProxiesList: every tracked proxy for AUTH's dashboard.
    pub async fn active_proxies_list(&self, p_server: &Server) -> Vec<Value> {
        let st = self.m_state.lock().await;
        st.entries
            .values()
            .map(|e| {
                let v_config = e.proxy.config(p_server.cfg.get("public_host"));
                let mut v_row = Map::new();
                if let Some(n) = &e.name {
                    v_row.insert("name".into(), n.clone());
                }
                if let Some(a) = &e.account {
                    v_row.insert("account".into(), a.clone());
                }
                v_row.insert("socket1".into(), v_config["socket1"].clone());
                v_row.insert("socket2".into(), v_config["socket2"].clone());
                v_row.insert("ready".into(), json!(e.proxy.is_ready()));
                v_row.insert("blocked".into(), json!(e.blocked));
                v_row.insert("created".into(), json!(e.created));
                v_row.insert("last_access".into(), json!(e.last_access()));
                Value::Object(v_row)
            })
            .collect()
    }

    /// fn_setProxyBlocked: with p_account only that account's pair; with only
    /// a name the single pair of that name (none when several accounts have
    /// one). @returns true when one proxy was updated.
    pub async fn set_proxy_blocked(&self, p_name: &str, p_blocked: bool, p_account: Option<&Value>) -> bool {
        let mut st = self.m_state.lock().await;
        let v_key = match p_account.filter(|a| !a.is_null()) {
            Some(a) => {
                let k = proxy_key(Some(a), Some(&json!(p_name)));
                if !st.entries.contains_key(&k) {
                    return false;
                }
                k
            }
            None => {
                let v_matches: Vec<String> = st
                    .entries
                    .iter()
                    .filter(|(_, e)| e.name.as_ref().and_then(Value::as_str) == Some(p_name))
                    .map(|(k, _)| k.clone())
                    .collect();
                if v_matches.len() > 1 {
                    println!("UDP proxy '{}' exists in {} accounts; block needs an account.", p_name, v_matches.len());
                    return false;
                }
                match v_matches.into_iter().next() {
                    Some(k) => k,
                    None => return false,
                }
            }
        };
        let v_entry = st.entries.get_mut(&v_key).unwrap();
        v_entry.blocked = p_blocked;
        v_entry.proxy.shared.blocked.store(p_blocked, Ordering::Relaxed);
        println!(
            "UDP proxy '{}' {} by admin.",
            proxy_key(v_entry.account.as_ref(), v_entry.name.as_ref()),
            if p_blocked { "BLOCKED (packets dropped)" } else { "unblocked (forwarding resumed)" }
        );
        true
    }

    /// Number of pairs (tests).
    pub async fn pair_count(&self) -> usize {
        self.m_state.lock().await.entries.len()
    }
}
