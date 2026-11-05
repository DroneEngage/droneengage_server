//! UDP relay broker (udp_relay_select.test.js, udp_relay_client.test.js,
//! udp_proxy_peer.test.js): relay selection, and 9008/9021 against a real
//! relay process. The relay is the droneengage_udp_proxy binary, found via
//! DE_UDP_RELAY_BIN or in a sibling checkout (droneengage_udp_proxy,
//! prv_droneengage_udp_proxy or droneengage_udp_relay_server, built); the
//! relay tests are skipped when there is none.

mod common;

use andruavserver::s2s::udp_relay_client::{
    select_relay_id, unit_payload, UdpRelayClient, SELECT_FIRST_HEALTHY, SELECT_HASH, SELECT_LEAST_LOADED,
};
use andruavserver::Config;
use common::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

fn ids(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| s.to_string()).collect()
}

fn units(p_n: usize) -> Vec<(String, String)> {
    (0..p_n).map(|i| (format!("acc{}", i % 7), format!("drone{}", i))).collect()
}

#[test]
fn hash_matches_the_javascript_comm_server() {
    // fn_selectRelayId results of the JavaScript server, so JS and Rust comm
    // servers put a unit on the same relay
    let v_expected = [
        ("acc0", "drone0", "R3"),
        ("acc1", "drone1", "R3"),
        ("acc2", "drone2", "R3"),
        ("acc0", "drone3", "R2"),
        ("acc1", "drone4", "R2"),
        ("acc2", "drone5", "R2"),
        ("acc0", "drone6", "R1"),
        ("acc1", "drone7", "R3"),
        ("acc2", "drone8", "R2"),
        ("acc0", "drone9", "R3"),
        ("acc1", "drone10", "R2"),
        ("acc2", "drone11", "R1"),
    ];
    for (a, u, want) in v_expected {
        assert_eq!(
            select_relay_id(a, u, &ids(&["R1", "R2", "R3"]), SELECT_HASH, &HashMap::new()).as_deref(),
            Some(want),
            "{}/{}",
            a,
            u
        );
    }
    assert_eq!(select_relay_id("é", "5", &ids(&["R1", "R2"]), SELECT_HASH, &HashMap::new()).as_deref(), Some("R2"));
    assert_eq!(
        select_relay_id("undefined", "x", &ids(&["R1", "R2", "R3"]), SELECT_HASH, &HashMap::new()).as_deref(),
        Some("R3")
    );
}

#[test]
fn hash_is_stable_spread_and_order_independent() {
    let v_ids = ids(&["R1", "R2", "R3"]);
    let v_first: Vec<_> =
        units(300).iter().map(|(a, u)| select_relay_id(a, u, &v_ids, SELECT_HASH, &HashMap::new())).collect();
    for id in &v_ids {
        let n = v_first.iter().filter(|p| p.as_deref() == Some(id.as_str())).count();
        assert!(n > 50, "{} got {}/300", id, n);
    }
    let v_shuffled: Vec<_> = units(300)
        .iter()
        .map(|(a, u)| select_relay_id(a, u, &ids(&["R3", "R1", "R2"]), SELECT_HASH, &HashMap::new()))
        .collect();
    assert_eq!(v_shuffled, v_first);
}

#[test]
fn removing_one_relay_moves_only_its_units() {
    let v_before: Vec<_> = units(300)
        .iter()
        .map(|(a, u)| select_relay_id(a, u, &ids(&["R1", "R2", "R3"]), SELECT_HASH, &HashMap::new()).unwrap())
        .collect();
    let v_after: Vec<_> = units(300)
        .iter()
        .map(|(a, u)| select_relay_id(a, u, &ids(&["R1", "R3"]), SELECT_HASH, &HashMap::new()).unwrap())
        .collect();
    let mut v_moved = 0;
    for (b, a) in v_before.iter().zip(&v_after) {
        if b == "R2" {
            assert!(a == "R1" || a == "R3");
            v_moved += 1;
        } else {
            assert_eq!(a, b, "a unit not on R2 must not move");
        }
    }
    assert!(v_moved > 50);
}

