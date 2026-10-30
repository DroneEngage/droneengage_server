//! UDP relay broker (udp_relay_select.test.js, udp_relay_client.test.js,
//! udp_proxy_peer.test.js): relay selection, and 9008/9021 against a real
//! relay process. The relay is the droneengage_udp_proxy binary, found via
//! DE_UDP_RELAY_BIN or in a sibling checkout (droneengage_udp_proxy,
//! prv_droneengage_udp_proxy or droneengage_udp_relay_server, built); the
//! relay tests are skipped when there is none.

mod common;

use andruavserver::s2s::udp_relay_client::{
    select_relay_id, unit_payload, SELECT_FIRST_HEALTHY, SELECT_HASH, SELECT_LEAST_LOADED,
};
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
    let v_dir = tempfile::tempdir().unwrap();
    let v_lo = 47000 + (p_port % 100) * 20;
    let v_cfg = json!({
        "server_id": "R", "server_ip": "127.0.0.1", "server_port": p_port, "enable_SSL": false,
        "s2s_cert_enabled": false, "udp_bind_ip": "0.0.0.0", "public_host": p_public_host,
        "udp_port_range": [v_lo, v_lo + 19], "allow_fixed_port": true, "udp_buffer_bytes": 0,
        "gcs_peer_policy": "allowlist_then_tofu", "idle_timeout_s": 300, "reaper_interval_s": 60, "owner_grace_s": 120
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
        "server_id": "DE_CommSrv_Test", "public_host": "comm.test.local", "allow_udpproxy_fixed_port": false,
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

#[tokio::test]
async fn relay_hosts_the_pair_with_gcs_ips_token_and_9021() {
    let v_bin = need_relay!();
    let v_relay = start_relay(&v_bin, free_port(), "relay.test.local");
    let v_srv = relay_server(v_relay.port, json!({}));
    andruavserver::s2s::udp_relay_client::UdpRelayClient::start(&v_srv);
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
    andruavserver::s2s::udp_relay_client::UdpRelayClient::start(&v_srv);
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
async fn relay_down_fallback_builtin_serves_the_unit_locally() {
    let v_srv = relay_server(free_port(), json!({ "udp_relay_fallback": "builtin" }));
    andruavserver::s2s::udp_relay_client::UdpRelayClient::start(&v_srv);
    let mut v_unit = connect(&v_srv, Party::new("u3", "d"));
    request_9008(&v_srv, &v_unit, json!({}));
    let v_ms = next_ms(&mut v_unit, 9008).await;
    assert_eq!(v_ms["en"], true);
    assert_eq!(v_ms["socket1"]["address"], "comm.test.local", "the comm server's own proxy");
    // 9021 for a unit on the built-in proxy: mode open
    let mut v_gcs = connect(&v_srv, Party::new("gcs", "g").prm(json!(3840)));
    assert_eq!(
        peer(&v_srv, &mut v_gcs, "u3", "query").await,
        json!({ "tg_unit": "u3", "pinned": { "gcs": { "mode": "open" } } })
    );
    // en:false goes to the built-in proxy too
    send(&v_srv, &v_unit, json!({ "ty": "s", "mt": 9008, "ms": { "en": false } }));
    assert_eq!(next_ms(&mut v_unit, 9008).await["en"], false);
    assert_eq!(v_srv.udp.pair_count().await, 0);
}
