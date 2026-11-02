//! Client connections (js_chat_connection.js): the HTTPS/WebSocket server,
//! login (legacy query-string credentials or the de_auth frame), accepting a
//! party into its account/group, and cleanup when its socket closes.
//!
//! The JavaScript server was an Express app on an https server with the ws
//! library attached. Here the HTTP request head is read directly: a
//! WebSocket upgrade is answered like ws does (first offered subprotocol
//! selected, no permessage-deflate), anything else gets Express's 404 page.

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{sleep_until, Instant};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::colors as c;
use crate::constants::*;
use crate::hub::{Client, ConnHandle, Login, Out, Status};
use crate::server::Server;
use crate::util::{js_key, js_parse_bytes, js_stringify, now_ms};
use crate::validation::is_alphanumeric;

/// A party has this long to send its de_auth frame.
const AUTH_FRAME_TIMEOUT: Duration = Duration::from_millis(8000);
/// ws closeTimeout: how long a closing socket waits for the peer's close frame.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HEADER_BYTES: usize = 16 * 1024;
const PARAM_LENGTH: usize = 200;

/// getHeaderParams: `[?&]key=value` pairs of a URL (not URL-decoded).
pub fn header_params(p_url: &str) -> Map<String, Value> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let v_re = RE.get_or_init(|| Regex::new(r"[?&]([^=#]+)=([^&#]*)").unwrap());
    let mut v_params = Map::new();
    for v_cap in v_re.captures_iter(p_url) {
        v_params.insert(v_cap[1].to_string(), Value::String(v_cap[2].to_string()));
    }
    v_params
}

/// fn_getClientIp: the first X-Forwarded-For entry when trust_proxy_header
/// is true, else the socket address; IPv4-mapped IPv6 is reduced to IPv4.
pub fn client_ip(p_trust_proxy: bool, p_forwarded_for: Option<&str>, p_remote: Option<SocketAddr>) -> Option<String> {
    let mut v_ip: Option<String> = None;
    if p_trust_proxy {
        if let Some(x) = p_forwarded_for.filter(|x| !x.is_empty()) {
            v_ip = Some(x.split(',').next().unwrap_or("").trim().to_string());
        }
    }
    if v_ip.as_deref().map(str::is_empty).unwrap_or(true) {
        v_ip = p_remote.map(|a| remote_address(&a));
    }
    let v_ip = v_ip?;
    if v_ip.len() >= 7 && v_ip[..7].eq_ignore_ascii_case("::ffff:") {
        return Some(v_ip[7..].to_string());
    }
    (!v_ip.is_empty()).then_some(v_ip)
}

/// req.socket.remoteAddress as Node prints it ("::ffff:1.2.3.4" on a
/// dual-stack listener).
pub fn remote_address(p_addr: &SocketAddr) -> String {
    p_addr.ip().to_string()
}

// ----------------------------------------------------------------------
// accepting a party
// ----------------------------------------------------------------------

/// send_ok_message: the 9007 welcome on a newly accepted socket.
fn send_ok_message(p_handle: &ConnHandle) {
    let v_addr = p_handle.remote_addr.map(|a| format!("{}:{}", remote_address(&a), a.port())).unwrap_or_default();
    let v_msg = json!({
        "ty": "s",
        "mt": CONST_TYPE_ANDRUAV_SYSTEM_CONNECTED_COMM_SERVER,
        "ms": { "s": format!("OK:connected:tcp:{}", v_addr), "ts": now_ms() }
    });
    p_handle.send_json(&v_msg);
}

/// _acceptConnection: registers the login, kicks out an older socket with the
/// same senderID, joins the group and welcomes the party.
fn accept(p_server: &Arc<Server>, p_conn: u64, p_login: Login) {
    let (v_handle, v_join_gcs) = {
        let mut v_hub = p_server.hub();
        let Some(v_client) = v_hub.client_mut(p_conn) else { return };
        v_client.login = Some(p_login.clone());
        v_client.status = Some(Status::default());
        let v_handle = v_client.handle.clone();
        v_hub.add_active_sender(p_login.sender_id.as_ref(), p_conn);
        v_hub.del_member_from_account_by_name(Some(&p_login), true);
        v_hub.add_member_to_account_group(p_conn);
        (v_handle, p_login.actor_is(CONST_ACTOR_TYPE_GCS))
    };
    // a GCS joining a group is allowed on the relay pairs of that group's units
    if v_join_gcs && p_server.relay.is_enabled() {
        crate::s2s::udp_relay_client::on_gcs_joined(p_server, p_conn);
    }
    send_ok_message(&v_handle);
}

