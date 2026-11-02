//! The comm server context: what the JavaScript code reached through
//! `global.*` (configuration, logger, chat server singleton, relay channel
//! singletons) and the module-level singletons (AUTH link, storage link, UDP
//! proxy, UDP relay broker). One `Arc<Server>` is shared by every task.

use serde_json::{json, Value};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::chat::outbound_guard::GuardConfig;
use crate::chat::plugins::PluginManager;
use crate::config::Config;
use crate::hub::Hub;
use crate::logger::FileLogger;
use crate::s2s::auth_client::AuthClient;
use crate::s2s::child::ChildLink;
use crate::s2s::db_proxy_client::DbProxyClient;
use crate::s2s::parent::ParentServer;
use crate::s2s::s2s_auth::S2sAuth;
use crate::s2s::udp_relay_client::UdpRelayClient;
use crate::util::js_stringify;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Process environment the server reads (injectable for tests).
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// de_comm_server_port
    pub comm_server_port: Option<String>,
    /// de_s2s_shared_secret
    pub s2s_shared_secret: Option<String>,
    /// NODE_EXTRA_CA_CERTS (extra CA file trusted on the AUTH link)
    pub extra_ca_certs: Option<String>,
}

impl Env {
    pub fn from_process() -> Env {
        Env {
            comm_server_port: std::env::var("de_comm_server_port").ok(),
            s2s_shared_secret: std::env::var("de_s2s_shared_secret").ok(),
            extra_ca_certs: std::env::var("NODE_EXTRA_CA_CERTS").ok(),
        }
    }
}

pub struct Server {
    pub cfg: Config,
    pub env: Env,
    pub hub: Mutex<Hub>,
    pub guard: GuardConfig,
    pub s2s: S2sAuth,
    pub auth: AuthClient,
    pub db: DbProxyClient,
    pub relay: UdpRelayClient,
    pub parent: ParentServer,
    pub child: ChildLink,
    pub plugins: PluginManager,
    pub logger: Option<FileLogger>,
    /// global.DEBUG_LOGGING
    pub debug: bool,
}

impl Server {
    /// A server for p_cfg; nothing is started (see main.rs / start()).
    pub fn new(p_cfg: Config, p_env: Env, p_logger: Option<FileLogger>) -> Arc<Server> {
        let v_debug = p_cfg.is_true("debug_logging");
        Arc::new(Server {
            guard: GuardConfig::from_config(&p_cfg),
            s2s: S2sAuth::new(&p_cfg, p_env.s2s_shared_secret.clone()),
            auth: AuthClient::new(),
            db: DbProxyClient::new(),
            relay: UdpRelayClient::new(&p_cfg),
            parent: ParentServer::new(),
            child: ChildLink::new(),
            plugins: PluginManager::new(),
            hub: Mutex::new(Hub::new()),
            logger: p_logger,
            debug: v_debug,
            env: p_env,
            cfg: p_cfg,
        })
    }

    pub fn hub(&self) -> MutexGuard<'_, Hub> {
        self.hub.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `process.env.de_comm_server_port || server_port`
    pub fn server_port_value(&self) -> Option<Value> {
        match &self.env.comm_server_port {
            Some(p) if !p.is_empty() => Some(Value::String(p.clone())),
            _ => self.cfg.get("server_port").cloned(),
        }
    }

    /// getServerOriginID(): server_id, or 'unknown'.
    pub fn origin_id(&self) -> String {
        self.cfg.server_id()
    }

    /// AUTH is used unless ignore_auth_server is true.
    pub fn uses_auth(&self) -> bool {
        !self.cfg.is_true("ignore_auth_server")
    }

    /// fn_updateServerWatchdog: sends this server's INFO card to AUTH (its
    /// accounts and units, so AUTH can route between comm servers).
    pub fn update_auth_server(&self) {
        let v_card = {
            let v_hub = self.hub();
            let mut v_obj = serde_json::Map::new();
            v_obj.insert("isOnline".into(), json!(true));
            v_obj.insert("version".into(), json!(VERSION));
            if let Some(v) = self.cfg.get("server_id") {
                v_obj.insert("serverId".into(), v.clone());
            }
            if let Some(v) = self.cfg.get("public_host") {
                v_obj.insert("public_host".into(), v.clone());
            }
            if let Some(v) = self.server_port_value() {
                v_obj.insert("serverPort".into(), v);
            }
            v_obj.insert("accounts".into(), json!(v_hub.unit_keys()));
            v_obj.insert("accountDetails".into(), v_hub.account_details());
            json!({ "c": crate::constants::CONST_CS_CMD_INFO, "d": Value::Object(v_obj) })
        };
        self.auth.send_message(js_stringify(&v_card));
    }

    /// Optional file logger (global.m_logger)
    pub fn log_info(&self, p_message: &str, p_service: Option<&str>, p_obj: Option<&Value>) {
        if let Some(l) = &self.logger {
            l.info(p_message, p_service, p_obj);
        }
    }

    pub fn log_warn(&self, p_message: &str, p_service: Option<&str>, p_obj: Option<&Value>) {
        if let Some(l) = &self.logger {
            l.warn(p_message, p_service, p_obj);
        }
    }

    pub fn log_error(&self, p_message: &str, p_service: Option<&str>, p_obj: Option<&Value>) {
        if let Some(l) = &self.logger {
            l.error(p_message, p_service, p_obj);
        }
    }
}