#[test]
fn first_healthy_and_least_loaded() {
    let fn_loads = |p: &[(&str, f64)]| p.iter().map(|(k, v)| (k.to_string(), *v)).collect::<HashMap<_, _>>();
    assert_eq!(
        select_relay_id("a", "u", &ids(&["R2", "R1"]), SELECT_FIRST_HEALTHY, &HashMap::new()).as_deref(),
        Some("R2")
    );
    assert_eq!(
        select_relay_id(
            "a",
            "u",
            &ids(&["R1", "R2", "R3"]),
            SELECT_LEAST_LOADED,
            &fn_loads(&[("R1", 9.0), ("R2", 3.0), ("R3", 5.0)])
        )
        .as_deref(),
        Some("R2")
    );
    assert_eq!(
        select_relay_id("a", "u", &ids(&["R1", "R2"]), SELECT_LEAST_LOADED, &fn_loads(&[("R1", 4.0), ("R2", 4.0)]))
            .as_deref(),
        Some("R1"),
        "ties in list order"
    );
    assert_eq!(
        select_relay_id("a", "u", &ids(&["R1", "R2"]), SELECT_LEAST_LOADED, &fn_loads(&[("R1", 4.0)])).as_deref(),
        Some("R2"),
        "unknown load is 0"
    );
    assert_eq!(select_relay_id("a", "u", &[], SELECT_HASH, &HashMap::new()), None);
}

#[test]
fn unit_payload_drops_relay_ids_and_zeroes_a_refused_open() {
    assert_eq!(
        unit_payload(
            &json!({ "relay_id": "R1", "tk": "T", "en": true, "socket1": { "address": "r", "port": 1 }, "socket2": { "address": "r", "port": 2 }, "pair_id": "p" })
        ),
        json!({ "tk": "T", "en": true, "socket1": { "address": "r", "port": 1 }, "socket2": { "address": "r", "port": 2 } })
    );
    assert_eq!(
        unit_payload(&json!({ "en": false, "error": "no_ports" })),
        json!({ "en": false, "socket1": { "address": "0.0.0.0", "port": 0 }, "socket2": { "address": "0.0.0.0", "port": 0 } })
    );
}

// ----------------------------------------------------------------------
// against a real relay
// ----------------------------------------------------------------------

