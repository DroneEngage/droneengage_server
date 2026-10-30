//! Outbound backpressure guard (outbound_guard.test.js). Bands per socket:
//!   buffered < high_water_bytes   -> send everything
//!   high_water .. hard_cap_bytes  -> send must_deliver, skip latest_wins
//!   buffered >= hard_cap_bytes    -> terminate socket

mod common;

use andruavserver::chat::outbound_guard::{GuardConfig, MsgClass, SendResult};
use andruavserver::config::Config;
use andruavserver::hub::{Client, ConnHandle, Payload, Status, READY_CLOSED};
use serde_json::{json, Value};

const HIGH_WATER: usize = 262144;
const HARD_CAP: usize = 4194304;

fn guard(p_block: Value) -> GuardConfig {
    GuardConfig::from_config(&Config::from_value(json!({ "outbound_guard": p_block }), "."))
}

fn default_guard() -> GuardConfig {
    guard(
        json!({ "enabled": true, "high_water_bytes": HIGH_WATER, "hard_cap_bytes": HARD_CAP, "latest_wins_message_types": [1002, 6502] }),
    )
}

fn socket(p_buffered: usize) -> (Client, tokio::sync::mpsc::UnboundedReceiver<andruavserver::hub::Out>) {
    let (v_handle, v_rx) = ConnHandle::new(None);
    v_handle.set_buffered(p_buffered);
    let mut v_client = Client::new(v_handle, None);
    v_client.status = Some(Status::default());
    (v_client, v_rx)
}

fn msg(p: &str) -> Payload {
    Payload::text(p.to_string())
}

fn count(p_rx: &mut tokio::sync::mpsc::UnboundedReceiver<andruavserver::hub::Out>) -> usize {
    let mut n = 0;
    while p_rx.try_recv().is_ok() {
        n += 1;
    }
    n
}

#[test]
fn unknown_mt_is_must_deliver_configured_mt_is_latest_wins() {
    let g = default_guard();
    assert_eq!(g.message_class(Some(&json!(1002))), MsgClass::LatestWins);
    assert_eq!(g.message_class(Some(&json!(6502))), MsgClass::LatestWins);
    assert_eq!(g.message_class(Some(&json!(1005))), MsgClass::MustDeliver);
    assert_eq!(g.message_class(Some(&json!(999999))), MsgClass::MustDeliver);
    assert_eq!(g.message_class(Some(&json!("1002"))), MsgClass::MustDeliver, "a string mt is not the number");
    assert_eq!(g.message_class(None), MsgClass::MustDeliver);
}

#[test]
fn phase_4a_message_classes_in_the_shipped_server_config() {
    let v_cfg =
        Config::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")), "server.config", &|_| None, &|_| {}).unwrap();
    let g = GuardConfig::from_config(&v_cfg);
    // DE_ROSTER coalesces; event, ack, world and op-control never do
    assert_eq!(g.message_class(Some(&json!(1092))), MsgClass::LatestWins);
    for mt in [1090, 1091, 1093, 1094] {
        assert_eq!(g.message_class(Some(&json!(mt))), MsgClass::MustDeliver, "mt {} must stay must_deliver", mt);
    }
}

#[test]
fn below_high_water_both_classes_are_sent() {
    let g = default_guard();
    let (mut c, mut rx) = socket(HIGH_WATER - 1);
    assert_eq!(g.safe_send(&mut c, &msg("m1"), false, MsgClass::LatestWins), SendResult::Sent);
    assert_eq!(g.safe_send(&mut c, &msg("m2"), false, MsgClass::MustDeliver), SendResult::Sent);
    assert_eq!(count(&mut rx), 2);
}

#[test]
fn between_high_water_and_hard_cap_latest_wins_is_skipped() {
    let g = default_guard();
    let (mut c, mut rx) = socket(HIGH_WATER + 1);
    assert_eq!(g.safe_send(&mut c, &msg("tele"), false, MsgClass::LatestWins), SendResult::Skipped);
    assert_eq!(g.safe_send(&mut c, &msg("cmd"), false, MsgClass::MustDeliver), SendResult::Sent);
    assert_eq!(count(&mut rx), 1);
    assert_eq!(c.status.as_ref().unwrap().dropped, 1);
}

