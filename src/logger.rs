//! File logger replacing node-file-logger (global.m_logger): one file per
//! day, `<log_directory>/Logs_<YYYY-MM-DD>.log`, lines
//! `HH:mm:ss.SSS | <Level> | <message> | Service: <s> | Method: <m> | \n<object>`
//! with values printed the way node-stringify prints them. With
//! log_detailed=false (log level 'prod') Debug and Trace lines are dropped.
//!
//! Times are UTC: log_timeZone "GMT"/"UTC" (the default) is supported; other
//! zones print a warning and fall back to UTC.

use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::util::{civil, js_number_fmt, now_ms};

pub struct FileLogger {
    m_dir: PathBuf,
    m_detailed: bool,
    m_lock: Mutex<()>,
}

fn quote(p_text: &str) -> String {
    let mut v_out = String::with_capacity(p_text.len() + 2);
    v_out.push('\'');
    for c in p_text.chars() {
        match c {
            '\\' => v_out.push_str("\\\\"),
            '\'' => v_out.push_str("\\'"),
            '\n' => v_out.push_str("\\n"),
            '\r' => v_out.push_str("\\r"),
            '\t' => v_out.push_str("\\t"),
            '\u{8}' => v_out.push_str("\\b"),
            '\u{c}' => v_out.push_str("\\f"),
            c => v_out.push(c),
        }
    }
    v_out.push('\'');
    v_out
}

/// node-stringify output for a JSON value.
pub fn node_stringify(p_value: &Value) -> String {
    match p_value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => js_number_fmt(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => quote(s),
        Value::Array(a) => format!("[{}]", a.iter().map(node_stringify).collect::<Vec<_>>().join(",")),
        Value::Object(m) => format!(
            "({{{}}})",
            m.iter().map(|(k, v)| format!("{}:{}", quote(k), node_stringify(v))).collect::<Vec<_>>().join(",")
        ),
    }
}

impl FileLogger {
    /// @param p_dir      log_directory (default ./log)
    /// @param p_detailed log_detailed === true (log level 'debug')
    /// @param p_tz       log_timeZone
    pub fn new(p_dir: PathBuf, p_detailed: bool, p_tz: Option<&str>) -> FileLogger {
        if let Some(v_tz) = p_tz {
            if !matches!(v_tz.to_ascii_uppercase().as_str(), "GMT" | "UTC" | "ETC/UTC" | "ETC/GMT" | "Z") {
                println!("Node File Logger Warning: Invalid timezone. Set to default: GMT");
            }
        }
        if !p_dir.exists() {
            if let Err(_e) = std::fs::create_dir_all(&p_dir) {
                println!("Node File Logger Warning: Error occured while creating log folder. Set to default: ./");
            }
        }
        FileLogger { m_dir: p_dir, m_detailed: p_detailed, m_lock: Mutex::new(()) }
    }

    fn log(
        &self,
        p_level: &str,
        p_message: &str,
        p_service: Option<&str>,
        p_method: Option<&str>,
        p_obj: Option<&Value>,
    ) {
        if !self.m_detailed && (p_level == "Debug" || p_level == "Trace") {
            return;
        }
        let (y, mo, d, h, mi, s, ms) = civil(now_ms());
        let v_file = self.m_dir.join(format!("Logs_{:04}-{:02}-{:02}.log", y, mo, d));
        let mut v_line = format!("{:02}:{:02}:{:02}.{:03} | {} | {} | ", h, mi, s, ms, p_level, quote(p_message));
        if let Some(v_s) = p_service.filter(|s| !s.is_empty()) {
            v_line.push_str(&format!("Service: {} | ", v_s));
        }
        if let Some(v_m) = p_method.filter(|s| !s.is_empty()) {
            v_line.push_str(&format!("Method: {} | ", v_m));
        }
        if let Some(v_o) = p_obj.filter(|o| !o.is_null()) {
            v_line.push('\n');
            v_line.push_str(&node_stringify(v_o));
        }
        v_line.push('\n');
        let _g = self.m_lock.lock().unwrap();
        let _ = std::fs::create_dir_all(&self.m_dir);
        let v_res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&v_file)
            .and_then(|mut f| f.write_all(v_line.as_bytes()));
        if let Err(e) = v_res {
            println!("Node file logger Error: {}", e);
        }
    }

    pub fn info(&self, p_message: &str, p_service: Option<&str>, p_obj: Option<&Value>) {
        self.log("Info", p_message, p_service, None, p_obj);
    }

    pub fn warn(&self, p_message: &str, p_service: Option<&str>, p_obj: Option<&Value>) {
        self.log("Warn", p_message, p_service, None, p_obj);
    }

    pub fn error(&self, p_message: &str, p_service: Option<&str>, p_obj: Option<&Value>) {
        self.log("Error", p_message, p_service, None, p_obj);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stringify_like_node_stringify() {
        assert_eq!(
            node_stringify(&json!({"a": {"b": 1}, "e": [{"x": 1}], "q": "it's", "bs": "a\\b", "nl": "a\nb"})),
            r"({'a':({'b':1}),'e':[({'x':1})],'q':'it\'s','bs':'a\\b','nl':'a\nb'})"
        );
        assert_eq!(node_stringify(&json!([1, "a", 1.5, null])), "[1,'a',1.5,null]");
    }

    #[test]
    fn writes_daily_file_and_filters_debug() {
        let v_dir = tempfile::tempdir().unwrap();
        let v_log = FileLogger::new(v_dir.path().join("logs"), false, Some("GMT"));
        v_log.info("WS Created from Party", Some("fn_onConnect_Handler"), Some(&json!({"f": "k"})));
        v_log.log("Debug", "hidden", None, None, None);
        let v_files: Vec<_> = std::fs::read_dir(v_dir.path().join("logs")).unwrap().collect();
        assert_eq!(v_files.len(), 1);
        let v_text = std::fs::read_to_string(v_files[0].as_ref().unwrap().path()).unwrap();
        assert!(v_text.contains(" | Info | 'WS Created from Party' | Service: fn_onConnect_Handler | \n({'f':'k'})\n"));
        assert!(!v_text.contains("hidden"));
    }
}
