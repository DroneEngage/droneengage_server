//! Server configuration: droneengage_server_common's loader with this
//! server's environment override (`de_comm_debug_logging` sets
//! `debug_logging`, "true"/"1" enable it).

pub use droneengage_server_common::config::*;

use serde_json::{Map, Value};
use std::path::Path;

fn debug_logging(p_cfg: &mut Map<String, Value>, p_value: &str) {
    p_cfg.insert("debug_logging".into(), Value::Bool(p_value == "true" || p_value == "1"));
}

/// js_serverConfig.js's envOverrides.
pub const ENV_OVERRIDES: [(&str, EnvOverride); 1] = [("de_comm_debug_logging", EnvOverride::Transform(debug_logging))];

/// Loads the comm server's configuration.
pub fn load(
    p_base_dir: &Path,
    p_file_name: &str,
    p_env: &dyn Fn(&str) -> Option<String>,
    p_log: &dyn Fn(&str),
) -> Result<Config, ConfigError> {
    Config::load_with(
        p_base_dir,
        p_file_name,
        &LoadOptions { env: p_env, log: p_log, overrides: &ENV_OVERRIDES, hash_handling: false },
    )
}
