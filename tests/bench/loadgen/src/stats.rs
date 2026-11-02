//! Run accounting: per-account counters, latency samples and CPU use read
//! from /proc. "delivered" is counted where a frame is received; "rounds"
//! counts echoes that completed a round trip (the old THROUGHPUT number).

use serde_json::Value;
use std::sync::atomic::AtomicU64;
use std::sync::Mutex;

/// USER_HZ: /proc stat times are always in 100 Hz ticks on Linux.
const TICKS_PER_SEC: f64 = 100.0;

#[derive(Default)]
pub struct AccountStats {
    /// frames the account's drones offered to the server
    pub sent: AtomicU64,
    /// frames received by the account's GCS sockets
    pub delivered: AtomicU64,
    /// echo replies a drone got back (p2p round trips)
    pub rounds: AtomicU64,
    /// frames a receiver saw whose sender is not in its account
    pub violations: AtomicU64,
    /// frames a drone socket received but never should (telemetry goes to GCS only)
    pub stray: AtomicU64,
    /// sends that waited on a full channel (client-side limit indicator)
    pub stalls: AtomicU64,
    /// sockets that closed mid-run (e.g. outbound_guard terminate)
    pub closed: AtomicU64,
    /// one-way receiver samples in ms (same host: send time -> receive time)
    pub oneway_ms: Mutex<Vec<i64>>,
    /// round-trip samples in ms (p2p echo at the drone)
    pub rtt_ms: Mutex<Vec<i64>>,
}

impl AccountStats {
    pub fn add_oneway(&self, p_ms: i64) {
        self.oneway_ms.lock().unwrap().push(p_ms);
    }
    pub fn add_rtt(&self, p_ms: i64) {
        self.rtt_ms.lock().unwrap().push(p_ms);
    }
}

/// utime+stime ticks of a process (all its threads).
pub fn proc_ticks(p_pid: u32) -> Option<u64> {
    let v_stat = std::fs::read_to_string(format!("/proc/{}/stat", p_pid)).ok()?;
    let v_close = v_stat.rfind(')')?;
    let v_parts: Vec<&str> = v_stat[v_close + 2..].split_whitespace().collect();
    // fields after "(comm) ": index 0 = state (field 3); utime is field 14, stime 15
    let v_utime: u64 = v_parts.get(11)?.parse().ok()?;
    let v_stime: u64 = v_parts.get(12)?.parse().ok()?;
    Some(v_utime + v_stime)
}

/// CPU use between two tick samples, in % (100% = one core).
pub fn cpu_percent(p_start: Option<u64>, p_end: Option<u64>, p_secs: f64) -> Option<f64> {
    match (p_start, p_end) {
        (Some(a), Some(b)) if p_secs > 0.0 => Some((b.saturating_sub(a)) as f64 / TICKS_PER_SEC / p_secs * 100.0),
        _ => None,
    }
}

/// Sorts the samples and reports percentiles; empties the vector.
pub fn lat_summary(p_lat: &Mutex<Vec<i64>>) -> String {
    let mut v_lat = std::mem::take(&mut *p_lat.lock().unwrap());
    if v_lat.is_empty() {
        return "n/a".into();
    }
    v_lat.sort_unstable();
    let v_q = |p: f64| v_lat[((v_lat.len() - 1) as f64 * p) as usize];
    format!("p50={} p95={} p99={} max={}", v_q(0.5), v_q(0.95), v_q(0.99), v_lat[v_lat.len() - 1])
}

/// The "r" request id of an s2s reply frame.
pub fn reply_key(p_msg: &Value) -> Option<String> {
    p_msg.get("d").and_then(|d| d.get("r")).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}
