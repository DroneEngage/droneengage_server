//! DroneEngage communication server: the real-time messaging backbone between
//! drone units and ground control stations (WebSocket), with a
//! server-to-server mesh, a link to the AUTH server, a link to the storage
//! server, and a UDP proxy / UDP relay broker for MAVLink streams.
//!
//! Ported from the Node.js server (src/server.js and src/comm_server/*); the
//! wire protocol, configuration and behaviour are unchanged.

pub mod chat;
pub mod cli;
pub mod comm_server;
pub mod config;
pub mod constants;
pub mod hub;
pub mod s2s;
pub mod server;

// shared with the other DroneEngage servers
pub use droneengage_server_common::{colors, logger, net, util};

/// Validation helpers (hlp_validation.js).
pub mod validation {
    pub use droneengage_server_common::validation::is_alphanumeric_str as is_alphanumeric;
}

use std::sync::Arc;

pub use config::Config;
pub use server::{Env, Server};

/// What start() bound.
#[derive(Debug, Clone)]
pub struct Started {
    pub chat: std::net::SocketAddr,
    pub parent: Option<std::net::SocketAddr>,
}

/// m_andruav_comm_server.fn_startServer + the rest of server.js's startup:
/// AUTH link, storage link, UDP relays, plugins, the client listener, the
/// kernel buffer check and the super/child mesh links.
pub async fn start(p_server: &Arc<Server>) -> Result<Started, String> {
    use colors as c;
    println!("{}Communication Server Starting...{}", c::FG_YELLOW, c::RESET);
    if p_server.cfg.is_true("allow_fake_SSL") {
        // should be false in production
        println!("{}ATTENTION!!  SECURITY RISK . NODE_TLS_REJECT_UNAUTHORIZED = 0{}", c::B_FG_YELLOW, c::RESET);
        println!("{}THIS IS ACCEPTED ONLY IF COMM with ANDRUAV_AUTH is 127.0.0.1{}", c::B_FG_YELLOW, c::RESET);
    }
    if p_server.uses_auth() {
        s2s::auth_client::AuthClient::start(p_server);
    }
    s2s::db_proxy_client::DbProxyClient::start(p_server);
    s2s::udp_relay_client::UdpRelayClient::start(p_server);

    // the chat server
    println!(
        "{}[OK] Comm Server Manager has Started at port {}{}",
        c::B_SUCCESS,
        crate::net::cfg_str(p_server.cfg.get("server_port")),
        c::RESET
    );
    p_server.plugins.init(p_server);
    if p_server.cfg.is_false("ignoreLoadingTasks") {
        println!(
            "{}[INFO] ignoreLoadingTasks is false: tasks are served by the storage server (the MySQL task store is not supported){}",
            c::FG_YELLOW,
            c::RESET
        );
    }
    let v_chat = chat::connection::start_chat_server(p_server).await?;

    for v_warning in s2s::udp_relay_client::UdpRelayClient::config_warnings(&p_server.cfg) {
        println!("[WARN] {}", v_warning);
    }

    let v_parent = if p_server.cfg.is_true("enable_super_server") {
        Some(s2s::parent::ParentServer::start(p_server).await?)
    } else {
        None
    };
    if p_server.cfg.is_true("enable_persistant_relay") {
        s2s::child::ChildLink::start(p_server);
    }
    Ok(Started { chat: v_chat, parent: v_parent })
}
