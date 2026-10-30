//! Built-in UDP proxy (udp_proxy_account_key.test.js and more): pairs keyed
//! by (account, unit), AUTH `g` blocking per account, datagram forwarding,
//! and the reaper.

mod common;

use andruavserver::comm_server::handle_set_udp_proxy_state;
use common::*;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::net::UdpSocket;

fn cfg() -> Value {
    json!({ "public_host": "comm.test.local", "allow_udpproxy_fixed_port": false })
}

fn request_9008(p_srv: &std::sync::Arc<andruavserver::Server>, p_unit: &FakeClient, p_en: bool) {
    send(
        p_srv,
        p_unit,
        json!({ "ty": "s", "mt": 9008, "ms": { "en": p_en, "socket1": { "address": "0.0.0.0", "port": 0 }, "socket2": { "address": "0.0.0.0", "port": 0 } } }),
    );
}

async fn rows(p_srv: &andruavserver::Server, p_name: &str) -> Vec<Value> {
    p_srv.udp.active_proxies_list(p_srv).await.into_iter().filter(|r| r["name"] == p_name).collect()
}

/// The 9008 reply payload (the built-in proxy answers from a task).
async fn reply(p_unit: &mut FakeClient) -> Value {
    p_unit.wait_frames(1).await.first().map(|f| f.json()["ms"].clone()).expect("9008 reply")
}

async fn g(p_srv: &std::sync::Arc<andruavserver::Server>, p_d: Value) -> Value {
    handle_set_udp_proxy_state(p_srv, &json!({ "c": "g", "d": p_d })).await;
    let v_sent: Value = serde_json::from_str(p_srv.auth.queued().last().unwrap()).unwrap();
    assert_eq!(v_sent["c"], "f");
    v_sent["d"].clone()
}

#[tokio::test]
async fn same_unit_name_in_two_accounts_gets_two_pairs() {
    let v_srv = server(cfg());
    let mut v_a = connect(&v_srv, Party::new("drone1", "d").account(json!("accA")));
    let mut v_b = connect(&v_srv, Party::new("drone1", "d").account(json!("accB")));
    request_9008(&v_srv, &v_a, true);
    let v_ms_a = reply(&mut v_a).await;
    request_9008(&v_srv, &v_b, true);
    let v_ms_b = reply(&mut v_b).await;
    assert_eq!(v_ms_a["en"], true);
    assert_eq!(v_ms_b["en"], true);
    assert_eq!(v_ms_a["socket1"]["address"], "comm.test.local");
    let v_ports: std::collections::HashSet<String> = [&v_ms_a, &v_ms_b]
        .iter()
        .flat_map(|m| [m["socket1"]["port"].to_string(), m["socket2"]["port"].to_string()])
        .collect();
    assert_eq!(v_ports.len(), 4, "B did not reuse A's sockets");
    let mut v_accounts: Vec<Value> = rows(&v_srv, "drone1").await.iter().map(|r| r["account"].clone()).collect();
    v_accounts.sort_by_key(|a| a.to_string());
    assert_eq!(v_accounts, [json!("accA"), json!("accB")]);
    // B goes away: A's pair stays ready; A asking again reuses its pair
    request_9008(&v_srv, &v_b, false);
    assert_eq!(reply(&mut v_b).await["en"], false);
    let v_left = rows(&v_srv, "drone1").await;
    assert_eq!(v_left.len(), 1);
    assert_eq!(v_left[0]["account"], "accA");
    assert_eq!(v_left[0]["ready"], true);
    request_9008(&v_srv, &v_a, true);
    assert_eq!(reply(&mut v_a).await["socket1"]["port"], v_ms_a["socket1"]["port"]);
}

#[tokio::test]
async fn g_blocks_per_account_and_a_name_in_two_accounts_is_ambiguous() {
    let v_srv = server(cfg());
    let mut v_a = connect(&v_srv, Party::new("drone2", "d").account(json!("accA")));
    let mut v_b = connect(&v_srv, Party::new("drone2", "d").account(json!("accB")));
    let mut v_solo = connect(&v_srv, Party::new("solo", "d").account(json!("accA")));
    for u in [&mut v_a, &mut v_b, &mut v_solo] {
        request_9008(&v_srv, u, true);
        reply(u).await;
    }
    let v_reply = g(&v_srv, json!({ "name": "drone2", "account": "accB", "blocked": true, "r": "rq1" })).await;
    assert_eq!(v_reply["applied"], true);
    assert_eq!(v_reply["account"], "accB");
    assert_eq!(v_reply["r"], "rq1");
    let v_blocked: Vec<(Value, Value)> =
        rows(&v_srv, "drone2").await.iter().map(|r| (r["account"].clone(), r["blocked"].clone())).collect();
    assert!(v_blocked.contains(&(json!("accA"), json!(false))) && v_blocked.contains(&(json!("accB"), json!(true))));
    assert_eq!(g(&v_srv, json!({ "name": "drone2", "account": "accC", "blocked": true })).await["applied"], false);
    assert_eq!(g(&v_srv, json!({ "name": "drone2", "blocked": true })).await["applied"], false, "ambiguous");
    assert_eq!(g(&v_srv, json!({ "name": "solo", "blocked": true })).await["applied"], true);
    assert_eq!(rows(&v_srv, "solo").await[0]["blocked"], true);
}

