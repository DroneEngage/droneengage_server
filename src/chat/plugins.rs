//! Command plugins (pluginManager.js).
//!
//! A plugin sees every inbound client message (after the server injected the
//! permission, before routing) through `process_command`, like the
//! JavaScript plugins' processCommand(ws, message, jmsg, nullIndex). It
//! cannot change or block the message.
//!
//! The JavaScript server `require()`d each path listed in `command_plugin`.
//! Plugins are now Rust crates compiled into the server binary: a binary
//! registers them by name (`register`, see cli::run), and a
//! `command_plugin` entry selects one by a component of its path, e.g.
//! "./plugins/command_processing/js_andruav_command_processing" selects
//! `command_processing`. An entry that matches no registered plugin is
//! reported like a missing module and skipped. droneengage_server_plugins
//! builds the server binary with its plugins registered.

use serde_json::Value;
use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::colors as c;
use crate::hub::Client;
use crate::server::Server;

/// The connection a message came from, as a plugin sees it.
pub struct PluginConn<'a> {
    /// ws.m_loginRequest.m_accountID
    pub account_id: Option<&'a Value>,
    /// the plugin's own per-connection state (ws.m__status)
    pub state: &'a mut Option<Box<dyn Any + Send>>,
}

pub trait CommandPlugin: Send + Sync {
    /// processCommand: p_message is the raw frame, p_jmsg its parsed JSON
    /// header (with the injected permission `p`), p_null_index the index of
    /// the first 0 byte in p_message (-1 when there is none).
    fn process_command(&self, p_conn: &mut PluginConn<'_>, p_message: &[u8], p_jmsg: &Value, p_null_index: i64);
}

/// Builds a plugin; the argument is the plugin's directory (for its .env).
pub type PluginFactory = fn(&Path) -> Arc<dyn CommandPlugin>;

#[derive(Default)]
pub struct PluginManager {
    m_plugins: RwLock<Vec<Arc<dyn CommandPlugin>>>,
    /// compiled-in plugins by name
    m_registry: RwLock<Vec<(String, PluginFactory)>>,
}

impl PluginManager {
    pub fn new() -> PluginManager {
        PluginManager::default()
    }

    /// Makes a compiled-in plugin available to command_plugin entries whose
    /// path has a component named p_name (e.g. "command_processing").
    pub fn register(&self, p_name: &str, p_factory: PluginFactory) {
        self.m_registry.write().unwrap().push((p_name.to_string(), p_factory));
    }

    fn lookup(&self, p_entry: &str) -> Option<PluginFactory> {
        let v_names: Vec<&str> = p_entry.split(['/', '\\']).collect();
        self.m_registry.read().unwrap().iter().find(|(n, _)| v_names.contains(&n.as_str())).map(|(_, f)| *f)
    }

    /// Adds a plugin (tests, embedding).
    pub fn add(&self, p_plugin: Arc<dyn CommandPlugin>) {
        self.m_plugins.write().unwrap().push(p_plugin);
    }

    pub fn len(&self) -> usize {
        self.m_plugins.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// fn_initPlugins: loads the plugins named in command_plugin.
    pub fn init(&self, p_server: &Server) {
        let Some(v_list) = p_server.cfg.get("command_plugin").filter(|v| crate::util::js_truthy(Some(v))) else {
            println!("No command plugins specified in configuration.");
            return;
        };
        let Value::Array(v_list) = v_list else {
            println!("{}command_plugin must be a list of plugin paths{}", c::ERROR, c::RESET);
            return;
        };
        for v_entry in v_list {
            let v_path = crate::util::js_key(Some(v_entry));
            let v_resolved: PathBuf = p_server.cfg.resolve(&v_path);
            let v_dir = if v_resolved.is_dir() {
                v_resolved.clone()
            } else {
                v_resolved.parent().map(Path::to_path_buf).unwrap_or_else(|| p_server.cfg.base_dir().to_path_buf())
            };
            match self.lookup(&v_path) {
                Some(v_factory) => {
                    self.add(v_factory(&v_dir));
                    println!(
                        "{}[OK] Loading Plugin Succeeded -  {} {} with processCommand method. {}",
                        c::B_SUCCESS,
                        c::B_FG_YELLOW,
                        v_path,
                        c::RESET
                    );
                }
                None => println!("{}Could not find plugin module {} {} {}", c::ERROR, c::FG_YELLOW, v_path, c::RESET),
            }
        }
    }

    /// fn_processPlugins: every plugin, in order, for one inbound message.
    pub fn process(
        &self,
        _p_server: &Server,
        p_client: &mut Client,
        p_message: &[u8],
        p_jmsg: &Value,
        p_null_index: Option<usize>,
    ) {
        let v_plugins = self.m_plugins.read().unwrap();
        if v_plugins.is_empty() {
            return;
        }
        if p_client.plugin_state.len() < v_plugins.len() {
            p_client.plugin_state.resize_with(v_plugins.len(), || None);
        }
        let v_account = p_client.login.as_ref().and_then(|l| l.account_id.as_ref());
        for (i, v_plugin) in v_plugins.iter().enumerate() {
            let mut v_conn = PluginConn { account_id: v_account, state: &mut p_client.plugin_state[i] };
            let v_index = p_null_index.map(|i| i as i64).unwrap_or(-1);
            // a plugin error must not break routing
            let v_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                v_plugin.process_command(&mut v_conn, p_message, p_jmsg, v_index)
            }));
            if v_res.is_err() {
                println!("Error processing plugin command at plugin {}: panic", i);
            }
        }
    }
}