/// acceptLocalConnection: local_server_enabled — no AUTH, the party's own
/// senderID and actor type, full permission.
fn accept_local_connection(p_server: &Arc<Server>, p_conn: u64, p_params: &Map<String, Value>) {
    let v_sender = p_params.get(CONST_CS_SENDER_ID).cloned();
    let v_account = match p_server.cfg.get("local_server_account_id") {
        None | Some(Value::Null) => json!("1"),
        Some(v) => v.clone(),
    };
    let v_login = Login {
        sender_id: v_sender.clone(),
        account_id: Some(v_account),
        group_id: Some(json!("1")),
        request_id: v_sender,
        actor_type: Some(match p_params.get("at") {
            None | Some(Value::Null) => json!("a"),
            Some(v) => v.clone(),
        }),
        prm: Some(json!(0xffff_ffffu64)),
        creation_date: now_ms(),
    };
    accept(p_server, p_conn, v_login);
}

/// acceptConnection: a party AUTH announced (temp key in the waiting list).
fn accept_connection(p_server: &Arc<Server>, p_conn: u64, p_key: &str, p_params: &Map<String, Value>) {
    let v_waiting = p_server.hub().waiting.get(p_key).cloned();
    let Some(v_login) = v_waiting.filter(|v| !v.is_null()) else {
        if let Some(c) = p_server.hub().client(p_conn) {
            c.handle.close();
        }
        return;
    };
    let v_get = |k: &str| crate::util::field(&v_login, k).cloned();
    let v_onb = Login {
        sender_id: p_params.get(CONST_CS_SENDER_ID).cloned(),
        account_id: v_get(CONST_CS_ACCOUNT_ID),
        group_id: v_get(CONST_CS_GROUP_ID),
        request_id: v_get(CONST_CS_REQUEST_ID),
        actor_type: v_get(CONST_ACTOR_TYPE),
        prm: v_get(CONST_PERMISSION2),
        creation_date: now_ms(),
    };
    p_server.hub().waiting.remove(p_key);
    accept(p_server, p_conn, v_onb);
    if p_server.uses_auth() {
        // give feedback to AUTH server
        p_server.update_auth_server();
    } else {
        println!("TEMP DEBUG EXCEPTION ... Ex:{{}}");
    }
}

/// fn_completeConnection: validates the credentials (from the query string
/// or the de_auth frame) and accepts or closes the socket.
/// Err is a thrown validation error (the frame path answers 'validation error').
fn complete_connection(p_server: &Arc<Server>, p_conn: u64, p_params: &Map<String, Value>) -> Result<(), ()> {
    let v_handle = match p_server.hub().client(p_conn) {
        Some(c) => c.handle.clone(),
        None => return Ok(()),
    };
    if p_server.cfg.is_true("local_server_enabled") {
        // fn_validateKeyLocal: params.f.toString() throws without f
        match p_params.get(CONST_CS_LOGIN_TEMP_KEY) {
            None | Some(Value::Null) => return Err(()),
            Some(_) => {}
        }
        accept_local_connection(p_server, p_conn, p_params);
        return Ok(());
    }
    // fn_validateKey
    let v_key = p_params.get(CONST_CS_LOGIN_TEMP_KEY).map(|f| js_key(Some(f)));
    if let Some(k) = &v_key {
        if !is_alphanumeric(k) || k.chars().count() > PARAM_LENGTH {
            v_handle.close();
            p_server.log_warn(
                "Party tried to login using bad credentials",
                Some("fn_validateKey"),
                Some(&Value::Object(p_params.clone())),
            );
            v_handle.close();
            return Ok(());
        }
    }
    let v_key = v_key.unwrap_or_else(|| "undefined".into());
    let v_exists = p_server.hub().waiting.get(&v_key).map(|v| !v.is_null()).unwrap_or(false);
    if !v_exists {
        // unauthorized login, or the same key used twice
        v_handle.close();
        p_server.log_warn("Party failed to login", Some("fn_validateKey"), Some(&Value::Object(p_params.clone())));
        if let Some(c) = p_server.hub().client_mut(p_conn) {
            c.login = None;
        }
        v_handle.close();
        return Ok(());
    }
    p_server.log_info("Party successfully login", Some("fn_validateKey"), Some(&Value::Object(p_params.clone())));
    accept_connection(p_server, p_conn, &v_key, p_params);
    Ok(())
}