#[tokio::test]
async fn datagrams_flow_between_the_last_senders_and_stop_when_blocked() {
    let v_srv = server(cfg());
    let mut v_unit = connect(&v_srv, Party::new("drone3", "d"));
    request_9008(&v_srv, &v_unit, true);
    let v_ms = reply(&mut v_unit).await;
    let (p1, p2) = (v_ms["socket1"]["port"].as_u64().unwrap() as u16, v_ms["socket2"]["port"].as_u64().unwrap() as u16);
    let v_drone = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let v_gcs = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let fn_recv = |s: &UdpSocket| {
        let mut b = [0u8; 64];
        match s.try_recv(&mut b) {
            Ok(n) => Some(b[..n].to_vec()),
            Err(_) => None,
        }
    };
    v_drone.send_to(b"d1", ("127.0.0.1", p1)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fn_recv(&v_gcs), None, "socket2 has no peer yet");
    v_gcs.send_to(b"g1", ("127.0.0.1", p2)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(v_drone.recv(&mut [0u8; 8]).await.unwrap(), 2);
    v_drone.send_to(b"d2", ("127.0.0.1", p1)).await.unwrap();
    let mut b = [0u8; 8];
    let n = tokio::time::timeout(Duration::from_secs(1), v_gcs.recv(&mut b)).await.unwrap().unwrap();
    assert_eq!(&b[..n], b"d2");
    assert!(v_srv.udp.set_proxy_blocked("drone3", true, None).await);
    v_drone.send_to(b"d3", ("127.0.0.1", p1)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(fn_recv(&v_gcs), None, "blocked");
}

#[tokio::test]
async fn reaper_closes_an_idle_pair_only_after_its_unit_disconnected() {
    let v_srv = server(cfg());
    let mut v_unit = connect(&v_srv, Party::new("drone4", "d"));
    request_9008(&v_srv, &v_unit, true);
    reply(&mut v_unit).await;
    let v_now = andruavserver::util::now_ms();
    // connected unit: kept even when idle (ticks a minute apart)
    for i in 1..=6 {
        v_srv.udp.reap(&v_srv, v_now + i * 60_000 + 1).await;
    }
    assert_eq!(rows(&v_srv, "drone4").await.len(), 1);
    v_srv.hub().delete_active_sender(Some(&json!("drone4")));
    v_srv.udp.reap(&v_srv, v_now + 7 * 60_000).await;
    assert!(rows(&v_srv, "drone4").await.is_empty());
    assert!(v_unit.sent().is_empty(), "the unit is gone, nothing is sent");
}

#[tokio::test]
async fn reaper_recreates_pairs_on_the_same_ports_after_a_suspend() {
    let v_srv = server(cfg());
    let mut v_unit = connect(&v_srv, Party::new("drone5", "d"));
    request_9008(&v_srv, &v_unit, true);
    let v_before = reply(&mut v_unit).await["socket1"]["port"].clone();
    let v_now = andruavserver::util::now_ms();
    v_srv.udp.reap(&v_srv, v_now + 60_000).await;
    v_srv.udp.reap(&v_srv, v_now + 60_000 + 4 * 60_000).await;
    let v_row = &rows(&v_srv, "drone5").await[0];
    assert_eq!(v_row["socket1"]["port"], v_before);
    assert_eq!(v_row["ready"], true);
}

#[tokio::test]
async fn port_values_are_coerced_like_dgram_bind() {
    let v_srv = server(json!({ "public_host": "h", "allow_udpproxy_fixed_port": true }));
    let mut v_unit = connect(&v_srv, Party::new("drone6", "d"));
    send(
        &v_srv,
        &v_unit,
        json!({ "ty": "s", "mt": 9008, "ms": { "en": true, "socket1": { "port": "abc" }, "socket2": { "port": 0 } } }),
    );
    let v_ms = reply(&mut v_unit).await;
    assert_eq!(v_ms["en"], true, "a non-numeric port binds an ephemeral port");
    assert!(v_ms["socket1"]["port"].as_u64().unwrap() > 0);
    // a gcs can't open a proxy
    let mut v_gcs = connect(&v_srv, Party::new("gcs", "g"));
    request_9008(&v_srv, &v_gcs, true);
    settle().await;
    assert!(v_gcs.sent().is_empty());
}
