//! Client of the AUTH server (js_comm_server_manager_client.js).
//!
//! Connects to `wss://s2s_ws_target_ip:s2s_ws_target_port`, answers AUTH's
//! S2S challenge with the Ed25519 key and announces this server (INFO card)
//! on open and after the handshake. Messages sent while the link is down are
//! queued (up to 1000) and flushed on (re)connect; a closed link drops the
//! queue and reconnects after 2 s. The link is never closed on purpose.

use futures_util::StreamExt;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::{Message, Utf8Bytes};

use crate::colors as c;
use crate::net::{cfg_str, frame_bytes, split_with_writer, ws_connect, TlsVerify};
use crate::s2s::s2s_auth::{parse_envelope, S2S_AUTH_CHALLENGE};
use crate::server::Server;

const RETRY_TIME: Duration = Duration::from_millis(2000);
const MAX_QUEUE: usize = 1000;

#[derive(Default)]
struct AuthState {
    tx: Option<UnboundedSender<Message>>,
    queue: VecDeque<String>,
}

pub struct AuthClient {
    m_state: Mutex<AuthState>,
}

impl Default for AuthClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthClient {
    pub fn new() -> AuthClient {
        AuthClient { m_state: Mutex::new(AuthState::default()) }
    }

    /// fn_sendMessage: sent now when the link is open, else queued.
    pub fn send_message(&self, p_message: String) {
        let mut st = self.m_state.lock().unwrap();
        if let Some(tx) = &st.tx {
            if tx.send(Message::Text(Utf8Bytes::from(p_message.clone()))).is_ok() {
                return;
            }
        }
        if st.queue.len() < MAX_QUEUE {
            st.queue.push_back(p_message);
        }
    }

    /// Messages waiting for the link (tests).
    pub fn queued(&self) -> Vec<String> {
        self.m_state.lock().unwrap().queue.iter().cloned().collect()
    }

    fn flush_queue(&self) {
        let mut st = self.m_state.lock().unwrap();
        let AuthState { tx, queue } = &mut *st;
        if let Some(tx) = tx {
            while let Some(m) = queue.pop_front() {
                let _ = tx.send(Message::Text(Utf8Bytes::from(m)));
            }
        }
    }

    /// fn_startServer: connects (and keeps reconnecting) to AUTH.
    pub fn start(p_server: &Arc<Server>) {
        let v_host = cfg_str(p_server.cfg.get("s2s_ws_target_ip"));
        let v_port = cfg_str(p_server.cfg.get("s2s_ws_target_port"));
        println!(
            "{}[OK] Comm Server Manager Client connecting to wss://{}:{} to reach AndruavAuth{}",
            c::B_SUCCESS,
            v_host,
            v_port,
            c::RESET
        );
        let v_server = p_server.clone();
        tokio::spawn(async move {
            loop {
                v_server.auth.run_once(&v_server, &v_host, &v_port).await;
                tokio::time::sleep(RETRY_TIME).await;
            }
        });
    }

    async fn run_once(&self, p_server: &Arc<Server>, p_host: &str, p_port: &str) {
        let v_verify = if p_server.cfg.is_true("allow_fake_SSL") {
            TlsVerify::None
        } else {
            TlsVerify::Roots { extra_ca: p_server.env.extra_ca_certs.clone() }
        };
        let v_ws = match ws_connect(p_host, p_port, Some(&v_verify)).await {
            Ok(w) => w,
            Err(e) => {
                println!("{}ATTENTION!! Connection error with AuthServer: Error: {}{}", c::ERROR, e, c::RESET);
                self.on_close();
                return;
            }
        };
        let (v_tx, mut v_rx) = split_with_writer(v_ws);
        self.m_state.lock().unwrap().tx = Some(v_tx.clone());
        println!("{}[OK] Connection established with AuthServer{}", c::B_SUCCESS, c::RESET);
        // announce ourselves immediately; after a challenge we announce again
        p_server.update_auth_server();
        self.flush_queue();

        while let Some(v_msg) = v_rx.next().await {
            let Ok(v_msg) = v_msg else { break };
            let Some(v_data) = frame_bytes(&v_msg) else { continue };
            if let Some(v_env) = parse_envelope(v_data) {
                if v_env.get("s2s_auth").and_then(|v| v.as_str()) == Some(S2S_AUTH_CHALLENGE) {
                    match p_server.s2s.build_response(v_env.get("nonce")) {
                        Ok(v_resp) => {
                            let _ = v_tx.send(Message::Text(Utf8Bytes::from(v_resp)));
                            p_server.update_auth_server();
                            self.flush_queue();
                        }
                        Err(e) => {
                            println!(
                                "{}ATTENTION!! S2S handshake failed (check s2s_my_private_key): Error: {}{}",
                                c::ERROR,
                                e,
                                c::RESET
                            );
                            let _ = v_tx.send(Message::Close(None));
                        }
                    }
                }
                continue;
            }
            crate::comm_server::on_auth_message(p_server, v_data);
        }
        self.on_close();
    }

    fn on_close(&self) {
        println!("{}ATTENTION!! Connection closed with AuthServer{}", c::ERROR, c::RESET);
        let mut st = self.m_state.lock().unwrap();
        st.tx = None;
        st.queue.clear();
    }
}
