//! Message routing (js_chat_routing.js): local messages by `ty`/`tg` and
//! sender actor type, permission injection, blocked units, and relayed
//! (external) messages — loop prevention, scoped delivery, one-to-one.

mod common;

use andruavserver::chat::relay::Source;
use andruavserver::chat::routing::parse_external_message;
use common::*;
use serde_json::{json, Value};

fn mts(p_client: &mut FakeClient) -> Vec<Value> {
    p_client.sent().iter().map(|m| m["mt"].clone()).collect()
}

struct Room {
    d1: FakeClient,
    d2: FakeClient,
    g1: FakeClient,
    g2: FakeClient,
    a1: FakeClient,
    other: FakeClient,
}

fn room(p_srv: &std::sync::Arc<andruavserver::Server>) -> Room {
    Room {
        d1: connect(p_srv, Party::new("D1", "d")),
        d2: connect(p_srv, Party::new("D2", "d")),
        g1: connect(p_srv, Party::new("G1", "g").prm(json!(3840))),
        g2: connect(p_srv, Party::new("G2", "g")),
        a1: connect(p_srv, Party::new("A1", "a")),
        // same group name, other account: never sees acc1 traffic
        other: connect(p_srv, Party::new("X1", "g").account(json!("acc2"))),
    }
}

#[tokio::test]
async fn group_messages_go_to_gcs_from_drones_and_to_everyone_from_gcs() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    send(&v_srv, &r.d1, json!({ "ty": "g", "mt": 1 }));
    send(&v_srv, &r.g1, json!({ "ty": "g", "mt": 2 }));
    send(&v_srv, &r.a1, json!({ "ty": "g", "mt": 3 }));
    assert_eq!(mts(&mut r.g1), [json!(1), json!(3)]);
    assert_eq!(mts(&mut r.g2), [json!(1), json!(2), json!(3)]);
    assert_eq!(mts(&mut r.d1), [json!(2)]);
    assert_eq!(mts(&mut r.d2), [json!(2)]);
    assert_eq!(mts(&mut r.a1), [json!(2)]);
    assert!(r.other.sent().is_empty());
}

#[tokio::test]
async fn individual_messages_by_target() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    send(&v_srv, &r.d1, json!({ "ty": "i", "tg": "G1", "mt": 1 }));
    send(&v_srv, &r.d1, json!({ "ty": "i", "tg": "_GCS_", "mt": 2 }));
    send(&v_srv, &r.d1, json!({ "ty": "i", "tg": "_AGN_", "mt": 3 }));
    send(&v_srv, &r.d1, json!({ "ty": "i", "tg": "_GD_", "mt": 4 }));
    send(&v_srv, &r.d1, json!({ "ty": "i", "mt": 5 }));
    send(&v_srv, &r.g1, json!({ "ty": "i", "mt": 6 }));
    send(&v_srv, &r.a1, json!({ "ty": "i", "mt": 7 }));
    send(&v_srv, &r.d1, json!({ "ty": "i", "tg": null, "mt": 8 }));
    send(&v_srv, &r.d1, json!({ "ty": "i", "tg": "X1", "mt": 9 }));
    // an agent without a target reaches nobody
    assert_eq!(mts(&mut r.g1), [json!(1), json!(2), json!(4), json!(5)]);
    assert_eq!(mts(&mut r.g2), [json!(2), json!(4), json!(5), json!(6)]);
    assert_eq!(mts(&mut r.d2), [json!(3), json!(4), json!(6)]);
    assert_eq!(mts(&mut r.d1), [json!(6)]);
    assert_eq!(mts(&mut r.a1), [json!(4), json!(6)]);
    assert!(r.other.sent().is_empty(), "a unit of another account is not reachable by name");
}

#[tokio::test]
async fn the_permission_from_auth_is_injected_and_cannot_be_faked() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    send(&v_srv, &r.g1, json!({ "ty": "i", "tg": "D1", "mt": 1, "p": 4294967295u64, "ms": { "a": 1.0 } }));
    let v = r.d1.sent();
    assert_eq!(v[0], json!({ "ty": "i", "tg": "D1", "mt": 1, "p": 3840, "ms": { "a": 1 } }));
}

#[tokio::test]
async fn binary_messages_keep_their_payload() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    send_binary(&v_srv, &r.d1, json!({ "ty": "i", "tg": "G1", "mt": 6502 }), &[0xfd, 0, 1, 2]);
    match &r.g1.frames()[0] {
        Frame::Binary(h, p) => {
            assert_eq!(h["p"], json!(4294967295u64));
            assert_eq!(p, &vec![0xfd, 0, 1, 2]);
        }
        f => panic!("expected binary, got {:?}", f),
    }
    let v_hub = v_srv.hub();
    let v_status = v_hub.client(r.d1.conn).unwrap().status.clone().unwrap();
    assert_eq!(v_status.btx as usize, r#"{"ty":"i","tg":"G1","mt":6502}"#.len() + 1 + 4);
}

