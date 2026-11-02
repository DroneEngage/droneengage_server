//! A minimal AUTH server for benchmarks: the comm server dials out to
//! `s2s_ws_target_ip:port` and AUTH announces each login here, which the
//! server parks in `hub.waiting` under a temp key the party then presents.
//! No s2s challenge is sent (the comm server only answers one), and no
//! real credentials are checked — bench only.

use droneengage_server_common::net::{frame_bytes, tls_acceptor, ws_config};
use droneengage_server_common::util::{field, js_parse_bytes};
use droneengage_server_common::ws_accept::{bind_listener, read_head, upgrade_response};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::stats::reply_key;

/// CONST_CS_CMD_LOGIN_REQUEST ("b"): AUTH -> server login request.
const CMD_LOGIN_REQUEST: &str = "b";

pub struct AuthStub {
    m_tx: mpsc::UnboundedSender<Message>,
    m_pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    m_next_req: AtomicU64,
}

impl AuthStub {
    /// Listens for the comm server's auth link; resolves once it is up.
    pub async fn listen(p_ip: &str, p_port: &str, p_key: &Path, p_cert: &Path) -> Result<Arc<AuthStub>, String> {
        let v_acceptor = tls_acceptor(p_key, p_cert)?;
        let v_listener = bind_listener(p_ip, p_port).await?;
        let (v_tx, mut v_rx) = mpsc::unbounded_channel::<Message>();
        let v_pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>> = Arc::new(Mutex::new(HashMap::new()));
        let v_stub = Arc::new(AuthStub { m_tx: v_tx, m_pending: v_pending.clone(), m_next_req: AtomicU64::new(1) });

        // writer: whatever login() queues goes to the latest link
        let v_out: Arc<Mutex<Option<tokio::sync::mpsc::UnboundedSender<Message>>>> = Arc::new(Mutex::new(None));
        let v_link_up = v_out.clone();
        {
            let v_out = v_out.clone();
            tokio::spawn(async move {
                while let Some(v_msg) = v_rx.recv().await {
                    let v_link = v_out.lock().await.clone();
                    if let Some(v_link) = v_link {
                        let _ = v_link.send(v_msg);
                    }
                }
            });
        }
        {
            let v_pending = v_pending.clone();
            tokio::spawn(async move {
                loop {
                    let Ok((v_tcp, _)) = v_listener.accept().await else {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    };
                    let _ = v_tcp.set_nodelay(true);
                    let v_acceptor = v_acceptor.clone();
                    let v_pending = v_pending.clone();
                    let v_out = v_out.clone();
                    tokio::spawn(async move {
                        let Ok(mut v_tls) = v_acceptor.accept(v_tcp).await else { return };
                        let Ok((v_head, v_rest)) = read_head(&mut v_tls).await else { return };
                        let Ok(v_resp) = upgrade_response(&v_head) else { return };
                        if v_tls.write_all(&v_resp).await.is_err() {
                            return;
                        }
                        let v_ws =
                            WebSocketStream::from_partially_read(v_tls, v_rest, Role::Server, Some(ws_config())).await;
                        let (mut v_sink, mut v_source) = v_ws.split();
                        let (v_link_tx, mut v_link_rx) = mpsc::unbounded_channel::<Message>();
                        *v_out.lock().await = Some(v_link_tx);
                        tokio::spawn(async move {
                            while let Some(m) = v_link_rx.recv().await {
                                if v_sink.send(m).await.is_err() {
                                    break;
                                }
                            }
                        });
                        while let Some(Ok(v_msg)) = v_source.next().await {
                            let Some(v_data) = frame_bytes(&v_msg) else { continue };
                            let Some(v_cmd) = js_parse_bytes(v_data) else { continue };
                            if field(&v_cmd, "c").and_then(Value::as_str) != Some(CMD_LOGIN_REQUEST) {
                                continue;
                            }
                            if let Some(v_key) = reply_key(&v_cmd) {
                                if let Some(v_wait) = v_pending.lock().await.remove(&v_key) {
                                    let _ = v_wait.send(v_cmd);
                                }
                            }
                        }
                        *v_out.lock().await = None;
                    });
                }
            });
        }
        // wait for the comm server's auth link (it retries every 2 s)
        while v_link_up.lock().await.is_none() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        println!("auth stub: comm server link up on {}:{}", p_ip, p_port);
        Ok(v_stub)
    }

    /// Announces a login and returns the temp key the party presents.
    pub async fn login(&self, p_name: &str, p_account: &str, p_group: &str, p_actor: &str) -> Result<String, String> {
        let v_req = format!("loadgen-{}", self.m_next_req.fetch_add(1, Ordering::Relaxed));
        let (v_tx, v_rx) = oneshot::channel();
        self.m_pending.lock().await.insert(v_req.clone(), v_tx);
        let v_cmd = json!({
            "c": CMD_LOGIN_REQUEST,
            "d": { "s": p_name, "a": p_account, "b": p_group, "at": p_actor, "r": v_req, "prm": 4294967295u64 }
        });
        self.m_tx
            .send(Message::Text(v_cmd.to_string().into()))
            .map_err(|_| "auth link closed".to_string())?;
        let v_reply = tokio::time::timeout(Duration::from_secs(5), v_rx)
            .await
            .map_err(|_| format!("login reply timeout for {}", p_name))?
            .map_err(|_| "login reply dropped".to_string())?;
        let v_d = v_reply.get("d").ok_or("login reply without d")?;
        if v_d.get("e").and_then(Value::as_i64) != Some(0) {
            return Err(format!("login rejected for {}: {}", p_name, v_d));
        }
        v_d.get("f").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "login reply without f".into())
    }
}