fn send_auth_ack(p_handle: &ConnHandle, p_ok: bool, p_reason: Option<&str>) {
    let mut v_msg = Map::new();
    v_msg.insert("ty".into(), json!(CONST_WS_MSG_ROUTING_SYSTEM));
    v_msg.insert("mt".into(), json!("de_auth_ack"));
    v_msg.insert("r".into(), json!(if p_ok { "ok" } else { "fail" }));
    if let (false, Some(r)) = (p_ok, p_reason) {
        v_msg.insert("em".into(), json!(r));
    }
    p_handle.send_json(&Value::Object(v_msg));
}

/// The first message of a socket that sent no query-string credentials:
/// it must be `{ty:'s', mt:'de_auth', f, s, at}`.
fn on_auth_frame(p_server: &Arc<Server>, p_conn: u64, p_handle: &ConnHandle, p_data: &[u8]) {
    let Some(v_parsed) = js_parse_bytes(p_data) else {
        send_auth_ack(p_handle, false, Some("bad auth frame format"));
        p_server.log_warn("Party sent invalid auth frame (not JSON)", Some("fn_onConnect_Handler"), None);
        p_handle.close();
        return;
    };
    let fn_is = |k: &str, v: &str| crate::util::field(&v_parsed, k).and_then(Value::as_str) == Some(v);
    if v_parsed.is_null() || !fn_is("ty", CONST_WS_MSG_ROUTING_SYSTEM) || !fn_is("mt", "de_auth") {
        send_auth_ack(p_handle, false, Some("expected de_auth frame"));
        p_server.log_warn("Party first message was not a de_auth frame", Some("fn_onConnect_Handler"), Some(&v_parsed));
        p_handle.close();
        return;
    }
    // the frame's fields under the query-string parameter names
    let mut v_params = Map::new();
    for (v_from, v_to) in [("f", CONST_CS_LOGIN_TEMP_KEY), ("s", CONST_CS_SENDER_ID), ("at", "at")] {
        if let Some(v) = crate::util::field(&v_parsed, v_from).filter(|v| !v.is_null()) {
            v_params.insert(v_to.into(), v.clone());
        }
    }
    if complete_connection(p_server, p_conn, &v_params).is_err() {
        send_auth_ack(p_handle, false, Some("validation error"));
        p_server.log_error("Auth frame validation error", Some("fn_onConnect_Handler"), None);
        p_handle.close();
        return;
    }
    let v_accepted = p_server.hub().client(p_conn).map(|c| c.login.is_some()).unwrap_or(false);
    if v_accepted {
        send_auth_ack(p_handle, true, None);
    } else {
        send_auth_ack(p_handle, false, Some("auth rejected"));
        p_handle.close();
    }
}

/// One writer's batch limits: at most this many frames or this many bytes
/// leave in a single flush. Without a bound a busy sender could keep one
/// flush open forever.
const WRITER_BATCH_FRAMES: usize = 64;
const WRITER_BATCH_BYTES: usize = 64 * 1024;

/// How many flushes the party writers have done (test instrumentation).
pub static WRITER_FLUSHES: AtomicU64 = AtomicU64::new(0);

