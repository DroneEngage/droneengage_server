//! de_loadgen: benchmark load client for the DroneEngage comm server.
//!
//! Replaces routing_benchmark.py where a single Python process was the
//! limit: one Rust process drives a whole small fleet at a high rate over
//! WSS. Two login modes:
//!   --local   de_auth with local_server_enabled (everyone lands in one
//!             account/group — single-account runs only)
//!   --auth    the real login path: a stub AUTH announces each login, the
//!             party presents the temp key it got — several accounts on one
//!             server, each in its own group
//!
//! What it counts, in plain terms:
//!   sent      frames drones offered to the server
//!   delivered frames received at the account's GCS sockets
//!   rounds    echo replies that completed a round trip (p2p mode only —
//!             the old THROUGHPUT number, ~two routed frames each)
//!
//! Examples:
//!   de_loadgen --local --pairs 12 --mode p2p --secs 8
//!   de_loadgen --auth 127.0.0.1:19970 --fleet "accA:7:2,accB:3:1" \
//!       --mode broadcast --secs 10 --server-pid 12345

mod auth;
mod party;
mod stats;

use stats::{cpu_percent, lat_summary, proc_ticks, AccountStats};

use std::collections::HashSet;
use std::process::exit;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// One fleet account: N drones + M GCS in one group.
struct Fleet {
    account: String,
    drones: usize,
    gcs: usize,
    group: String,
}

struct Args {
    server: String,
    local: bool,
    auth_addr: String,
    cert: String,
    key: String,
    fleets: Vec<Fleet>,
    broadcast: bool,
    binary: bool,
    secs: f64,
    rate: f64,
    pad: usize,
    server_pid: Option<u32>,
    prefix: String,
}

fn usage() -> ! {
    eprintln!(
        "de_loadgen --server H:P [--local | --auth H:P --cert C --key K] \
         (--fleet A:D:G[,A:D:G] | --account A --drones N --gcs M [--group G] | --pairs N [--group G]) \
         [--mode p2p|broadcast] [--binary] [--secs N] [--rate N] [--msg-bytes N] [--server-pid PID] [--prefix S]"
    );
    exit(2)
}

fn parse_args() -> Args {
    let mut a = Args {
        server: "127.0.0.1:19966".into(),
        local: false,
        auth_addr: "127.0.0.1:19970".into(),
        cert: "./ssl_local/ssl_airgap/domain.crt".into(),
        key: "./ssl_local/ssl_airgap/domain.key".into(),
        fleets: Vec::new(),
        broadcast: false,
        binary: false,
        secs: 8.0,
        rate: 0.0,
        pad: 64,
        server_pid: None,
        prefix: String::new(),
    };
    let mut v_account: Option<String> = None;
    let mut v_drones: Option<usize> = None;
    let mut v_gcs: Option<usize> = None;
    let mut v_group = "g1".to_string();
    let v_args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    let next = |i: &mut usize| -> String {
        *i += 1;
        v_args.get(*i).cloned().unwrap_or_else(|| usage())
    };
    while i < v_args.len() {
        match v_args[i].as_str() {
            "--server" => a.server = next(&mut i),
            "--local" => a.local = true,
            "--auth" => a.auth_addr = next(&mut i),
            "--cert" => a.cert = next(&mut i),
            "--key" => a.key = next(&mut i),
            "--fleet" => {
                for v_part in next(&mut i).split(',') {
                    let v_f: Vec<&str> = v_part.split(':').collect();
                    if v_f.len() < 3 {
                        usage();
                    }
                    a.fleets.push(Fleet {
                        account: v_f[0].into(),
                        drones: v_f[1].parse().unwrap_or_else(|_| usage()),
                        gcs: v_f[2].parse().unwrap_or_else(|_| usage()),
                        group: v_f.get(3).map(|g| g.to_string()).unwrap_or_else(|| "g1".into()),
                    });
                }
            }
            "--pairs" => {
                let v_n: usize = next(&mut i).parse().unwrap_or_else(|_| usage());
                a.fleets.push(Fleet { account: "acct".into(), drones: v_n, gcs: v_n, group: v_group.clone() });
            }
            "--account" => v_account = Some(next(&mut i)),
            "--drones" => v_drones = Some(next(&mut i).parse().unwrap_or_else(|_| usage())),
            "--gcs" => v_gcs = Some(next(&mut i).parse().unwrap_or_else(|_| usage())),
            "--group" => v_group = next(&mut i),
            "--mode" => a.broadcast = next(&mut i) == "broadcast",
            "--binary" => a.binary = true,
            "--secs" => a.secs = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--rate" => a.rate = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--msg-bytes" => a.pad = next(&mut i).parse().unwrap_or_else(|_| usage()),
            "--server-pid" => a.server_pid = Some(next(&mut i).parse().unwrap_or_else(|_| usage())),
            "--prefix" => a.prefix = next(&mut i),
            _ => usage(),
        }
        i += 1;
    }
    if let (Some(v_acc), Some(v_d), Some(v_g)) = (v_account, v_drones, v_gcs) {
        a.fleets.push(Fleet { account: v_acc, drones: v_d, gcs: v_g, group: v_group });
    }
    if a.fleets.is_empty() {
        usage();
    }
    a
}

