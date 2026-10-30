//! Super (parent) server side of the mesh (js_parent_comm_server.js):
//! accepts child comm servers on s2s_super_server_ip:s2s_super_server_port
//! (wss). A child must answer the Ed25519 challenge
//! (s2s_trusted_server_keys) within 8 s before any of its traffic is
//! trusted; afterwards its messages are routed as external messages and
//! forwarded to the other children.

use futures_util::{SinkExt, StreamExt};
use indexmap::IndexMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::time::{sleep_until, Instant};
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::chat::connection::{bind_listener, is_upgrade, read_head, remote_address, upgrade_response};
use crate::chat::relay::Source;
use crate::colors as c;
use crate::hub::{ConnHandle, Out, Payload};
use crate::net::cfg_str;
use crate::s2s::s2s_auth::{
    build_challenge, generate_nonce, parse_envelope, S2S_AUTH_HANDSHAKE_TIMEOUT_MS, S2S_AUTH_RESPONSE,
};
use crate::server::Server;

struct Child {
    handle: Arc<ConnHandle>,
    authed: bool,
}

#[derive(Default)]
pub struct ParentServer {
    /// "ip:port" order of connection -> child
    m_children: Mutex<IndexMap<u64, Child>>,
}

impl ParentServer {
    pub fn new() -> ParentServer {
        ParentServer::default()
    }

    /// Forwards to every authenticated child except p_exclude (the sender).
    pub fn forward_message(&self, p_msg: &Payload, p_binary: bool, p_exclude: Option<u64>) {
        let v_children = self.m_children.lock().unwrap();
        for (v_id, v_child) in v_children.iter() {
            if v_child.authed && v_child.handle.is_open() && Some(*v_id) != p_exclude {
                v_child.handle.send(p_msg.with_binary(p_binary));
            }
        }
    }

    /// Test hook: registers a child connection (authenticated or not).
    #[doc(hidden)]
    pub fn add_child_for_test(&self, p_handle: Arc<ConnHandle>, p_authed: bool) -> u64 {
        let v_id = p_handle.id;
        self.m_children.lock().unwrap().insert(v_id, Child { handle: p_handle, authed: p_authed });
        v_id
    }

    /// Connected children ("ip:port"), for diagnostics.
    pub fn child_count(&self) -> usize {
        self.m_children.lock().unwrap().len()
    }

    /// The parent server's TLS files: like the chat server (absolute, or
    /// relative to the server root), falling back to the location the
    /// JavaScript parent server resolved them from (src/comm_server/).
    fn ssl_path(p_server: &Server, p_key: &str) -> Result<PathBuf, String> {
        let v_file = p_server.cfg.string(p_key).ok_or_else(|| format!("{} is not configured", p_key))?;
        let v_path = std::path::Path::new(&v_file);
        if v_path.is_absolute() {
            return Ok(v_path.to_path_buf());
        }
        let v_root = p_server.cfg.resolve(&v_file);
        if v_root.is_file() {
            return Ok(v_root);
        }
        let v_legacy = p_server.cfg.resolve(&format!("src/comm_server/{}", v_file));
        if v_legacy.is_file() {
            return Ok(v_legacy);
        }
        Err(format!("ENOENT: no such file or directory, open '{}'", v_root.display()))
    }

    /// _startServer
    pub async fn start(p_server: &Arc<Server>) -> Result<SocketAddr, String> {
        let v_host = cfg_str(p_server.cfg.get("s2s_super_server_ip"));
        let v_port = cfg_str(p_server.cfg.get("s2s_super_server_port"));
        println!("Super Server Starting");
        println!(
            "listening on ip: {}{}{} port: {}{}{}",
            c::B_SUCCESS,
            v_host,
            c::RESET,
            c::B_SUCCESS,
            v_port,
            c::RESET
        );
        let v_acceptor = crate::net::tls_acceptor(
            &Self::ssl_path(p_server, "ssl_key_file")?,
            &Self::ssl_path(p_server, "ssl_cert_file")?,
        )?;
        let v_listener = bind_listener(&v_host, &v_port).await?;
        let v_addr = v_listener.local_addr().map_err(|e| e.to_string())?;
        println!("{}[OK] Super Server has Started at {}:{}{}", c::B_SUCCESS, v_host, v_port, c::RESET);
        let v_server = p_server.clone();
        tokio::spawn(async move {
            loop {
                let Ok((v_tcp, v_remote)) = v_listener.accept().await else {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let (v_server, v_acceptor) = (v_server.clone(), v_acceptor.clone());
                tokio::spawn(async move {
                    if let Ok(v_tls) = v_acceptor.accept(v_tcp).await {
                        serve_child(v_server, v_tls, v_remote).await;
                    }
                });
            }
        });
        Ok(v_addr)
    }
}

async fn serve_child<S>(p_server: Arc<Server>, mut p_stream: S, p_remote: SocketAddr)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok((v_head, v_rest)) = read_head(&mut p_stream).await else { return };
    if !is_upgrade(&v_head) {
        return;
    }
    match upgrade_response(&v_head) {
        Ok(r) => {
            if p_stream.write_all(&r).await.is_err() {
                return;
            }
        }
        Err(r) => {
            let _ = p_stream.write_all(&r).await;
            return;
        }
    }
    let v_ws =
        WebSocketStream::from_partially_read(p_stream, v_rest, Role::Server, Some(crate::net::ws_config())).await;
    let v_key = format!("{}:{}", remote_address(&p_remote), p_remote.port());
    let (v_handle, mut v_out) = ConnHandle::new(Some(p_remote));
    let v_id = v_handle.id;
    p_server.parent.m_children.lock().unwrap().insert(v_id, Child { handle: v_handle.clone(), authed: false });
    println!("New Child Communication Server Connected from {}{}{}", c::B_SUCCESS, v_key, c::RESET);