/// The socket writer (one per party connection): takes queued frames,
/// feeds everything already waiting — up to the batch limits — and flushes
/// once. An empty queue flushes immediately, so a lone message pays no
/// added latency and no timer is involved. p_buffered drops only after a
/// flush succeeded, so outbound_guard's bufferedAmount stays accurate.
/// Frames leave in order; a Close frame flushes what came before it, ends
/// the writer, and anything queued after it is dropped — as before.
pub async fn conn_writer<S>(
    mut p_rx: UnboundedReceiver<Out>,
    mut p_sink: SplitSink<WebSocketStream<S>, Message>,
    p_buffered: Arc<AtomicUsize>,
) -> SplitSink<WebSocketStream<S>, Message>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut v_batch: Vec<Out> = Vec::with_capacity(16);
    while let Some(v_frame) = p_rx.recv().await {
        // the queued batch: this frame plus whatever is already waiting
        v_batch.clear();
        let mut v_bytes = v_frame.len();
        let mut v_close = matches!(v_frame, Out::Close);
        v_batch.push(v_frame);
        while !v_close && v_batch.len() < WRITER_BATCH_FRAMES && v_bytes < WRITER_BATCH_BYTES {
            match p_rx.try_recv() {
                Ok(v_frame) => {
                    v_bytes += v_frame.len();
                    v_close = matches!(v_frame, Out::Close);
                    v_batch.push(v_frame);
                }
                Err(_) => break,
            }
        }
        let mut v_ok = true;
        for v_frame in v_batch.drain(..) {
            let v_msg = match v_frame {
                Out::Text(t) => Message::Text(t),
                Out::Binary(b) => Message::Binary(b),
                Out::Close => Message::Close(None),
            };
            if p_sink.feed(v_msg).await.is_err() {
                v_ok = false;
                break;
            }
        }
        if v_ok {
            v_ok = p_sink.flush().await.is_ok();
        }
        if v_ok {
            p_buffered.fetch_sub(v_bytes.min(p_buffered.load(Ordering::SeqCst)), Ordering::SeqCst);
            WRITER_FLUSHES.fetch_add(1, Ordering::Relaxed);
        }
        if !v_ok || v_close {
            break;
        }
    }
    p_sink
}

/// fn_onWsClose: cleans up after a socket — unless it was kicked out by a
/// newer login with the same senderID, whose state must not be torn down.
pub fn on_close(p_server: &Arc<Server>, p_conn: u64) {
    let Some(v_client) = p_server.hub().clients.remove(&p_conn) else { return };
    p_server.plugins.connection_closed(p_conn);
    if v_client.terminated {
        return;
    }
    let v_unit_name = v_client.name.clone();
    let v_login = v_client.login.clone();
    p_server.hub().del_member_from_account_by_name(v_login.as_ref(), true);
    // a relay-hosted pair is not closed: the relay idles it out, so a quick
    // reconnect keeps its ports
    if p_server.relay.is_enabled() {
        crate::s2s::udp_relay_client::on_unit_disconnected(p_server, v_login.as_ref(), v_unit_name.as_ref());
    }
    if let Some(l) = &v_login {
        p_server.hub().delete_active_sender(l.sender_id.as_ref());
        let mut v_d = Map::new();
        if let Some(s) = &l.sender_id {
            v_d.insert(CONST_CS_SENDER_ID.into(), s.clone());
        }
        let v_logout = json!({ "c": CONST_CS_CMD_LOGOUT_REQUEST, "d": Value::Object(v_d) });
        p_server.auth.send_message(js_stringify(&v_logout));
        if p_server.uses_auth() {
            p_server.update_auth_server();
        }
    }
}

// ----------------------------------------------------------------------
// HTTP / WebSocket plumbing
// ----------------------------------------------------------------------

/// A parsed HTTP request head.
pub struct RequestHead {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl RequestHead {
    /// A header value; repeated headers are joined with ", " (Node).
    pub fn header(&self, p_name: &str) -> Option<String> {
        let v: Vec<&str> =
            self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(p_name)).map(|(_, v)| v.as_str()).collect();
        (!v.is_empty()).then(|| v.join(", "))
    }
}

