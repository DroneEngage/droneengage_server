//! Server-to-server mesh forwarding (relay.test.js): origin id, the _path
//! loop trail, _gid/_aid injection, binary payload preservation, and which
//! channel (parent / children) a message goes to.

mod common;

use andruavserver::chat::relay::{forward_external_message, forward_message, with_relay_metadata, Source};
use andruavserver::hub::Payload;
use common::*;
use serde_json::{json, Value};

fn text(p: Value) -> Payload {
    Payload::text(p.to_string())
}

#[test]
fn origin_id_is_server_id_or_unknown() {
    assert_eq!(server(json!({ "server_id": "SrvA" })).origin_id(), "SrvA");
    assert_eq!(server(json!({ "server_id": null })).origin_id(), "unknown");
}

#[tokio::test]
async fn forward_message_text_appends_path_and_injects_gid_aid() {
    let v_srv = server(json!({ "server_id": "SrvA", "enable_super_server": true }));
    let mut v_mesh = capture_mesh(&v_srv);
    forward_message(&v_srv, &text(json!({ "ty": "g", "sd": "u1" })), false, Some((json!("g1"), json!("acc1"))));
    let v_sent = v_mesh.to_children.sent();
    assert_eq!(v_sent.len(), 1);
    assert_eq!(v_sent[0]["_path"], json!(["SrvA"]));
    assert_eq!(v_sent[0]["_gid"], "g1");
    assert_eq!(v_sent[0]["_aid"], "acc1");
}

#[test]
fn no_gid_aid_without_a_group_and_existing_path_is_extended() {
    let v_out = with_relay_metadata(&text(json!({ "ty": "g", "sd": "u1" })), "SrvA", None, "x");
    let v: Value = serde_json::from_slice(v_out.as_bytes()).unwrap();
    assert_eq!(v["_path"], json!(["SrvA"]));
    assert!(v.get("_gid").is_none() && v.get("_aid").is_none());
    let v_out = with_relay_metadata(&text(json!({ "ty": "g", "_path": ["SrvA"] })), "SrvB", None, "x");
    let v: Value = serde_json::from_slice(v_out.as_bytes()).unwrap();
    assert_eq!(v["_path"], json!(["SrvA", "SrvB"]));
    // an unparsable message is forwarded unchanged
    assert_eq!(with_relay_metadata(&Payload::text("nope".into()), "S", None, "x").as_bytes(), b"nope");
}

#[test]
fn binary_payload_after_the_null_terminator_is_preserved() {
    let mut v_frame = br#"{"ty":"g","sd":"u1"}"#.to_vec();
    v_frame.push(0);
    v_frame.extend_from_slice(&[9, 8, 7, 6, 5, 0, 1]);
    let v_out = with_relay_metadata(&Payload::binary(v_frame), "SrvA", None, "x");
    assert!(v_out.is_binary());
    let v_bytes = v_out.as_bytes();
    assert_eq!(header(v_bytes)["_path"], json!(["SrvA"]));
    let i = v_bytes.iter().position(|b| *b == 0).unwrap();
    assert_eq!(&v_bytes[i + 1..], &[9, 8, 7, 6, 5, 0, 1]);
}

#[tokio::test]
async fn channels_follow_the_relay_modes() {
    // child mode only
    let v_srv = server(json!({ "server_id": "SrvA", "enable_persistant_relay": true }));
    let mut v_mesh = capture_mesh(&v_srv);
    forward_message(&v_srv, &text(json!({ "ty": "g" })), false, None);
    assert!(v_mesh.to_children.sent().is_empty());
    assert!(v_mesh.to_parent.try_recv().is_ok());
    // both modes
    let v_srv = server(json!({ "server_id": "SrvA", "enable_super_server": true, "enable_persistant_relay": true }));
    let mut v_mesh = capture_mesh(&v_srv);
    forward_message(&v_srv, &text(json!({ "ty": "g" })), false, None);
    assert_eq!(v_mesh.to_children.sent().len(), 1);
    assert!(v_mesh.to_parent.try_recv().is_ok());
}

#[tokio::test]
async fn forward_external_appends_path_only_and_skips_the_source_child() {
    let v_srv = server(json!({ "server_id": "SrvA", "enable_super_server": true, "enable_persistant_relay": true }));
    let mut v_mesh = capture_mesh(&v_srv);
    forward_external_message(&v_srv, &text(json!({ "ty": "g", "sd": "u1" })), false, Some(Source::Parent));
    let v_children = v_mesh.to_children.sent();
    assert_eq!(v_children.len(), 1);
    assert_eq!(v_children[0]["_path"], json!(["SrvA"]));
    assert!(v_children[0].get("_gid").is_none());
    let v_up = v_mesh.to_parent.try_recv().unwrap();
    assert_eq!(header(&v_up.into_data())["_path"], json!(["SrvA"]));
    // the child the message came from does not get it back
    forward_external_message(&v_srv, &text(json!({ "ty": "g" })), false, Some(Source::Child(v_mesh.to_children.conn)));
    assert!(v_mesh.to_children.sent().is_empty());
}

#[tokio::test]
async fn standalone_forwards_nothing_and_broadcast_still_delivers() {
    // GL-0086: no mesh flags -> forward_* returns before any JSON work;
    // local delivery is unaffected
    let v_srv = server(json!({ "server_id": "SrvA" }));
    let mut v_mesh = capture_mesh(&v_srv);
    forward_message(&v_srv, &text(json!({ "ty": "g", "sd": "u1" })), false, Some((json!("g1"), json!("acc1"))));
    forward_external_message(&v_srv, &text(json!({ "ty": "g", "sd": "u1" })), false, None);
    assert!(v_mesh.to_children.sent().is_empty());
    assert!(v_mesh.to_parent.try_recv().is_err());

    // a drone's group broadcast still reaches the GCS on a standalone server
    let v_gcs = connect(&v_srv, Party::new("g1", "g").account(json!("accA")).group(json!("g1")));
    let v_drone = connect(&v_srv, Party::new("u1", "d").account(json!("accA")).group(json!("g1")));
    let mut v_gcs = v_gcs;
    send(&v_srv, &v_drone, json!({ "ty": "i", "sd": "u1", "mt": 2022, "ms": {"x": 1} }));
    let v_got = v_gcs.wait_frames(1).await;
    assert!(!v_got.is_empty(), "broadcast must still reach the GCS");
}