fn relay_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("DE_UDP_RELAY_BIN") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let v_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for v_up in ["..", "../.."] {
        for v_name in ["droneengage_udp_proxy", "prv_droneengage_udp_proxy", "droneengage_udp_relay_server"] {
            for v_profile in ["release", "debug"] {
                let p = v_root.join(v_up).join(v_name).join("target").join(v_profile).join("droneengage-udp-proxy");
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

macro_rules! need_relay {
    () => {
        match relay_bin() {
            Some(b) => b,
            None => {
                eprintln!("skipped: no relay binary (set DE_UDP_RELAY_BIN to a built droneengage-udp-proxy)");
                return;
            }
        }
    };
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Relay {
    child: Child,
    _dir: tempfile::TempDir,
    port: u16,
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_relay(p_bin: &PathBuf, p_port: u16, p_public_host: &str) -> Relay {
    start_relay_policy(p_bin, p_port, p_public_host, "allowlist_then_tofu")
}

/// DE-0081: disjoint 20-port udp_port_range per relay process. Deriving
/// the range from `control_port % 100` let two relays spawned close
/// together share one range - a pin packet for one test's pair could
/// then land on the other relay (or its port be handed out twice). The
/// counter is per process, so the band is staggered by pid as well:
/// two cargo invocations running this binary in parallel (gate vs a
/// developer run) no longer offer each other the same bind candidates.
static RELAY_RANGE_INDEX: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

fn start_relay_policy(p_bin: &PathBuf, p_port: u16, p_public_host: &str, p_policy: &str) -> Relay {
    let v_dir = tempfile::tempdir().unwrap();
    let v_slot = (std::process::id() % 64) as u16;
    let v_lo = 40000 + v_slot * 320 + RELAY_RANGE_INDEX.fetch_add(1, std::sync::atomic::Ordering::Relaxed) * 20;
    let v_cfg = json!({
        "server_id": "R", "server_ip": "127.0.0.1", "server_port": p_port, "enable_SSL": false,
        "s2s_cert_enabled": false, "udp_bind_ip": "0.0.0.0", "public_host": p_public_host,
        "udp_port_range": [v_lo, v_lo + 19], "allow_fixed_port": true, "udp_buffer_bytes": 0,
        "gcs_peer_policy": p_policy, "idle_timeout_s": 300, "reaper_interval_s": 60, "owner_grace_s": 120
    });
    let v_file = v_dir.path().join("relay.config");
    std::fs::write(&v_file, v_cfg.to_string()).unwrap();
    let mut v_child = Command::new(p_bin)
        .arg(format!("--config={}", v_file.display()))
        .current_dir(v_dir.path())
        .env("de_s2s_shared_secret", SECRET)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let v_out = v_child.stdout.take().unwrap();
    let (v_tx, v_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for l in std::io::BufReader::new(v_out).lines().map_while(Result::ok) {
            if l.contains("control API") {
                let _ = v_tx.send(());
            }
        }
    });
    v_rx.recv_timeout(Duration::from_secs(10)).expect("relay started");
    Relay { child: v_child, _dir: v_dir, port: p_port }
}

fn relay_server(p_relay_port: u16, p_extra: Value) -> Arc<andruavserver::Server> {
    let mut v_cfg = json!({
        "server_id": "DE_CommSrv_Test", "public_host": "comm.test.local",
        "ignore_auth_server": true,
        "udp_relay": [{ "host": "127.0.0.1", "port": p_relay_port, "id": "R1", "ssl": false }],
        "udp_relay_pending_s": 1
    });
    if let (Value::Object(b), Value::Object(o)) = (&mut v_cfg, p_extra) {
        b.extend(o);
    }
    server(v_cfg)
}

async fn wait_until(p_cond: impl Fn() -> bool, p_ms: u64) -> bool {
    let v_end = tokio::time::Instant::now() + Duration::from_millis(p_ms);
    while tokio::time::Instant::now() < v_end {
        if p_cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    p_cond()
}

fn request_9008(p_srv: &Arc<andruavserver::Server>, p_unit: &FakeClient, p_extra: Value) {
    let mut v_ms = json!({ "en": true, "socket1": { "address": "0.0.0.0", "port": 0 }, "socket2": { "address": "0.0.0.0", "port": 0 } });
    if let (Value::Object(b), Value::Object(o)) = (&mut v_ms, p_extra) {
        b.extend(o);
    }
    send(p_srv, p_unit, json!({ "ty": "s", "mt": 9008, "ms": v_ms }));
}

async fn next_ms(p_client: &mut FakeClient, p_mt: i64) -> Value {
    for _ in 0..20 {
        for f in p_client.wait_frames(1).await {
            if f.json()["mt"] == p_mt {
                return f.json()["ms"].clone();
            }
        }
    }
    panic!("no {} reply", p_mt);
}

async fn peer(p_srv: &Arc<andruavserver::Server>, p_gcs: &mut FakeClient, p_unit: &str, p_act: &str) -> Value {
    send(p_srv, p_gcs, json!({ "ty": "s", "mt": 9021, "ms": { "tg_unit": p_unit, "act": p_act } }));
    next_ms(p_gcs, 9021).await
}

/// DE-0081: one sent packet does not mean the pin landed - a second
/// source sent too soon wins 'first_come' instead of 'same_ip'. Re-sends
/// and polls 9021 'query' until the relay reports p_sock itself pinned
/// - matching the port as well as the IP, so a stray same-IP source
/// cannot satisfy the wait (3 s cap). Re-sends after the pin lands are
/// no-ops (the pinned source only refreshes the heard clock), so the
/// pin's switched push is emitted exactly once.
async fn wait_pinned_gcs(
    p_srv: &Arc<andruavserver::Server>,
    p_gcs: &mut FakeClient,
    p_unit: &str,
    p_sock: &std::net::UdpSocket,
    p_port: u16,
    p_ip: &str,
) {
    let v_src_port = p_sock.local_addr().unwrap().port() as u64;
    for _ in 0..30 {
        let _ = p_sock.send_to(b"hb", ("127.0.0.1", p_port));
        // peer_ms skips 9021 pushes (they carry ms.act; replies never do)
        let v_q = peer_ms(
            p_srv,
            p_gcs,
            json!({ "tg_unit": p_unit, "act": "query" }),
        )
        .await;
        let v_gcs = &v_q["pinned"]["gcs"];
        if v_gcs["ip"] == p_ip && v_gcs["port"].as_u64() == Some(v_src_port) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("pin on {} never showed in 9021 query", p_ip);
}

/// DE-0081: same idea for a waiting connection attempt - re-sends and
/// polls 9021 'query' until p_sock shows as the waiting request (3 s
/// cap), so a dropped first datagram cannot flake a later approve.
/// Extra datagrams from the attempt's own source are no-ops, so the
/// attempt push is emitted exactly once and `others` stays exact.
async fn wait_attempt_gcs(
    p_srv: &Arc<andruavserver::Server>,
    p_gcs: &mut FakeClient,
    p_unit: &str,
    p_sock: &std::net::UdpSocket,
    p_port: u16,
    p_ip: &str,
) {
    let v_src_port = p_sock.local_addr().unwrap().port() as u64;
    for _ in 0..30 {
        let _ = p_sock.send_to(b"hb", ("127.0.0.1", p_port));
        let v_q = peer_ms(
            p_srv,
            p_gcs,
            json!({ "tg_unit": p_unit, "act": "query" }),
        )
        .await;
        let v_att = &v_q["pinned"]["gcs"]["attempt"];
        if v_att["ip"] == p_ip && v_att["port"].as_u64() == Some(v_src_port) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("attempt from {} never showed in 9021 query", p_ip);
}

#[tokio::test]
async fn relay_hosts_the_pair_with_gcs_ips_token_and_9021() {
    let v_bin = need_relay!();
    let v_relay = start_relay(&v_bin, free_port(), "relay.test.local");
    let v_srv = relay_server(v_relay.port, json!({}));
    UdpRelayClient::start(&v_srv);
    assert!(wait_until(|| v_srv.relay.is_connected(), 5000).await, "relay link up");

    let mut v_gcs = connect(&v_srv, Party::new("gcs_a", "g").prm(json!(3840)).ip("10.1.1.1"));
    let mut v_viewer = connect(&v_srv, Party::new("gcs_viewer", "g").prm(json!(0x100)).ip("10.1.1.7"));
    let mut v_stranger = connect(&v_srv, Party::new("gcs_other", "g").account(json!("acc2")).ip("10.2.2.2"));
    let mut v_unit = connect(&v_srv, Party::new("u1", "d").ip("10.9.9.9"));
    request_9008(&v_srv, &v_unit, json!({ "cap": ["tk"] }));
    let v_ms = next_ms(&mut v_unit, 9008).await;
    assert_eq!(v_ms["en"], true);
    assert_eq!(v_ms["socket1"]["address"], "relay.test.local");
    assert_eq!(v_ms["tk"].as_str().map(str::len), Some(22), "the relay's unit token reaches the unit");
    assert!(v_ms.get("pair_id").is_none() && v_ms.get("relay_id").is_none());

    // the GCS IPs of the unit's account+group were allowlisted at open (not
    // the drone's, not another account's); permission doesn't matter there
    let v_q = peer(&v_srv, &mut v_gcs, "u1", "query").await;
    assert_eq!(v_q["pinned"]["gcs"]["allowlist"], json!(["10.1.1.1", "10.1.1.7"]));
    assert_eq!(peer(&v_srv, &mut v_stranger, "u1", "allow").await, json!({ "tg_unit": "u1", "error": "not_allowed" }));
    assert_eq!(
        peer(&v_srv, &mut v_viewer, "u1", "allow").await,
        json!({ "tg_unit": "u1", "error": "permission_denied" })
    );
    assert_eq!(
        peer(&v_srv, &mut v_viewer, "u1", "query").await["pinned"]["gcs"]["mode"],
        "allowlist",
        "query needs no permission"
    );
    assert_eq!(peer(&v_srv, &mut v_gcs, "u1", "explode").await, json!({ "tg_unit": "u1", "error": "bad_request" }));
    // a GCS joining later is allowed right away
    let _v_late = connect(&v_srv, Party::new("gcs_late", "g").ip("10.1.1.3"));
    andruavserver::s2s::udp_relay_client::on_gcs_joined(&v_srv, _v_late.conn);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        peer(&v_srv, &mut v_gcs, "u1", "query").await["pinned"]["gcs"]["allowlist"],
        json!(["10.1.1.1", "10.1.1.7", "10.1.1.3"])
    );
    let v_reset = peer(&v_srv, &mut v_gcs, "u1", "reset").await;
    assert_eq!(v_reset["applied"], true);

    // AUTH e: the relay rows this comm server owns
    let v_rows = v_srv.relay.relay_proxies_list(&v_srv).await;
    assert_eq!(v_rows.len(), 1);
    assert_eq!(v_rows[0]["name"], "u1");
    assert_eq!(v_rows[0]["relay_id"], "R");
    assert_eq!(v_rows[0]["online"], true);
    // the unit's socket closes: unit_offline, the pair is kept
    andruavserver::chat::connection::on_close(&v_srv, v_unit.conn);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let v_rows = v_srv.relay.relay_proxies_list(&v_srv).await;
    assert_eq!(v_rows[0]["online"], false);
}

#[tokio::test]
async fn relay_down_fallback_none_then_en_true_when_it_is_back() {
    let v_bin = need_relay!();
    let v_port = free_port();
    let v_srv = relay_server(v_port, json!({}));
    UdpRelayClient::start(&v_srv);
    let mut v_unit = connect(&v_srv, Party::new("u2", "d"));
    request_9008(&v_srv, &v_unit, json!({}));
    // held for udp_relay_pending_s, then en:false
    let v_ms = next_ms(&mut v_unit, 9008).await;
    assert_eq!(v_ms["en"], false);
    let _v_relay = start_relay(&v_bin, v_port, "relay.back.local");
    // the relay authenticates: an unsolicited 9008 en:true
    let mut v_pushed = Value::Null;
    for _ in 0..20 {
        let v_frames = v_unit.wait_frames(1).await;
        if let Some(f) = v_frames.into_iter().find(|f| f.json()["mt"] == 9008) {
            v_pushed = f.json().clone();
            break;
        }
    }
    assert_eq!(v_pushed["ms"]["en"], true, "{}", v_pushed);
    assert_eq!(v_pushed["ms"]["socket1"]["address"], "relay.back.local");
    assert_eq!(v_pushed["tg"], "u2");
    assert_eq!(v_pushed["sd"], "_SYS_");
}

#[tokio::test]
async fn relay_down_fallback_builtin_is_ignored_and_en_true_when_it_is_back() {
    let v_bin = need_relay!();
    let v_port = free_port();
    let v_srv = relay_server(v_port, json!({ "udp_relay_fallback": "builtin" }));
    let v_warnings = UdpRelayClient::config_warnings(&v_srv.cfg);
    assert!(v_warnings.iter().any(|w| w.contains("udp_relay_fallback is ignored")), "{:?}", v_warnings);
    UdpRelayClient::start(&v_srv);
    let mut v_unit = connect(&v_srv, Party::new("u3", "d"));
    request_9008(&v_srv, &v_unit, json!({}));
    assert_eq!(next_ms(&mut v_unit, 9008).await["en"], false);
    let _v_relay = start_relay(&v_bin, v_port, "relay.back.local");
    let mut v_pushed = Value::Null;
    for _ in 0..20 {
        let v_frames = v_unit.wait_frames(1).await;
        if let Some(f) = v_frames.into_iter().find(|f| f.json()["mt"] == 9008) {
            v_pushed = f.json().clone();
            break;
        }
    }
    assert_eq!(v_pushed["ms"]["en"], true, "{}", v_pushed);
    assert_eq!(v_pushed["ms"]["socket1"]["address"], "relay.back.local");
}

#[tokio::test]
async fn no_relay_configured_means_en_false_and_no_proxies() {
    let v_srv = server(json!({ "public_host": "comm.test.local" }));
    let v_warnings = UdpRelayClient::config_warnings(&v_srv.cfg);
    assert!(v_warnings.iter().any(|w| w.contains("UDP proxy is off")), "{:?}", v_warnings);
    let mut v_unit = connect(&v_srv, Party::new("u4", "d"));
    request_9008(&v_srv, &v_unit, json!({}));
    let v_ms = next_ms(&mut v_unit, 9008).await;
    assert_eq!(v_ms["en"], false);
    assert_eq!(v_ms["socket1"]["port"], 0);
    send(&v_srv, &v_unit, json!({ "ty": "s", "mt": 9008, "ms": { "en": false } }));
    assert_eq!(next_ms(&mut v_unit, 9008).await["en"], false);
    assert!(andruavserver::comm_server::all_udp_proxies(&v_srv).await.is_empty());
}

/// Waits for a pushed 9021 with ms.act == p_act on p_client.
async fn push_ms(p_client: &mut FakeClient, p_act: &str) -> Value {
    for _ in 0..15 {
        for f in p_client.wait_frames(1).await {
            let j = f.json();
            if j["mt"] == 9021 && j["ms"]["act"] == p_act {
                return j["ms"].clone();
            }
        }
    }
    panic!("no 9021 act:{} push", p_act);
}

/// Drains p_client's incoming frames for p_ms and returns them as json.
async fn collect_for(p_client: &mut FakeClient, p_ms: u64) -> Vec<Value> {
    let mut v_out = Vec::new();
    let v_end = tokio::time::Instant::now() + Duration::from_millis(p_ms);
    while let Ok(v_frames) = tokio::time::timeout_at(v_end, p_client.wait_frames(1)).await {
        v_out.extend(v_frames.iter().map(|f| f.json().clone()));
    }
    v_out
}

/// GL-0109: the relay's gcs_attempt/gcs_switched pushes are forwarded to
/// the unit's account+group GCSs holding full control, 9021 'query'
/// returns a waiting request, and an answered request clears everywhere.
#[tokio::test]
async fn gcs_attempt_and_switched_reach_full_control_gcs_and_query_sees_the_request() {
    let v_bin = need_relay!();
    // pure tofu: in allowlist_then_tofu the joined GCSs' IPs make the
    // pair effective allowlist and a stranger drops as gcs_not_allowed,
    // never raising an attempt
    let v_relay = start_relay_policy(&v_bin, free_port(), "relay.test.local", "tofu");
    let v_srv = relay_server(v_relay.port, json!({}));
    UdpRelayClient::start(&v_srv);
    assert!(wait_until(|| v_srv.relay.is_connected(), 5000).await, "relay link up");

    // the unit opens its pair - first source pins, the rest are attempts
    let mut v_unit = connect(&v_srv, Party::new("u1", "d").ip("10.9.9.9"));
    request_9008(&v_srv, &v_unit, json!({}));
    let v_ms = next_ms(&mut v_unit, 9008).await;
    assert_eq!(v_ms["en"], true);
    let v_port = v_ms["socket2"]["port"].as_u64().unwrap() as u16;

    let mut v_gcs = connect(&v_srv, Party::new("gcs_a", "g").prm(json!(3840)).ip("10.1.1.1"));
    let mut v_gcs2 = connect(&v_srv, Party::new("gcs_b", "g").prm(json!(3840)).ip("10.1.1.2"));
    let mut v_viewer = connect(&v_srv, Party::new("gcs_viewer", "g").prm(json!(0x100)).ip("10.1.1.7"));
    let mut v_stranger = connect(&v_srv, Party::new("gcs_other", "g").account(json!("acc2")).ip("10.2.2.2"));

    // a GCS pins, then a same-IP new port moves the lock: gcs_switched
    // with reason 'same_ip' reaches both full-control GCSs. The initial
    // pin itself is a switched/first_come push (SL-0187) - drain it on
    // both GCSs or it stays queued and masks the same_ip push below.
    // The pin query runs on the viewer: a poll on a full-control client
    // would consume the pushed frame while scanning for the reply.
    let v_sock_a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    wait_pinned_gcs(&v_srv, &mut v_viewer, "u1", &v_sock_a, v_port, "127.0.0.1").await;
    for v_g in [&mut v_gcs, &mut v_gcs2] {
        assert_eq!(push_ms(v_g, "switched").await["reason"], "first_come");
    }
    let v_sock_b = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    // wait until the lock is pinned on sock_b itself: the same_ip push
    // is then emitted exactly once (re-sends of a pinned source are
    // no-ops) and cannot be lost to a dropped datagram
    wait_pinned_gcs(&v_srv, &mut v_viewer, "u1", &v_sock_b, v_port, "127.0.0.1").await;
    for v_g in [&mut v_gcs, &mut v_gcs2] {
        let v_push = push_ms(v_g, "switched").await;
        assert_eq!(v_push["tg_unit"], "u1");
        assert_eq!(v_push["reason"], "same_ip");
        assert_eq!(v_push["from_ip"], "127.0.0.1");
    }

    // a stranger knocks: gcs_attempt with its source and the expiry.
    // Wait for the request to register before reading its push, same
    // as the pin wait above.
    let v_sock_c = std::net::UdpSocket::bind("127.0.0.2:0").unwrap();
    wait_attempt_gcs(&v_srv, &mut v_viewer, "u1", &v_sock_c, v_port, "127.0.0.2").await;
    for v_g in [&mut v_gcs, &mut v_gcs2] {
        let v_push = push_ms(v_g, "attempt").await;
        assert_eq!(v_push["tg_unit"], "u1");
        assert_eq!(v_push["from_ip"], "127.0.0.2");
        assert_eq!(v_push["from_port"], v_sock_c.local_addr().unwrap().port() as i64);
        assert!(v_push["until_ms"].as_i64().unwrap_or(0) > 0, "countdown carried");
        assert_eq!(v_push["others"], 0);
    }

    // no permission / another account: nothing
    let v_got = collect_for(&mut v_viewer, 500).await;
    assert!(
        !v_got.iter().any(|j| j["ms"]["act"] == "attempt" || j["ms"]["act"] == "switched"),
        "viewer got a push: {:?}",
        v_got
    );
    let v_got = collect_for(&mut v_stranger, 300).await;
    assert!(!v_got.iter().any(|j| j["ms"]["act"] == "attempt" || j["ms"]["act"] == "switched"));

    // a GCS that connects after the request can query it
    let mut v_late = connect(&v_srv, Party::new("gcs_late", "g").prm(json!(3840)).ip("10.1.1.3"));
    let v_q = peer(&v_srv, &mut v_late, "u1", "query").await;
    assert_eq!(v_q["pinned"]["gcs"]["attempt"]["ip"], "127.0.0.2");
    assert_eq!(v_q["pinned"]["gcs"]["attempt"]["port"], v_sock_c.local_addr().unwrap().port() as i64);

    // one GCS answers (reset clears pin + request): the cleared state
    // reaches the other logged-in GCSs as act:'state'
    let v_r = peer(&v_srv, &mut v_gcs, "u1", "reset").await;
    assert_eq!(v_r["applied"], true);
    let v_push = push_ms(&mut v_gcs2, "state").await;
    assert_eq!(v_push["tg_unit"], "u1");
    assert!(v_push["pinned"]["gcs"].get("attempt").is_none(), "request cleared");
    assert!(v_push["pinned"]["gcs"].get("ip").is_none(), "pin cleared");
}

/// 9021 with arbitrary ms fields - returns the reply ms (pushes carry
/// ms.act and are skipped; replies never do).
async fn peer_ms(p_srv: &Arc<andruavserver::Server>, p_gcs: &mut FakeClient, p_ms: Value) -> Value {
    send(p_srv, p_gcs, json!({ "ty": "s", "mt": 9021, "ms": p_ms }));
    for _ in 0..20 {
        for f in p_gcs.wait_frames(1).await {
            let j = f.json();
            if j["mt"] == 9021 && j["ms"].get("act").is_none() {
                return j["ms"].clone();
            }
        }
    }
    panic!("no 9021 reply");
}

/// GL-0110: act switch/reject/kick/change_port reach the relay with the
/// right fields under the 9021 permission rule, secs is capped, and a
/// dead relay answers relay_unavailable.
#[tokio::test]
async fn gcs_9021_actions_reach_the_relay_under_the_permission_rule() {
    let v_bin = need_relay!();
    let mut v_relay = start_relay_policy(&v_bin, free_port(), "relay.test.local", "tofu");
    let v_srv = relay_server(v_relay.port, json!({}));
    UdpRelayClient::start(&v_srv);
    assert!(wait_until(|| v_srv.relay.is_connected(), 5000).await, "relay link up");

    let mut v_unit = connect(&v_srv, Party::new("u1", "d").ip("10.9.9.9"));
    request_9008(&v_srv, &v_unit, json!({}));
    let v_port = next_ms(&mut v_unit, 9008).await["socket2"]["port"].as_u64().unwrap() as u16;

    let mut v_gcs = connect(&v_srv, Party::new("gcs_a", "g").prm(json!(3840)).ip("10.1.1.1"));
    let mut v_gcs2 = connect(&v_srv, Party::new("gcs_b", "g").prm(json!(3840)).ip("10.1.1.2"));
    let mut v_viewer = connect(&v_srv, Party::new("gcs_viewer", "g").prm(json!(0x100)).ip("10.1.1.7"));
    let mut v_stranger = connect(&v_srv, Party::new("gcs_other", "g").account(json!("acc2")).ip("10.2.2.2"));

    // refusals first: another account, no full control, missing fields
    assert_eq!(
        peer_ms(&v_srv, &mut v_stranger, json!({ "tg_unit": "u1", "act": "kick" })).await,
        json!({ "tg_unit": "u1", "error": "not_allowed" })
    );
    assert_eq!(
        peer_ms(&v_srv, &mut v_viewer, json!({ "tg_unit": "u1", "act": "reject", "ip": "9.9.9.9" })).await,
        json!({ "tg_unit": "u1", "error": "permission_denied" })
    );
    assert_eq!(
        peer_ms(&v_srv, &mut v_gcs, json!({ "tg_unit": "u1", "act": "reject" })).await,
        json!({ "tg_unit": "u1", "error": "bad_request" }),
        "reject needs ip"
    );

    // pin a source, raise a waiting attempt, then approve it - and
    // drain the pin's own switched/first_come push on both GCSs first,
    // or the queued push masks the switched/approved one asserted below.
    // The pin query runs on the viewer for the same reason as above.
    let v_sock_a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    wait_pinned_gcs(&v_srv, &mut v_viewer, "u1", &v_sock_a, v_port, "127.0.0.1").await;
    for v_g in [&mut v_gcs, &mut v_gcs2] {
        assert_eq!(push_ms(v_g, "switched").await["reason"], "first_come");
    }
    let v_sock_c = std::net::UdpSocket::bind("127.0.0.2:0").unwrap();
    // wait for the request to register (re-sent until it shows), so the
    // approve below cannot land before the attempt exists
    wait_attempt_gcs(&v_srv, &mut v_viewer, "u1", &v_sock_c, v_port, "127.0.0.2").await;
    let _ = push_ms(&mut v_gcs, "attempt").await;
    let v_sw = peer_ms(
        &v_srv,
        &mut v_gcs,
        json!({ "tg_unit": "u1", "act": "switch", "ip": "127.0.0.2", "port": v_sock_c.local_addr().unwrap().port() }),
    )
    .await;
    assert_eq!(v_sw["applied"], true, "{}", v_sw);
    assert_eq!(v_sw["pinned"]["gcs"]["ip"], "127.0.0.2", "the lock moved to the approved source");
    assert_eq!(push_ms(&mut v_gcs2, "switched").await["reason"], "approved", "the group's other GCS sees the approval");

    // reject above the cap is reduced to it (the relay echoes the secs)
    let v_rej = peer_ms(
        &v_srv,
        &mut v_gcs,
        json!({ "tg_unit": "u1", "act": "reject", "ip": "127.0.0.9", "secs": 99999 }),
    )
    .await;
    assert_eq!(v_rej["applied"], true, "{}", v_rej);
    let v_secs = v_rej["secs"].as_f64().unwrap_or(0.0);
    assert!(v_secs > 3500.0 && v_secs <= 3600.0, "secs capped at 3600: {}", v_secs);

    // kick frees the pin and rejects the current GCS
    let v_kick = peer_ms(&v_srv, &mut v_gcs, json!({ "tg_unit": "u1", "act": "kick", "secs": 5 })).await;
    assert_eq!(v_kick["applied"], true, "{}", v_kick);
    assert_eq!(push_ms(&mut v_gcs2, "switched").await["reason"], "kicked");
    assert!(v_kick["pinned"]["gcs"].get("ip").is_none(), "kick freed the pin");

    // change_port: the pair moves socket2 and the new port reaches the GCSs
    let v_cp = peer_ms(&v_srv, &mut v_gcs, json!({ "tg_unit": "u1", "act": "change_port" })).await;
    assert_eq!(v_cp["applied"], true, "{}", v_cp);
    let v_new = push_ms(&mut v_gcs2, "changed").await;
    let v_new_port = v_new["socket2"]["port"].as_u64().unwrap() as u16;
    assert_ne!(v_new_port, v_port, "socket2 moved to a fresh port");

    // no relay connected: every action answers relay_unavailable
    let _ = v_relay.child.kill();
    let _ = v_relay.child.wait();
    assert!(wait_until(|| !v_srv.relay.is_connected(), 5000).await, "relay link down");
    for v_act in ["switch", "reject", "kick", "change_port", "reset"] {
        let v_ms = match v_act {
            "switch" => json!({ "tg_unit": "u1", "act": "switch", "ip": "1.2.3.4", "port": 1 }),
            "reject" => json!({ "tg_unit": "u1", "act": "reject", "ip": "1.2.3.4" }),
            _ => json!({ "tg_unit": "u1", "act": v_act }),
        };
        assert_eq!(
            peer_ms(&v_srv, &mut v_gcs, v_ms).await["error"],
            "relay_unavailable",
            "{v_act} with a dead relay"
        );
    }
}

/// GL-0097: a configured relay with no shared secret and no Ed25519 key
/// warns loudly at startup — silently, every drone gets en:false.
#[test]
fn missing_relay_secret_warns_loudly_and_a_secret_file_silences_it() {
    let v_env = std::env::var("de_s2s_shared_secret").ok();
    std::env::remove_var("de_s2s_shared_secret");
    let v_relay = json!([{ "host": "127.0.0.1", "port": 9100, "id": "R1", "ssl": false }]);

    let v_cfg = Config::from_value(json!({ "server_id": "S", "udp_relay": v_relay }), env!("CARGO_MANIFEST_DIR"));
    let v_w = UdpRelayClient::config_warnings(&v_cfg);
    assert!(
        v_w.iter().any(|w| w.contains("de_s2s_shared_secret") && w.contains("s2s_shared_secret.key")),
        "the warning names the file and the env var: {:?}",
        v_w
    );

    // an existing secret file (or the env var, or a private key) silences it
    let v_dir = tempfile::tempdir().unwrap();
    let v_secret = v_dir.path().join("s2s_shared_secret.key");
    std::fs::write(&v_secret, "abc123\n").unwrap();
    let v_cfg = Config::from_value(
        json!({ "server_id": "S", "s2s_shared_secret_file": v_secret.to_str().unwrap(), "udp_relay": v_relay }),
        env!("CARGO_MANIFEST_DIR"),
    );
    assert!(!UdpRelayClient::config_warnings(&v_cfg).iter().any(|w| w.contains("no shared secret")));

    if let Some(v) = v_env {
        std::env::set_var("de_s2s_shared_secret", v);
    }
}