/// Reads the request head; returns it and any bytes read past it.
pub async fn read_head<S: AsyncRead + Unpin>(p_stream: &mut S) -> Result<(RequestHead, Vec<u8>), u16> {
    let mut v_buf: Vec<u8> = Vec::with_capacity(1024);
    let mut v_chunk = [0u8; 4096];
    loop {
        let n = p_stream.read(&mut v_chunk).await.map_err(|_| 0u16)?;
        if n == 0 {
            return Err(0);
        }
        v_buf.extend_from_slice(&v_chunk[..n]);
        let mut v_headers = [httparse::EMPTY_HEADER; 100];
        let mut v_req = httparse::Request::new(&mut v_headers);
        match v_req.parse(&v_buf) {
            Ok(httparse::Status::Complete(v_len)) => {
                let v_head = RequestHead {
                    method: v_req.method.unwrap_or("").to_string(),
                    url: v_req.path.unwrap_or("/").to_string(),
                    headers: v_req
                        .headers
                        .iter()
                        .map(|h| (h.name.to_string(), String::from_utf8_lossy(h.value).into_owned()))
                        .collect(),
                };
                return Ok((v_head, v_buf[v_len..].to_vec()));
            }
            Ok(httparse::Status::Partial) => {
                if v_buf.len() > MAX_HEADER_BYTES {
                    return Err(431);
                }
            }
            Err(httparse::Error::TooManyHeaders) => return Err(431),
            Err(_) => return Err(400),
        }
    }
}

fn http_date() -> String {
    let v_ms = now_ms();
    let (y, mo, d, h, mi, s, _) = crate::util::civil(v_ms);
    let v_days = v_ms.div_euclid(86_400_000);
    let v_wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(v_days.rem_euclid(7)) as usize];
    let v_mon = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(mo - 1) as usize];
    format!("{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT", v_wd, d, v_mon, y, h, mi, s)
}

fn escape_html(p_text: &str) -> String {
    p_text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// encodeurl: percent-encodes what may not appear in a URL (keeping valid
/// %XX escapes).
fn encode_url(p_text: &str) -> String {
    let v_bytes = p_text.as_bytes();
    let mut v_out = String::with_capacity(p_text.len());
    let mut i = 0;
    while i < v_bytes.len() {
        let b = v_bytes[i];
        let v_allowed = matches!(b, 0x21 | 0x23..=0x3B | 0x3D | 0x3F..=0x5F | 0x61..=0x7A | 0x7C | 0x7E);
        let v_escape = b == b'%'
            && i + 2 < v_bytes.len()
            && v_bytes[i + 1].is_ascii_hexdigit()
            && v_bytes[i + 2].is_ascii_hexdigit();
        if v_allowed && (b != b'%' || v_escape) {
            v_out.push(b as char);
        } else {
            v_out.push_str(&format!("%{:02X}", b));
        }
        i += 1;
    }
    v_out
}

/// Express's default 404 (finalhandler) for a request it has no route for;
/// p_keep_alive answers like Node's keep-alive (5 s) instead of closing.
pub fn express_404(p_method: &str, p_url: &str, p_keep_alive: bool) -> Vec<u8> {
    let v_path = p_url.split(['?', '#']).next().unwrap_or("");
    let v_body = format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>Error</title>\n</head>\n<body>\n<pre>Cannot {} {}</pre>\n</body>\n</html>\n",
        escape_html(p_method),
        escape_html(&encode_url(v_path))
    );
    format!(
        "HTTP/1.1 404 Not Found\r\nContent-Security-Policy: default-src 'none'\r\nX-Content-Type-Options: nosniff\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nDate: {}\r\n{}\r\n{}",
        v_body.len(),
        http_date(),
        if p_keep_alive { "Connection: keep-alive\r\nKeep-Alive: timeout=5\r\n" } else { "Connection: close\r\n" },
        v_body
    )
    .into_bytes()
}