#[tokio::test]
async fn a_blocked_unit_is_silently_dropped() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    assert!(v_srv.hub().set_sender_blocked("D1", true));
    send(&v_srv, &r.d1, json!({ "ty": "g", "mt": 1 }));
    assert!(r.g1.sent().is_empty());
    assert!(!v_srv.hub().set_sender_blocked("NOBODY", true), "unknown units can't be blocked");
    v_srv.hub().set_sender_blocked("D1", false);
    send(&v_srv, &r.d1, json!({ "ty": "g", "mt": 2 }));
    assert_eq!(mts(&mut r.g1), [json!(2)]);
}

#[tokio::test]
async fn a_message_for_an_absent_unit_goes_to_the_mesh_one_for_a_present_unit_does_not() {
    let v_srv = server(json!({ "enable_super_server": true }));
    let r = room(&v_srv);
    let mut v_mesh = capture_mesh(&v_srv);
    send(&v_srv, &r.d1, json!({ "ty": "i", "tg": "G1", "mt": 1 }));
    send(&v_srv, &r.d1, json!({ "ty": "i", "tg": "ELSEWHERE", "mt": 2 }));
    send(&v_srv, &r.d1, json!({ "ty": "g", "mt": 3 }));
    let v_fwd = v_mesh.to_children.sent();
    assert_eq!(v_fwd.iter().map(|m| m["mt"].clone()).collect::<Vec<_>>(), [json!(2), json!(3)]);
    assert_eq!(v_fwd[0]["_path"], json!(["SrvTest"]));
    assert_eq!(v_fwd[0]["_gid"], "g1");
    assert_eq!(v_fwd[0]["_aid"], "acc1");
}

fn external(p_srv: &std::sync::Arc<andruavserver::Server>, p_msg: Value) {
    parse_external_message(p_srv, p_msg.to_string().as_bytes(), true, Some(Source::Parent));
}

#[tokio::test]
async fn external_messages_are_scoped_loop_free_and_relayed_as_binary() {
    let v_srv = server(json!({ "server_id": "SrvB", "enable_super_server": true }));
    let mut r = room(&v_srv);
    let mut v_mesh = capture_mesh(&v_srv);
    // already relayed by this server: dropped
    external(&v_srv, json!({ "ty": "g", "sd": "Z", "mt": 1, "_path": ["SrvB"] }));
    // scoped to acc1/g1
    external(&v_srv, json!({ "ty": "g", "sd": "Z", "mt": 2, "_path": ["SrvA"], "_aid": "acc1", "_gid": "g1" }));
    // unknown account/group: legacy broadcast to every account
    external(&v_srv, json!({ "ty": "i", "tg": "_GCS_", "sd": "Z", "mt": 3, "_aid": "nope", "_gid": "x" }));
    assert_eq!(mts(&mut r.g1), [json!(2), json!(3)]);
    assert_eq!(mts(&mut r.d1), [json!(2)]);
    assert_eq!(mts(&mut r.other), [json!(3)]);
    let v_fwd = v_mesh.to_children.frames();
    assert_eq!(v_fwd.len(), 2, "broadcasts propagate further");
    assert!(matches!(v_fwd[0], Frame::Binary(..)), "relayed messages travel as binary frames");
    assert_eq!(v_fwd[0].json()["_path"], json!(["SrvA", "SrvB"]));
}

#[tokio::test]
async fn external_one_to_one_checks_the_group_and_is_not_propagated() {
    let v_srv = server(json!({ "server_id": "SrvB", "enable_super_server": true }));
    let mut r = room(&v_srv);
    let mut v_mesh = capture_mesh(&v_srv);
    external(&v_srv, json!({ "ty": "i", "tg": "D1", "sd": "Remote", "mt": 1, "_gid": "g1" }));
    external(&v_srv, json!({ "ty": "i", "tg": "D1", "sd": "Remote", "mt": 2, "_gid": "other" }));
    external(&v_srv, json!({ "ty": "i", "tg": "D1", "sd": "Remote", "mt": 3 }));
    // a local sender in the same group passes the check
    external(&v_srv, json!({ "ty": "i", "tg": "D1", "sd": "G1", "mt": 4 }));
    assert_eq!(mts(&mut r.d1), [json!(1), json!(4)]);
    assert!(v_mesh.to_children.sent().is_empty());
}
