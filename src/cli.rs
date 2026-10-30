//! The server binary (src/server.js): arguments, configuration, banner,
//! logger, memory watchdog, then the server. `run` is public so a binary
//! that compiles plugins in (droneengage_server_plugins) can register them.

use std::path::PathBuf;
use std::time::Duration;

use crate::colors as c;
use crate::config::{Args, Config, DEFAULT_CONFIG_FILENAME};
use crate::logger::FileLogger;
use crate::net::cfg_str;
use crate::server::{Env, Server, VERSION};
use crate::util::now_ms;

fn display_help() {
    println!("==================================");
    println!("{}Andruav Communication Server version \"{}\"{}", c::BRIGHT, VERSION, c::RESET);
    println!("----------------------------------");
    println!("--config=config_filename config file ");
    println!("-h help ");
    println!("-v version");
    println!("==================================");
}

fn display_info(p_cfg: &Config, p_env: &Env) -> Option<FileLogger> {
    let v_port = match &p_env.comm_server_port {
        Some(p) if !p.is_empty() => p.clone(),
        _ => cfg_str(p_cfg.get("server_port")),
    };
    println!("=============================================");
    println!("{}DE Communication Server version \"{}\"{}", c::BRIGHT, VERSION, c::RESET);
    println!("---------------------------------------------");
    println!("Server Name  {}{}{}", c::B_SUCCESS, cfg_str(p_cfg.get("server_id")), c::RESET);
    println!(
        "Listening on ip: {}{}{} port: {}{}{}",
        c::B_SUCCESS,
        cfg_str(p_cfg.get("server_ip")),
        c::RESET,
        c::B_SUCCESS,
        v_port,
        c::RESET
    );
    if p_cfg.get("public_host").map(|v| !v.is_null()).unwrap_or(false) {
        println!("Accessed as Public Host: {}{}{}", c::B_SUCCESS, cfg_str(p_cfg.get("public_host")), c::RESET);
    }
    println!(
        "Auth Server ip: {}{}{} port: {}{}{}",
        c::B_SUCCESS,
        cfg_str(p_cfg.get("s2s_ws_target_ip")),
        c::RESET,
        c::B_SUCCESS,
        cfg_str(p_cfg.get("s2s_ws_target_port")),
        c::RESET
    );
    let v_logger = if !p_cfg.is_false("ignoreLog") {
        println!("logging is {}disabled{}", c::B_FG_YELLOW, c::RESET);
        None
    } else {
        let v_dir = match p_cfg.string("log_directory") {
            Some(d) => p_cfg.resolve(&d),
            None => p_cfg.resolve("./log"),
        };
        let v_logger = FileLogger::new(v_dir, p_cfg.is_true("log_detailed"), p_cfg.string("log_timeZone").as_deref());
        println!("logging is {}enabled{}", c::B_FG_YELLOW, c::RESET);
        v_logger.info("System Started.", None, None);
        Some(v_logger)
    };
    // console.log("%s", new Date()) prints the ISO form
    println!("Datetime: {}", crate::util::iso_time(now_ms()));
    println!("=====================================================================");
    v_logger
}

/// Resident set size of this process in MB (process.memoryUsage().rss).
fn rss_mb() -> Option<f64> {
    let v_status = std::fs::read_to_string("/proc/self/status").ok()?;
    let v_line = v_status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let v_kb: f64 = v_line.split_whitespace().nth(1)?.parse().ok()?;
    Some((v_kb / 1024.0 * 100.0).round() / 100.0)
}

/// checkMemory: every minute; above memory_max (MB) the process exits with
/// code 1 so its supervisor restarts it.
fn check_memory(p_cfg: &Config, p_debug: bool) {
    let Some(v_rss) = rss_mb() else { return };
    if p_debug {
        println!("rss {} MB - ", v_rss);
    }
    if let Some(v_max) = p_cfg.get("memory_max").filter(|v| !v.is_null()) {
        let v_max = crate::util::js_number(Some(v_max));
        if v_max < v_rss {
            println!("Memory is {}{}{} RESTART{}", c::FG_YELLOW, v_rss, c::ERROR, c::RESET);
            std::process::exit(1);
        }
    }
}

/// Runs the server until SIGINT; p_register adds compiled-in plugins.
pub async fn run(p_register: impl FnOnce(&crate::chat::plugins::PluginManager)) {
    let v_argv: Vec<String> = std::env::args().skip(1).collect();
    let v_args = Args::parse(&v_argv);
    if v_args.has("h") || v_args.has("help") {
        display_help();
        std::process::exit(0);
    }
    if v_args.has("v") || v_args.has("version") {
        println!("Andruav Authentication Server version: \"{}\"", VERSION);
        std::process::exit(0);
    }
    let v_file = v_args.get("config").map(str::to_string).unwrap_or_else(|| DEFAULT_CONFIG_FILENAME.to_string());

    let v_root: PathBuf = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let v_env = Env::from_process();
    let v_cfg = match crate::config::load(&v_root, &v_file, &|k| std::env::var(k).ok(), &|l| println!("{}", l)) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    let v_debug = v_cfg.is_true("debug_logging");
    let v_logger = display_info(&v_cfg, &v_env);
    let v_server = Server::new(v_cfg.clone(), v_env, v_logger);
    p_register(&v_server.plugins);

    {
        let v_cfg = v_cfg.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                check_memory(&v_cfg, v_debug);
            }
        });
    }

    if let Err(e) = crate::start(&v_server).await {
        println!("{}", e);
        std::process::exit(1);
    }

    let _ = tokio::signal::ctrl_c().await;
    v_server.log_warn("SIGINT.", None, None);
    std::process::exit(0);
}