/// ws's abortHandshake response.
fn http_error(p_code: u16, p_extra: &str) -> Vec<u8> {
    let v_reason = match p_code {
        400 => "Bad Request",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
        _ => "Bad Request",
    };
    format!(
        "HTTP/1.1 {} {}\r\nConnection: close\r\nContent-Type: text/html\r\nContent-Length: {}\r\n{}\r\n{}",
        p_code,
        v_reason,
        v_reason.len(),
        p_extra,
        v_reason
    )
    .into_bytes()
}

/// Validates a WebSocket upgrade like ws's handleUpgrade; returns the 101
/// response or the error response.
pub fn upgrade_response(p_head: &RequestHead) -> Result<Vec<u8>, Vec<u8>> {
    if p_head.method != "GET" {
        return Err(http_error(405, ""));
    }
    let v_key = p_head.header("sec-websocket-key").unwrap_or_default();
    let v_key_ok = v_key.len() == 24
        && v_key.ends_with("==")
        && v_key[..22].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/');
    if !v_key_ok {
        return Err(http_error(400, ""));
    }
    let v_version = p_head.header("sec-websocket-version").unwrap_or_default();
    if v_version.trim() != "13" && v_version.trim() != "8" {
        return Err(http_error(400, "Sec-WebSocket-Version: 13, 8\r\n"));
    }
    let mut v_resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n",
        derive_accept_key(v_key.as_bytes())
    );
    if let Some(v_protocols) = p_head.header("sec-websocket-protocol") {
        if let Some(v_first) = v_protocols.split(',').map(str::trim).find(|p| !p.is_empty()) {
            v_resp.push_str(&format!("Sec-WebSocket-Protocol: {}\r\n", v_first));
        }
    }
    v_resp.push_str("\r\n");
    Ok(v_resp.into_bytes())
}

/// True for a WebSocket upgrade request (`Upgrade: websocket`).
pub fn is_upgrade(p_head: &RequestHead) -> bool {
    p_head.header("upgrade").map(|u| u.trim().eq_ignore_ascii_case("websocket")).unwrap_or(false)
}

/// Serves one TCP (TLS) connection: HTTP head, then a party's WebSocket.
pub async fn serve_connection<S>(p_server: Arc<Server>, mut p_stream: S, p_remote: SocketAddr)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // plain HTTP requests get Express's 404; the connection stays open
    // (keep-alive, 5 s) for more requests or an upgrade
    let mut v_first = true;
    let (v_head, v_rest) = loop {
        let v_read = if v_first {
            read_head(&mut p_stream).await
        } else {
            match tokio::time::timeout(Duration::from_secs(5), read_head(&mut p_stream)).await {
                Ok(r) => r,
                Err(_) => Err(0),
            }
        };
        v_first = false;
        let (v_head, v_rest) = match v_read {
            Ok(h) => h,
            Err(0) => return,
            Err(code) => {
                let _ = p_stream.write_all(&http_error(code, "")).await;
                return;
            }
        };
        if is_upgrade(&v_head) {
            break (v_head, v_rest);
        }
        // a request body is never read: close instead of parsing past it
        let v_has_body = v_head.header("content-length").map(|l| l.trim() != "0").unwrap_or(false)
            || v_head.header("transfer-encoding").is_some();
        let v_close = v_has_body
            || !v_rest.is_empty()
            || v_head.header("connection").map(|c| c.eq_ignore_ascii_case("close")).unwrap_or(false);
        if p_stream.write_all(&express_404(&v_head.method, &v_head.url, !v_close)).await.is_err() || v_close {
            let _ = p_stream.shutdown().await;
            return;
        }
    };
    match upgrade_response(&v_head) {
        Ok(v_resp) => {
            if p_stream.write_all(&v_resp).await.is_err() {
                return;
            }
        }
        Err(v_resp) => {
            let _ = p_stream.write_all(&v_resp).await;
            let _ = p_stream.shutdown().await;
            return;
        }
    }
    let v_ws =
        WebSocketStream::from_partially_read(p_stream, v_rest, Role::Server, Some(crate::net::ws_config())).await;
    println!("WebSocket client connected from {}", remote_address(&p_remote));
    run_party(p_server, v_ws, v_head, p_remote).await;
}

