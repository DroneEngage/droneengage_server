//! Server configuration, ported from droneengage_server_common's
//! js_serverConfig.js (create) and this server's js_serverConfig.js:
//!
//!   - `server.config` (or `--config=<file>`) is JSON with // and /* */
//!     comments, read relative to the server root;
//!   - `<configFile>.local` is merged on top when present (shallow: each
//!     top-level key replaces the base value), and a broken local file only
//!     prints a warning;
//!   - the `de_comm_debug_logging` env var overrides `debug_logging`
//!     ("true"/"1" enable it).
//!
//! The configuration is kept as the parsed JSON object and read the way the
//! JavaScript code read `global.m_serverconfig.m_configuration.<key>`, so a
//! missing key, `null` and `=== true` checks behave the same.
//!
//! The server root is the working directory: the run scripts start the
//! server from the repository root, where `node src/server.js` ran.

use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

use crate::util::{is_true, js_key};

pub const DEFAULT_CONFIG_FILENAME: &str = "server.config";

/// Removes // and /* */ comments while leaving string contents untouched
/// (so "http://..." values survive).
pub fn strip_json_comments(p_text: &str) -> String {
    let c: Vec<char> = p_text.chars().collect();
    let mut v_out = String::with_capacity(p_text.len());
    let mut v_in_string = false;
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let n = c.get(i + 1).copied();
        if v_in_string {
            v_out.push(ch);
            if ch == '\\' {
                if let Some(n) = n {
                    v_out.push(n);
                }
                i += 2;
                continue;
            }
            if ch == '"' {
                v_in_string = false;
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            v_in_string = true;
            v_out.push(ch);
            i += 1;
            continue;
        }
        if ch == '/' && n == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && n == Some('*') {
            i += 2;
            while i < c.len() && !(c[i] == '*' && c.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            continue;
        }
        v_out.push(ch);
        i += 1;
    }
    v_out
}

/// Command line arguments, like droneengage_server_common's hlp_args:
/// `--key=value` sets key (value up to the next '='; `--key` alone is
/// present without a value) and `-abc` sets the flags a, b and c.
#[derive(Debug, Default, Clone)]
pub struct Args {
    m_args: Vec<(String, Option<String>)>,
}

impl Args {
    pub fn parse(p_argv: &[String]) -> Args {
        let mut v_args: Vec<(String, Option<String>)> = Vec::new();
        let mut fn_set = |k: String, v: Option<String>| {
            v_args.retain(|(kk, _)| *kk != k);
            v_args.push((k, v));
        };
        for v_arg in p_argv {
            if let Some(v_long) = v_arg.strip_prefix("--") {
                let mut v_parts = v_long.split('=');
                let v_key = v_parts.next().unwrap_or("").to_string();
                fn_set(v_key, v_parts.next().map(str::to_string));
            } else if let Some(v_flags) = v_arg.strip_prefix('-') {
                for c in v_flags.chars() {
                    fn_set(c.to_string(), Some("true".to_string()));
                }
            }
        }
        Args { m_args: v_args }
    }

    pub fn has(&self, p_key: &str) -> bool {
        self.m_args.iter().any(|(k, _)| k == p_key)
    }

    pub fn get(&self, p_key: &str) -> Option<&str> {
        self.m_args.iter().find(|(k, _)| k == p_key).and_then(|(_, v)| v.as_deref())
    }
}

/// The parsed configuration (m_configuration) and the server root it was
/// loaded from.
#[derive(Debug, Clone)]
pub struct Config {
    m_raw: Value,
    m_base_dir: PathBuf,
    m_file: PathBuf,
}

/// What can go wrong loading the configuration (the JavaScript server exits
/// with code 1 on both).
#[derive(Debug)]
pub enum ConfigError {
    NotFound { name: String, path: PathBuf, error: String },
    BadFormat { name: String, error: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::NotFound { name, path, error } => {
                write!(f, "FATAL: could not find {} at {}\n{}", name, path.display(), error)
            }
            ConfigError::BadFormat { name, error } => write!(f, "FATAL: Bad File Format {}\n{}", name, error),
        }
    }
}

impl Config {
    /// A configuration from an in-memory object (tests, embedding).
    pub fn from_value(p_raw: Value, p_base_dir: impl Into<PathBuf>) -> Config {
        let v_raw = if p_raw.is_object() { p_raw } else { Value::Object(Map::new()) };
        Config { m_raw: v_raw, m_base_dir: p_base_dir.into(), m_file: PathBuf::new() }
    }

    /// Loads p_file_name (relative to p_base_dir) plus `<file>.local`, then
    /// applies the environment overrides from p_env.
    pub fn load(
        p_base_dir: &Path,
        p_file_name: &str,
        p_env: &dyn Fn(&str) -> Option<String>,
        p_log: &dyn Fn(&str),
    ) -> Result<Config, ConfigError> {
        let v_path = p_base_dir.join(p_file_name);
        let v_text = std::fs::read_to_string(&v_path).map_err(|e| ConfigError::NotFound {
            name: p_file_name.to_string(),
            path: v_path.clone(),
            error: e.to_string(),
        })?;
        let mut v_config: Value = serde_json::from_str(&strip_json_comments(&v_text))
            .map_err(|e| ConfigError::BadFormat { name: p_file_name.to_string(), error: e.to_string() })?;
        if !v_config.is_object() {
            // Object.assign on a non-object base is not something a config file does
            v_config = Value::Object(Map::new());
        }

        let mut v_local = v_path.as_os_str().to_owned();
        v_local.push(".local");
        let v_local = PathBuf::from(v_local);
        if v_local.exists() {
            match std::fs::read_to_string(&v_local)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str::<Value>(&strip_json_comments(&t)).map_err(|e| e.to_string()))
            {
                Ok(Value::Object(v_over)) => {
                    if let Value::Object(m) = &mut v_config {
                        for (k, v) in v_over {
                            m.insert(k, v);
                        }
                    }
                    p_log(&format!("[Config] Local override applied: {}", v_local.display()));
                }
                Ok(_) => p_log(&format!("[Config] Local override applied: {}", v_local.display())),
                Err(e) => {
                    p_log(&format!("[Config] WARNING: could not parse local override {}: {}", v_local.display(), e))
                }
            }
        }

