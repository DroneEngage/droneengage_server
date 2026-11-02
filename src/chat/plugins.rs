//! Command plugins (pluginManager.js).
//!
//! A plugin sees every inbound client message (after the server injected the
//! permission) through `process_command`, like the JavaScript plugins'
//! processCommand(ws, message, jmsg, nullIndex). It cannot change or block the
//! message: routing only queues the routed frame for the plugin's own worker
//! thread, which runs the plugin outside the hub lock and in order. A full
//! queue drops the newest frame and counts it (`dropped`).
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
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use crate::colors as c;
use crate::hub::Payload;
use crate::server::Server;
use crate::util::now_ms;

/// One plugin's queue limits: frames beyond either are dropped, so a plugin
/// that stops consuming cannot grow memory without bound.
pub const PLUGIN_QUEUE_MAX_FRAMES: usize = 4096;
pub const PLUGIN_QUEUE_MAX_BYTES: usize = 64 * 1024 * 1024;
/// A plugin dropping frames is reported at most this often.
const DROP_LOG_INTERVAL_MS: i64 = 10_000;

/// The connection a message came from, as a plugin sees it.
pub struct PluginConn<'a> {
    /// ws.m_loginRequest.m_accountID
    pub account_id: Option<&'a Value>,
    /// the plugin's own per-connection state (ws.m__status), dropped when the
    /// socket closes
    pub state: &'a mut Option<Box<dyn Any + Send>>,
}

pub trait CommandPlugin: Send + Sync {
    /// processCommand, on the plugin's worker thread: p_message is the frame
    /// being routed (with the injected permission `p`), p_null_index the index
    /// of the first 0 byte in it (-1 when there is none). A plugin that needs
    /// the JSON header parses its own copy.
    fn process_command(&self, p_conn: &mut PluginConn<'_>, p_message: &[u8], p_null_index: i64);
}

enum Job {
    Frame { conn: u64, account: Option<Value>, frame: Payload, null_index: i64 },
    /// after the connection's last frame: its plugin state goes
    Closed(u64),
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    frames: usize,
    bytes: usize,
    /// connections with a frame accepted since their last close; only these
    /// queue a Closed, which keeps closes bounded when a plugin is stuck
    open_conns: HashSet<u64>,
    busy: bool,
    /// the worker is waiting: only then does a push pay for a wake-up
    sleeping: bool,
    stopped: bool,
}

/// The queue between routing and one plugin's worker thread.
#[derive(Default)]
struct PluginWorker {
    m_queue: Mutex<Queue>,
    m_wake: Condvar,
    m_idle: Condvar,
    m_dropped: AtomicU64,
    m_last_drop_log: AtomicI64,
}

impl PluginWorker {
    fn push_frame(&self, p_index: usize, p_conn: u64, p_account: Option<&Value>, p_frame: &Payload, p_null_index: i64) {
        let v_len = p_frame.as_bytes().len();
        let mut v_queue = self.m_queue.lock().unwrap();
        if v_queue.frames >= PLUGIN_QUEUE_MAX_FRAMES || v_queue.bytes + v_len > PLUGIN_QUEUE_MAX_BYTES {
            drop(v_queue);
            self.count_drop(p_index);
            return;
        }
        v_queue.frames += 1;
        v_queue.bytes += v_len;
        v_queue.open_conns.insert(p_conn);
        v_queue.jobs.push_back(Job::Frame {
            conn: p_conn,
            account: p_account.cloned(),
            frame: p_frame.clone(),
            null_index: p_null_index,
        });
        self.wake(v_queue);
    }

    fn wake(&self, mut p_queue: std::sync::MutexGuard<'_, Queue>) {
        let v_sleeping = std::mem::take(&mut p_queue.sleeping);
        drop(p_queue);
        if v_sleeping {
            self.m_wake.notify_one();
        }
    }

    fn count_drop(&self, p_index: usize) {
        let v_total = self.m_dropped.fetch_add(1, Ordering::Relaxed) + 1;
        let v_now = now_ms();
        let v_last = self.m_last_drop_log.load(Ordering::Relaxed);
        if v_now - v_last >= DROP_LOG_INTERVAL_MS
            && self.m_last_drop_log.compare_exchange(v_last, v_now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            println!("{}Plugin {} is not keeping up: {} frames dropped{}", c::ERROR, p_index, v_total, c::RESET);
        }
    }

    fn push_closed(&self, p_conn: u64) {
        let mut v_queue = self.m_queue.lock().unwrap();
        if v_queue.open_conns.remove(&p_conn) {
            v_queue.jobs.push_back(Job::Closed(p_conn));
            self.wake(v_queue);
        }
    }