/// fn_onConnect_Handler and the socket's lifetime.
async fn run_party<S>(p_server: Arc<Server>, p_ws: WebSocketStream<S>, p_head: RequestHead, p_remote: SocketAddr)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (v_handle, v_out) = ConnHandle::new(Some(p_remote));
    let v_conn = v_handle.id;
    let v_ip = client_ip(
        p_server.cfg.is_true("trust_proxy_header"),
        p_head.header("x-forwarded-for").as_deref(),
        Some(p_remote),
    );
    p_server.hub().clients.insert(v_conn, Client::new(v_handle.clone(), v_ip));

    let (v_sink, mut v_source) = p_ws.split();
    // writer: frames leave in order; bufferedAmount drops as they are written
    let v_buffered = v_handle.buffered_counter();
    let v_writer = tokio::spawn(conn_writer(v_out, v_sink, v_buffered));

    let v_params = header_params(&p_head.url);
    p_server.log_info("WS Created from Party", Some("fn_onConnect_Handler"), Some(&Value::Object(v_params.clone())));
    let v_allow_old_compat = !p_server.cfg.is_false("ws_auth_no_frame_old_compatibility");
    let v_has_query_creds = v_params.contains_key(CONST_CS_LOGIN_TEMP_KEY);
    let mut v_auth_deadline: Option<Instant> = None;
    if v_has_query_creds && !v_allow_old_compat {
        println!(
            "{}[WARN] legacy query-string credentials rejected (ws_auth_no_frame_old_compatibility is false){}",
            c::B_FG_YELLOW,
            c::RESET
        );
        p_server.log_warn(
            "legacy query-string credentials rejected (ws_auth_no_frame_old_compatibility is false)",
            Some("fn_onConnect_Handler"),
            Some(&Value::Object(v_params.clone())),
        );
        v_handle.close();
    } else if v_has_query_creds {
        println!(
            "{}[WARN] ws_auth_no_frame_old_compatibility is enabled but client used legacy query-string credentials{}",
            c::B_FG_YELLOW,
            c::RESET
        );
        p_server.log_warn(
            "ws_auth_no_frame_old_compatibility is enabled but client used legacy query-string credentials",
            Some("fn_onConnect_Handler"),
            Some(&Value::Object(v_params.clone())),
        );
        let _ = complete_connection(&p_server, v_conn, &v_params);
    } else {
        // wait for the de_auth frame as the first message
        v_auth_deadline = Some(Instant::now() + AUTH_FRAME_TIMEOUT);
    }

    let mut v_close_deadline: Option<Instant> = None;
    let v_far = Instant::now() + Duration::from_secs(86400 * 365);
    loop {
        if v_close_deadline.is_none() && v_handle.ready_state() == crate::hub::READY_CLOSING {
            v_close_deadline = Some(Instant::now() + CLOSE_TIMEOUT);
        }
        tokio::select! {
            v_in = v_source.next() => {
                let Some(Ok(v_msg)) = v_in else { break };
                let (v_data, v_binary) = match &v_msg {
                    Message::Text(t) => (t.as_bytes(), false),
                    Message::Binary(b) => (&b[..], true),
                    Message::Close(_) => {
                        // the peer closes: no more sends (ws readyState CLOSING)
                        v_handle.mark_closing();
                        continue;
                    }
                    _ => continue,
                };
                // routing first (it ignores a socket that is not logged in) ...
                crate::chat::routing::parse_message(&p_server, v_conn, v_data, v_binary);
                // ... then the one-time de_auth interceptor
                if v_auth_deadline.take().is_some() {
                    on_auth_frame(&p_server, v_conn, &v_handle, v_data);
                }
            },
            _ = v_handle.killed() => break,
            _ = sleep_until(v_auth_deadline.unwrap_or(v_far)), if v_auth_deadline.is_some() => {
                v_auth_deadline = None;
                send_auth_ack(&v_handle, false, Some("auth frame timeout"));
                p_server.log_warn("Party auth frame timeout", Some("fn_onConnect_Handler"), None);
                v_handle.close();
            },
            _ = sleep_until(v_close_deadline.unwrap_or(v_far)), if v_close_deadline.is_some() => break,
        }
    }
    v_handle.mark_closed();
    v_writer.abort();
    on_close(&p_server, v_conn);
}