        if let Some(v_val) = p_env("de_comm_debug_logging") {
            if let Value::Object(m) = &mut v_config {
                m.insert("debug_logging".into(), Value::Bool(v_val == "true" || v_val == "1"));
            }
        }

        Ok(Config { m_raw: v_config, m_base_dir: p_base_dir.to_path_buf(), m_file: v_path })
    }

    /// The configuration object.
    pub fn raw(&self) -> &Value {
        &self.m_raw
    }

    /// The server root relative paths resolve against.
    pub fn base_dir(&self) -> &Path {
        &self.m_base_dir
    }

    /// The loaded file (empty for from_value).
    pub fn file(&self) -> &Path {
        &self.m_file
    }

    /// `m_configuration[key]` (None = undefined).
    pub fn get(&self, p_key: &str) -> Option<&Value> {
        self.m_raw.get(p_key)
    }

    /// `m_configuration[key] === true`.
    pub fn is_true(&self, p_key: &str) -> bool {
        is_true(self.get(p_key))
    }

    /// `m_configuration[key] === false`.
    pub fn is_false(&self, p_key: &str) -> bool {
        matches!(self.get(p_key), Some(Value::Bool(false)))
    }

    /// `String(m_configuration[key])` when set (not null/undefined).
    pub fn string(&self, p_key: &str) -> Option<String> {
        match self.get(p_key) {
            None | Some(Value::Null) => None,
            Some(v) => Some(js_key(Some(v))),
        }
    }

    /// `server_id`, or 'unknown' when it is missing/empty (getServerOriginID).
    pub fn server_id(&self) -> String {
        match self.get("server_id") {
            Some(v) if crate::util::js_truthy(Some(v)) => js_key(Some(v)),
            _ => "unknown".into(),
        }
    }

    /// `path.resolve(serverRoot, p)`.
    pub fn resolve(&self, p_path: &str) -> PathBuf {
        let v_joined = self.m_base_dir.join(p_path);
        v_joined.components().filter(|c| !matches!(c, std::path::Component::CurDir)).collect()
    }

    /// Replaces one key (tests).
    pub fn set(&mut self, p_key: &str, p_value: Value) {
        if let Value::Object(m) = &mut self.m_raw {
            m.insert(p_key.to_string(), p_value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shipped_config_parses() {
        let v_cfg =
            Config::load(Path::new(env!("CARGO_MANIFEST_DIR")), DEFAULT_CONFIG_FILENAME, &|_| None, &|_| {}).unwrap();
        assert_eq!(v_cfg.get("server_id"), Some(&json!("DE_CommSrv")));
        assert_eq!(v_cfg.get("server_port"), Some(&json!(9966)));
        assert!(v_cfg.is_true("allow_fake_SSL"));
        assert!(v_cfg.get("outbound_guard").unwrap()["latest_wins_message_types"].as_array().unwrap().len() >= 20);
    }

    #[test]
    fn local_override_is_shallow_and_env_sets_debug_logging() {
        let v_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            v_dir.path().join("a.config"),
            "{ // c\n \"x\": {\"y\": 1, \"z\": 2}, \"u\": \"http://h//p\", /* b */ \"k\": 1 }",
        )
        .unwrap();
        std::fs::write(v_dir.path().join("a.config.local"), "{ \"x\": {\"z\": 3} }").unwrap();
        let v_logs = std::cell::RefCell::new(Vec::new());
        let v_cfg = Config::load(
            v_dir.path(),
            "a.config",
            &|k| (k == "de_comm_debug_logging").then(|| "1".to_string()),
            &|l| v_logs.borrow_mut().push(l.to_string()),
        )
        .unwrap();
        assert_eq!(v_cfg.raw(), &json!({"x": {"z": 3}, "u": "http://h//p", "k": 1, "debug_logging": true}));
        assert!(v_logs.borrow()[0].starts_with("[Config] Local override applied"));

        std::fs::write(v_dir.path().join("a.config.local"), "{ broken").unwrap();
        let v_cfg =
            Config::load(v_dir.path(), "a.config", &|_| None, &|l| v_logs.borrow_mut().push(l.to_string())).unwrap();
        assert_eq!(v_cfg.get("k"), Some(&json!(1)));
        assert!(v_logs.borrow().last().unwrap().contains("WARNING: could not parse local override"));

        let e = Config::load(v_dir.path(), "missing.config", &|_| None, &|_| {}).unwrap_err();
        assert!(e.to_string().starts_with("FATAL: could not find missing.config"));
        std::fs::write(v_dir.path().join("bad.config"), "{ nope").unwrap();
        let e = Config::load(v_dir.path(), "bad.config", &|_| None, &|_| {}).unwrap_err();
        assert!(e.to_string().starts_with("FATAL: Bad File Format bad.config"));
    }

    #[test]
    fn args_like_hlp_args() {
        let a = Args::parse(&["--config=x.config".into(), "-hv".into(), "--flag".into(), "--k=a=b".into()]);
        assert_eq!(a.get("config"), Some("x.config"));
        assert!(a.has("h") && a.has("v") && a.has("flag"));
        assert_eq!(a.get("flag"), None);
        assert_eq!(a.get("k"), Some("a"));
    }
}
