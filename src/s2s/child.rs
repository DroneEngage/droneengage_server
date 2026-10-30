//! Child side of the mesh (js_child_comm_server.js): a persistent wss link
//! to the super server (s2s_relay_to_super_server_ip/port, certificates not
//! verified). It answers the super server's challenge with the Ed25519 key;
//! everything else it receives is routed as an external message. Reconnects
//! 10 s after a close.

use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::{Bytes, Message, Utf8Bytes};

use crate::chat::relay::Source;
use crate::hub::{Out, Payload};
use crate::net::{cfg_str, split_with_writer, ws_connect, TlsVerify};
use crate::s2s::s2s_auth::{parse_envelope, S2S_AUTH_CHALLENGE};
use crate::server::Server;

const RECONNECT: Duration = Duration::from_millis(10000);

#[derive(Default)]
pub struct ChildLink {
    m_tx: Mutex<Option<UnboundedSender<Message>>>,
}

impl ChildLink {
    pub fn new() -> ChildLink {
        ChildLink::default()
    }

    /// Test hook: the link to the parent is p_tx.
    #[doc(hidden)]
    pub fn set_link_for_test(&self, p_tx: Option<UnboundedSender<Message>>) {
        *self.m_tx.lock().unwrap() = p_tx;
    }

    pub fn is_connected(&self) -> bool {
        self.m_tx.lock().unwrap().is_some()
    }

    /// Sends to the parent, or logs that it is not connected.
    pub fn forward_message(&self, p_msg: &Payload, p_binary: bool) {
        let v_tx = self.m_tx.lock().unwrap().clone();
        let v_sent = v_tx
            .map(|tx| {
                let v_msg = match p_msg.with_binary(p_binary) {
                    Out::Text(t) => Message::Text(t),
                    Out::Binary(b) => Message::Binary(b),
                    Out::Close => Message::Binary(Bytes::new()),
                };
                tx.send(v_msg).is_ok()
            })
            .unwrap_or(false);
        if !v_sent {
            eprintln!("Parent WebSocket is not connected. Cannot forward message.");
        }
    }

    /// connectToParent, forever.
    pub fn start(p_server: &Arc<Server>) {
        let v_host = cfg_str(p_server.cfg.get("s2s_relay_to_super_server_ip"));
        let v_port = cfg_str(p_server.cfg.get("s2s_relay_to_super_server_port"));
        let v_server = p_server.clone();
        tokio::spawn(async move {
            loop {
                v_server.child.run_once(&v_server, &v_host, &v_port).await;
                tokio::time::sleep(RECONNECT).await;
            }
        });
    }

    async fn run_once(&self, p_server: &Arc<Server>, p_host: &str, p_port: &str) {
        let v_url = format!("wss://{}:{}", p_host, p_port);
        let v_ws = match ws_connect(p_host, p_port, Some(&TlsVerify::None)).await {
            Ok(w) => w,
            Err(e) => {
                eprintln!("WebSocket error from parent: Error: {}", e);
                println!("Child disconnected from parent server: 1006 - ");
                return;
            }
        };
        let (v_tx, mut v_rx) = split_with_writer(v_ws);
        *self.m_tx.lock().unwrap() = Some(v_tx.clone());
        println!("Child connected to parent server at {}.", v_url);
        let mut v_close = (1006u16, String::new());
        while let Some(v_msg) = v_rx.next().await {
            let Ok(v_msg) = v_msg else { break };
            let v_data: &[u8] = match &v_msg {
                Message::Text(t) => t.as_bytes(),
                Message::Binary(b) => b,
                Message::Close(f) => {
                    v_close =
                        f.as_ref().map(|f| (u16::from(f.code), f.reason.to_string())).unwrap_or((1005, String::new()));
                    continue;
                }
                _ => continue,
            };
            if let Some(v_env) = parse_envelope(v_data) {
                if v_env.get("s2s_auth").and_then(|v| v.as_str()) == Some(S2S_AUTH_CHALLENGE) {
                    match p_server.s2s.build_response(v_env.get("nonce")) {
                        Ok(r) => {
                            let _ = v_tx.send(Message::Text(Utf8Bytes::from(r)));
                        }
                        Err(e) => eprintln!("Error parsing message from parent: Error: {}", e),
                    }
                }
                continue;
            }
            if p_server.debug {
                println!("CHILD RX: {}", String::from_utf8_lossy(v_data));
            }
            crate::chat::routing::parse_external_message(p_server, v_data, true, Some(Source::Parent));
        }
        *self.m_tx.lock().unwrap() = None;
        println!("Child disconnected from parent server: {} - {}", v_close.0, v_close.1);
    }
}