/// fn_startChatServer: the HTTPS server for parties.
pub async fn start_chat_server(p_server: &Arc<Server>) -> Result<std::net::SocketAddr, String> {
    let fn_path = |k: &str| -> Result<std::path::PathBuf, String> {
        let v_file = p_server
            .cfg
            .string(k)
            .ok_or_else(|| format!("TypeError: Cannot read properties of undefined (reading 'toString') [{}]", k))?;
        Ok(if std::path::Path::new(&v_file).is_absolute() { v_file.into() } else { p_server.cfg.resolve(&v_file) })
    };
    let v_key = fn_path("ssl_key_file")?;
    let v_cert = fn_path("ssl_cert_file")?;
    let mut v_errors = Vec::new();
    if std::fs::metadata(&v_key).map(|m| !m.is_file()).unwrap_or(true) {
        v_errors.push(format!(
            "{}FATAL ERROR: Cannot read SSL key file: {}{}{}",
            c::ERROR,
            c::ERROR,
            v_key.display(),
            c::RESET
        ));
    }
    if std::fs::metadata(&v_cert).map(|m| !m.is_file()).unwrap_or(true) {
        v_errors.push(format!(
            "{}FATAL ERROR: Cannot read SSL cert file: {}{}{}",
            c::ERROR,
            c::ERROR,
            v_cert.display(),
            c::RESET
        ));
    }
    if !v_errors.is_empty() {
        return Err(v_errors.join("\n"));
    }
    let v_acceptor = crate::net::tls_acceptor(&v_key, &v_cert)?;
    if p_server.cfg.is_true("ws_compression") {
        println!(
            "{}[WARN] ws_compression: permessage-deflate is not supported by this server; connections are uncompressed{}",
            c::B_FG_YELLOW,
            c::RESET
        );
    }
    let v_ip = crate::net::cfg_str(p_server.cfg.get("server_ip"));
    let v_port = crate::net::cfg_str(p_server.server_port_value().as_ref());
    let v_listener = bind_listener(&v_ip, &v_port).await?;
    let v_addr = v_listener.local_addr().map_err(|e| e.to_string())?;
    println!("HTTPS server started on {}:{}", v_ip, v_port);
    let v_server = p_server.clone();
    tokio::spawn(async move {
        loop {
            let Ok((v_tcp, v_remote)) = v_listener.accept().await else {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            };
            let _ = v_tcp.set_nodelay(true);
            let v_server = v_server.clone();
            let v_acceptor = v_acceptor.clone();
            tokio::spawn(async move {
                if let Ok(v_tls) = v_acceptor.accept(v_tcp).await {
                    serve_connection(v_server, v_tls, v_remote).await;
                }
            });
        }
    });
    Ok(v_addr)
}

/// listen(port, host) like Node: "::" is dual-stack, a missing host is all
/// interfaces.
pub async fn bind_listener(p_ip: &str, p_port: &str) -> Result<TcpListener, String> {
    let v_port: u16 = if p_port == "undefined" || p_port.is_empty() {
        0
    } else {
        p_port.trim().parse().map_err(|_| {
            format!("RangeError [ERR_SOCKET_BAD_PORT]: options.port should be >= 0 and < 65536. Received {}.", p_port)
        })?
    };
    let v_host = if p_ip == "undefined" || p_ip.is_empty() { "::" } else { p_ip };
    let v_host = v_host.trim_start_matches('[').trim_end_matches(']');
    let v_addrs: Vec<SocketAddr> =
        tokio::net::lookup_host((v_host, v_port)).await.map_err(|e| e.to_string())?.collect();
    let mut v_last = String::from("no address");
    for v_addr in v_addrs {
        match TcpListener::bind(v_addr).await {
            Ok(l) => return Ok(l),
            Err(e) => v_last = format!("listen {}: {}", v_addr, e),
        }
    }
    Err(v_last)
}