fn host_port(p_addr: &str) -> (String, String) {
    let (v_host, v_port) = p_addr.rsplit_once(':').unwrap_or((p_addr, "19966"));
    (v_host.trim_start_matches('[').trim_end_matches(']').to_string(), v_port.to_string())
}

/// One drone's send loop: paced at p_rate frames/s (0 = as fast as the
/// socket takes them), stopped by p_running.
async fn drone_sender(
    p_tx: tokio::sync::mpsc::Sender<tokio_tungstenite::tungstenite::Message>,
    p_stats: Arc<AccountStats>,
    p_running: Arc<AtomicBool>,
    p_name: String,
    p_target: Option<String>,
    p_broadcast: bool,
    p_binary: bool,
    p_rate: f64,
    p_pad: String,
) {
    let v_mt = if p_binary { party::MT_BINARY } else { party::MT_TELEMETRY };
    let v_payload = vec![0xAB_u8; 128];
    let v_tg = if p_broadcast { None } else { p_target.as_deref() };
    let v_period = if p_rate > 0.0 { Some(Duration::from_secs_f64(1.0 / p_rate)) } else { None };
    let mut v_seq = 0u64;
    let mut v_next = tokio::time::Instant::now();
    while p_running.load(Ordering::Relaxed) {
        let v_msg = party::telemetry(&p_name, v_tg, v_mt, v_seq, &p_pad, p_binary, &v_payload);
        v_seq += 1;
        match p_tx.try_send(v_msg) {
            Ok(()) => {
                p_stats.sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(v_msg)) => {
                p_stats.stalls.fetch_add(1, Ordering::Relaxed);
                if p_tx.send(v_msg).await.is_err() {
                    break;
                }
                p_stats.sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => break,
        }
        if v_period.is_none() && v_seq % 256 == 0 {
            // unthrottled: never starve the worker thread
            tokio::task::yield_now().await;
        }
        if let Some(v_period) = v_period {
            v_next += v_period;
            let v_now = tokio::time::Instant::now();
            if v_next > v_now {
                tokio::time::sleep_until(v_next).await;
            } else {
                v_next = v_now;
                tokio::task::yield_now().await;
            }
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let v_args = parse_args();
    let (v_host, v_port) = host_port(&v_args.server);
    let v_running = Arc::new(AtomicBool::new(true));
    let v_pad = "x".repeat(v_args.pad);
    let v_mode = if v_args.broadcast { "broadcast" } else { "p2p" };

    // the stub AUTH the comm server dials into (real login path)
    let v_stub = if v_args.local {
        println!("login: local_server_enabled (single account \"1\"/group \"1\")");
        None
    } else {
        let (v_ah, v_ap) = host_port(&v_args.auth_addr);
        println!("auth stub: waiting for the comm server link on {}:{} ...", v_ah, v_ap);
        match auth::AuthStub::listen(&v_ah, &v_ap, std::path::Path::new(&v_args.key), std::path::Path::new(&v_args.cert)).await {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("auth stub failed: {}", e);
                exit(1);
            }
        }
    };

    let mut v_accounts: Vec<(String, Arc<AccountStats>, usize, usize)> = Vec::new();
    let mut v_senders = Vec::new();
    let mut v_readers = Vec::new();
    // tx clones kept alive so every socket writer stays up for the whole run
    let mut _v_handles = Vec::new();

    for v_fleet in &v_args.fleets {
        let v_stats = Arc::new(AccountStats::default());
        // every sender name of this account (for the isolation check)
        let v_own: Arc<HashSet<String>> = Arc::new(
            (0..v_fleet.drones)
                .map(|i| format!("{}{}_u{}", v_args.prefix, v_fleet.account, i))
                .chain((0..v_fleet.gcs).map(|i| format!("{}{}_g{}", v_args.prefix, v_fleet.account, i)))
                .collect(),
        );
        // login + connect every socket in parallel — many-pair runs would
        // take minutes at ~0.5 s per sequential TLS+auth handshake, and the
        // temp key is used well inside its 10 s wait window
        let v_gcs_names: Vec<String> =
            (0..v_fleet.gcs).map(|i| format!("{}{}_g{}", v_args.prefix, v_fleet.account, i)).collect();
        let v_drone_names: Vec<String> =
            (0..v_fleet.drones).map(|i| format!("{}{}_u{}", v_args.prefix, v_fleet.account, i)).collect();
        let v_jobs = v_gcs_names
            .iter()
            .map(|n| (n.clone(), "g"))
            .chain(v_drone_names.iter().map(|n| (n.clone(), "d")))
            .map(|(v_name, v_actor)| {
                let v_stub = v_stub.clone();
                let v_acc = v_fleet.account.clone();
                let v_grp = v_fleet.group.clone();
                let v_host = v_host.clone();
                let v_port = v_port.clone();
                async move {
                    let v_key = match &v_stub {
                        Some(s) => s.login(&v_name, &v_acc, &v_grp, v_actor).await?,
                        None => "x".into(),
                    };
                    party::connect(&v_host, &v_port, &v_name, &v_key, v_actor).await
                }
            })
            .collect::<Vec<_>>();
        let v_parties = futures_util::future::join_all(v_jobs).await;
        let mut v_iter = v_parties.into_iter();
        for v_name in &v_gcs_names {
            let v_party = match v_iter.next().unwrap() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("connect failed for {}: {}", v_name, e);
                    exit(1);
                }
            };
            v_readers.push(tokio::spawn(party::read_loop(
                v_party.source,
                v_party.tx.clone(),
                v_stats.clone(),
                v_own.clone(),
                if v_args.broadcast { None } else { Some(v_name.clone()) },
                false,
                false,
            )));
            _v_handles.push(v_party.tx);
        }
        for (i, v_name) in v_drone_names.iter().enumerate() {
            let v_party = match v_iter.next().unwrap() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("connect failed for {}: {}", v_name, e);
                    exit(1);
                }
            };
            let v_target = v_gcs_names.get(i % v_gcs_names.len()).cloned();
            v_readers.push(tokio::spawn(party::read_loop(
                v_party.source,
                v_party.tx.clone(),
                v_stats.clone(),
                v_own.clone(),
                None,
                !v_args.broadcast,
                v_args.broadcast,
            )));
            v_senders.push(tokio::spawn(drone_sender(
                v_party.tx.clone(),
                v_stats.clone(),
                v_running.clone(),
                v_name.clone(),
                v_target,
                v_args.broadcast,
                v_args.binary,
                v_args.rate,
                v_pad.clone(),
            )));
            _v_handles.push(v_party.tx);
        }
        v_accounts.push((v_fleet.account.clone(), v_stats, v_fleet.drones, v_fleet.gcs));
    }
    let v_n_drones: usize = v_args.fleets.iter().map(|f| f.drones).sum();
    let v_n_gcs: usize = v_args.fleets.iter().map(|f| f.gcs).sum();
    println!(
        "{} sockets up ({} drones, {} GCS, {} accounts); {} mode, {} frames, {} s, rate {}",
        v_n_drones + v_n_gcs,
        v_n_drones,
        v_n_gcs,
        v_args.fleets.len(),
        v_mode,
        if v_args.binary { "binary" } else { "text" },
        v_args.secs,
        if v_args.rate > 0.0 { format!("{}/s per drone", v_args.rate) } else { "max".into() },
    );

    let v_self_pid = std::process::id();
    let v_t0_client = proc_ticks(v_self_pid);
    let v_t0_server = v_args.server_pid.and_then(proc_ticks);
    let v_t0 = std::time::Instant::now();
    tokio::time::sleep(Duration::from_secs_f64(v_args.secs)).await;
    v_running.store(false, Ordering::Relaxed);
    // let echoes and queued deliveries land
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let v_elapsed = v_t0.elapsed().as_secs_f64() - 1.2;
    let v_t1_client = proc_ticks(v_self_pid);
    let v_t1_server = v_args.server_pid.and_then(proc_ticks);
    for v_r in v_readers {
        v_r.abort();
    }
    for v_s in v_senders {
        v_s.abort();
    }

    println!();
    println!("what is counted: sent = frames drones offered; delivered = frames a GCS received;");
    println!("                 rounds = completed round trips (p2p echo — the old THROUGHPUT figure)");
    let mut v_tot_sent = 0u64;
    let mut v_tot_del = 0u64;
    let mut v_tot_rnd = 0u64;
    for (v_acc, v_stats, v_d, v_g) in &v_accounts {
        let v_sent = v_stats.sent.load(Ordering::Relaxed);
        let v_del = v_stats.delivered.load(Ordering::Relaxed);
        let v_rnd = v_stats.rounds.load(Ordering::Relaxed);
        v_tot_sent += v_sent;
        v_tot_del += v_del;
        v_tot_rnd += v_rnd;
        println!(
            "account {} ({}d/{}g): sent={} ({:.0}/s) delivered={} ({:.0}/s) rounds={} ({:.0}/s)",
            v_acc,
            v_d,
            v_g,
            v_sent,
            v_sent as f64 / v_elapsed,
            v_del,
            v_del as f64 / v_elapsed,
            v_rnd,
            v_rnd as f64 / v_elapsed,
        );
        println!(
            "    latency_ms oneway: {} | rtt: {}",
            lat_summary(&v_stats.oneway_ms),
            lat_summary(&v_stats.rtt_ms),
        );
        println!(
            "    violations={} stray={} stalls={} closed={}",
            v_stats.violations.load(Ordering::Relaxed),
            v_stats.stray.load(Ordering::Relaxed),
            v_stats.stalls.load(Ordering::Relaxed),
            v_stats.closed.load(Ordering::Relaxed),
        );
    }
    println!(
        "TOTAL: sent {:.0}/s delivered {:.0}/s rounds {:.0}/s (routed frames, not socket writes)",
        v_tot_sent as f64 / v_elapsed,
        v_tot_del as f64 / v_elapsed,
        v_tot_rnd as f64 / v_elapsed,
    );
    match cpu_percent(v_t0_client, v_t1_client, v_elapsed) {
        Some(v_cpu) => println!("client cpu: {:.0}% of one core (this process)", v_cpu),
        None => println!("client cpu: n/a"),
    }
    match cpu_percent(v_t0_server, v_t1_server, v_elapsed) {
        Some(v_cpu) => println!("server cpu: {:.0}% of one core (pid {})", v_cpu, v_args.server_pid.unwrap_or(0)),
        None => println!("server cpu: n/a (pass --server-pid)"),
    }
}