#[test]
fn must_deliver_is_never_skipped_below_the_hard_cap() {
    let g = default_guard();
    let (mut c, mut rx) = socket(HARD_CAP - 1);
    assert_eq!(g.safe_send(&mut c, &msg("cmd"), false, MsgClass::MustDeliver), SendResult::Sent);
    assert_eq!(count(&mut rx), 1);
    assert!(c.handle.is_open());
}

#[test]
fn at_or_above_the_hard_cap_the_socket_is_terminated() {
    let g = default_guard();
    let (mut c, mut rx) = socket(HARD_CAP);
    assert_eq!(g.safe_send(&mut c, &msg("cmd"), false, MsgClass::MustDeliver), SendResult::Terminated);
    assert_eq!(c.handle.ready_state(), READY_CLOSED);
    assert_eq!(count(&mut rx), 0);
    assert_eq!(c.status.as_ref().unwrap().terminated_for_backlog, 1);
    let (mut c2, _rx2) = socket(HARD_CAP + 100000);
    assert_eq!(g.safe_send(&mut c2, &msg("tele"), false, MsgClass::LatestWins), SendResult::Terminated);
    // terminating twice is harmless
    assert_eq!(g.safe_send(&mut c2, &msg("tele"), false, MsgClass::LatestWins), SendResult::Terminated);
}

#[test]
fn a_socket_without_status_still_skips_and_terminates() {
    let g = default_guard();
    let (mut c, _rx) = socket(HIGH_WATER + 1);
    c.status = None;
    assert_eq!(g.safe_send(&mut c, &msg("tele"), false, MsgClass::LatestWins), SendResult::Skipped);
    let (mut c2, _rx2) = socket(HARD_CAP);
    c2.status = None;
    assert_eq!(g.safe_send(&mut c2, &msg("cmd"), false, MsgClass::MustDeliver), SendResult::Terminated);
}

#[test]
fn a_disabled_guard_sends_everything() {
    let g = guard(json!({ "enabled": false }));
    let (mut c, mut rx) = socket(HARD_CAP * 10);
    assert_eq!(g.safe_send(&mut c, &msg("tele"), false, MsgClass::LatestWins), SendResult::Sent);
    assert_eq!(count(&mut rx), 1);
    assert!(c.handle.is_open());
}

#[test]
fn missing_config_uses_the_defaults() {
    let g = GuardConfig::from_config(&Config::from_value(json!({}), "."));
    let (mut c, _rx) = socket(HARD_CAP + 1);
    assert_eq!(g.safe_send(&mut c, &msg("cmd"), false, MsgClass::MustDeliver), SendResult::Terminated);
    // no latest_wins types configured: everything is must_deliver
    assert_eq!(g.message_class(Some(&json!(1002))), MsgClass::MustDeliver);
}

#[tokio::test]
async fn group_broadcast_drops_a_backlogged_socket_from_its_group() {
    let v_srv = common::server(json!({ "outbound_guard": { "hard_cap_bytes": 10 } }));
    let v_d = common::connect(&v_srv, common::Party::new("D1", "d"));
    let mut v_g = common::connect(&v_srv, common::Party::new("G1", "g"));
    v_g.handle.set_buffered(11);
    common::send(&v_srv, &v_d, json!({ "ty": "g", "mt": 1 }));
    assert_eq!(v_g.handle.ready_state(), READY_CLOSED, "terminated");
    let v_hub = v_srv.hub();
    let v_client = v_hub.client(v_g.conn).unwrap();
    assert!(v_client.group.is_none() && v_client.name.is_none(), "orphan removed from its group, name dropped");
    assert_eq!(v_client.status.as_ref().unwrap().terminated_for_backlog, 1);
    drop(v_hub);
    assert!(v_g.sent().is_empty());
}