    fn next_job(&self) -> Option<Job> {
        let mut v_queue = self.m_queue.lock().unwrap();
        v_queue.busy = false;
        loop {
            if let Some(v_job) = v_queue.jobs.pop_front() {
                if let Job::Frame { frame, .. } = &v_job {
                    let v_len = frame.as_bytes().len();
                    v_queue.frames -= 1;
                    v_queue.bytes -= v_len;
                }
                v_queue.busy = true;
                return Some(v_job);
            }
            self.m_idle.notify_all();
            if v_queue.stopped {
                return None;
            }
            v_queue.sleeping = true;
            v_queue = self.m_wake.wait(v_queue).unwrap();
        }
    }

    /// The worker thread: the plugin's jobs in order, and its per-connection
    /// state, which nothing else touches.
    fn run(&self, p_index: usize, p_plugin: &dyn CommandPlugin) {
        let mut v_states: HashMap<u64, Option<Box<dyn Any + Send>>> = HashMap::new();
        while let Some(v_job) = self.next_job() {
            match v_job {
                Job::Frame { conn, account, frame, null_index } => {
                    let v_state = v_states.entry(conn).or_default();
                    let mut v_conn = PluginConn { account_id: account.as_ref(), state: v_state };
                    // a plugin error must not stop its worker
                    let v_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        p_plugin.process_command(&mut v_conn, frame.as_bytes(), null_index)
                    }));
                    if v_res.is_err() {
                        println!("Error processing plugin command at plugin {}: panic", p_index);
                    }
                }
                Job::Closed(v_conn_id) => {
                    v_states.remove(&v_conn_id);
                }
            }
        }
    }
}

/// Builds a plugin; the argument is the plugin's directory (for its .env).
pub type PluginFactory = fn(&Path) -> Arc<dyn CommandPlugin>;

#[derive(Default)]
pub struct PluginManager {
    m_workers: RwLock<Vec<Arc<PluginWorker>>>,
    /// m_workers.len(), so a server without plugins skips the lock per message
    m_count: AtomicUsize,
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

    /// Adds a plugin (tests, embedding) and starts its worker thread.
    pub fn add(&self, p_plugin: Arc<dyn CommandPlugin>) {
        let mut v_workers = self.m_workers.write().unwrap();
        let v_index = v_workers.len();
        let v_worker = Arc::new(PluginWorker::default());
        let v_thread_worker = v_worker.clone();
        std::thread::Builder::new()
            .name(format!("plugin-{}", v_index))
            .spawn(move || v_thread_worker.run(v_index, p_plugin.as_ref()))
            .expect("plugin worker thread");
        v_workers.push(v_worker);
        self.m_count.store(v_workers.len(), Ordering::Relaxed);
    }

    pub fn len(&self) -> usize {
        self.m_count.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Frames dropped so far because plugin p_index's queue was full.
    pub fn dropped(&self, p_index: usize) -> u64 {
        self.m_workers.read().unwrap()[p_index].m_dropped.load(Ordering::Relaxed)
    }

    /// Waits until every plugin has finished what is queued (tests, shutdown).
    pub fn wait_idle(&self) {
        for v_worker in self.m_workers.read().unwrap().iter() {
            let mut v_queue = v_worker.m_queue.lock().unwrap();
            while !v_queue.jobs.is_empty() || v_queue.busy {
                v_queue = v_worker.m_idle.wait(v_queue).unwrap();
            }
        }
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

    /// fn_processPlugins: queues one inbound message for every plugin. p_frame
    /// is the frame being routed and p_null_index the first 0 byte in it.
    pub fn dispatch(&self, p_conn: u64, p_account: Option<&Value>, p_frame: &Payload, p_null_index: Option<usize>) {
        if self.is_empty() {
            return;
        }
        let v_index = p_null_index.map(|i| i as i64).unwrap_or(-1);
        for (i, v_worker) in self.m_workers.read().unwrap().iter().enumerate() {
            v_worker.push_frame(i, p_conn, p_account, p_frame, v_index);
        }
    }

    /// A socket closed: after its last frame, each plugin drops its state.
    pub fn connection_closed(&self, p_conn: u64) {
        if self.is_empty() {
            return;
        }
        for v_worker in self.m_workers.read().unwrap().iter() {
            v_worker.push_closed(p_conn);
        }
    }
}

impl Drop for PluginManager {
    /// The workers finish what is queued and exit.
    fn drop(&mut self) {
        for v_worker in self.m_workers.read().unwrap().iter() {
            v_worker.m_queue.lock().unwrap().stopped = true;
            v_worker.m_wake.notify_all();
        }
    }
}
