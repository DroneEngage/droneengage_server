//! The comm server's configuration: the shipped file and its environment
//! override.

use andruavserver::config::{load, DEFAULT_CONFIG_FILENAME};
use serde_json::json;
use std::path::Path;

#[test]
fn shipped_config_parses() {
    let v_cfg = load(Path::new(env!("CARGO_MANIFEST_DIR")), DEFAULT_CONFIG_FILENAME, &|_| None, &|_| {}).unwrap();
    assert_eq!(v_cfg.get("server_id"), Some(&json!("DE_CommSrv")));
    assert_eq!(v_cfg.get("server_port"), Some(&json!(9966)));
    assert!(v_cfg.is_true("allow_fake_SSL"));
    assert!(v_cfg.get("outbound_guard").unwrap()["latest_wins_message_types"].as_array().unwrap().len() >= 20);
}

#[test]
fn de_comm_debug_logging_sets_debug_logging() {
    let v_dir = tempfile::tempdir().unwrap();
    std::fs::write(v_dir.path().join("a.config"), "{ \"debug_logging\": false }").unwrap();
    let v_env = |k: &str| (k == "de_comm_debug_logging").then(|| "1".to_string());
    let v_cfg = load(v_dir.path(), "a.config", &v_env, &|_| {}).unwrap();
    assert!(v_cfg.is_true("debug_logging"));
    let v_cfg = load(v_dir.path(), "a.config", &|_| Some("false".into()), &|_| {}).unwrap();
    assert!(!v_cfg.is_true("debug_logging"));
}