    let (mut v_sink, mut v_source) = v_ws.split();
    let v_buffered = v_handle.buffered_counter();
    let v_writer = tokio::spawn(async move {
        while let Some(v_frame) = v_out.recv().await {
            let v_len = v_frame.len();
            let v_msg = match v_frame {
                Out::Text(t) => Message::Text(t),
                Out::Binary(b) => Message::Binary(b),
                Out::Close => Message::Close(None),
            };
            let v_close = matches!(v_msg, Message::Close(_));
            let v_ok = v_sink.send(v_msg).await.is_ok();
            v_buffered.fetch_sub(
                v_len.min(v_buffered.load(std::sync::atomic::Ordering::SeqCst)),
                std::sync::atomic::Ordering::SeqCst,
            );
            if !v_ok || v_close {
                break;
            }
        }
    });

    // a child must prove ownership of its private key before it is trusted
    let v_nonce = generate_nonce();
    v_handle.send_text(build_challenge(&v_nonce, None));
    let mut v_auth_deadline = Some(Instant::now() + Duration::from_millis(S2S_AUTH_HANDSHAKE_TIMEOUT_MS));
    let v_far = Instant::now() + Duration::from_secs(86400 * 365);
    loop {
        tokio::select! {
            v_in = v_source.next() => {
                let Some(Ok(v_msg)) = v_in else { break };
                let v_data: &[u8] = match &v_msg {
                    Message::Text(t) => t.as_bytes(),
                    Message::Binary(b) => b,
                    Message::Close(_) => { v_handle.mark_closing(); continue; }
                    _ => continue,
                };
                let v_authed = p_server.parent.m_children.lock().unwrap().get(&v_id).map(|c| c.authed).unwrap_or(false);
                if !v_authed {
                    let v_env = parse_envelope(v_data);
                    let v_ok = v_env.as_ref().map(|e| {
                        e.get("s2s_auth").and_then(|v| v.as_str()) == Some(S2S_AUTH_RESPONSE)
                            && !crate::util::js_nullish(e.get("id"))
                            && p_server.s2s.verify(&v_nonce, e.get("sig"), e.get("id"))
                    }).unwrap_or(false);
                    if v_ok {
                        if let Some(ch) = p_server.parent.m_children.lock().unwrap().get_mut(&v_id) {
                            ch.authed = true;
                        }
                        v_auth_deadline = None;
                        println!(
                            "{}[OK] Child {} passed S2S handshake [{}]{}",
                            c::SUCCESS,
                            v_key,
                            crate::util::js_key(v_env.as_ref().and_then(|e| e.get("id"))),
                            c::RESET
                        );
                    } else {
                        println!("{}[ATTENTION!!] Child {} failed S2S handshake (bad signature or missing id){}", c::ERROR, v_key, c::RESET);
                        v_handle.terminate();
                    }
                    continue;
                }
                if p_server.debug {
                    println!("SUPER SRV RX:: {}", String::from_utf8_lossy(v_data));
                }
                // ws hands every message over as a Buffer: always binary
                crate::chat::routing::parse_external_message(&p_server, v_data, true, Some(Source::Child(v_id)));
            },
            _ = v_handle.killed() => break,
            _ = sleep_until(v_auth_deadline.unwrap_or(v_far)), if v_auth_deadline.is_some() => {
                v_auth_deadline = None;
                println!("{}[ATTENTION!!] Child {} failed S2S handshake (timeout){}", c::ERROR, v_key, c::RESET);
                v_handle.terminate();
            },
        }
    }
    v_handle.mark_closed();
    v_writer.abort();
    p_server.parent.m_children.lock().unwrap().shift_remove(&v_id);
    println!("Connection closed from {}", v_key);
}
