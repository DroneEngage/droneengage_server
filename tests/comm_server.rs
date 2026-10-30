//! AUTH-facing commands (andruav_comm_server.test.js and more): waiting
//! logins, the login reply, and the dashboard's unit traffic block.

mod common;

use andruavserver::comm_server::{
    add_waiting_account, cancel_login_request_by_sender_id, login_request_reply, on_auth_message,
};
use common::*;
use serde_json::{json, Value};

fn auth_sent(p_srv: &std::sync::Arc<andruavserver::Server>) -> Vec<Value> {
    p_srv.auth.queued().iter().map(|m| serde_json::from_str(m).unwrap()).collect()
}

#[tokio::test]
async fn cancel_login_request_removes_only_the_matching_login() {
    let v_srv = server(json!({ "ignore_auth_server": true }));
    add_waiting_account(&v_srv, "tempkey1".into(), json!({ "r": "req-123" }));
    add_waiting_account(&v_srv, "tempkey2".into(), json!({ "r": "req-other" }));
    cancel_login_request_by_sender_id(&v_srv, Some(&json!("req-no-match")));
    cancel_login_request_by_sender_id(&v_srv, None);
    cancel_login_request_by_sender_id(&v_srv, Some(&json!("req-123")));
    let v_hub = v_srv.hub();
    assert!(!v_hub.waiting.contains_key("tempkey1"));
    assert!(v_hub.waiting.contains_key("tempkey2"), "unrelated login must stay");
}

#[tokio::test]
async fn a_login_request_gets_a_temp_key_and_waits() {
    let v_srv = server(json!({ "public_host": "comm.example", "server_port": 9966 }));
    on_auth_message(&v_srv, br#"{"c":"b","d":{"a":"acc1","b":"g1","r":"req-1","at":"g","prm":3840}}"#);
    let v = auth_sent(&v_srv);
    assert_eq!(v.len(), 1);
    let v_key = v[0]["d"]["f"].as_str().unwrap().to_string();
    assert_eq!(v_key.len(), 32);
    assert_eq!(v[0], json!({ "c": "b", "d": { "r": "req-1", "e": 0, "g": "comm.example", "h": 9966, "f": v_key } }));
    assert_eq!(v_srv.hub().waiting[&v_key]["a"], "acc1");
    // nested d and non-objects are ignored
    on_auth_message(&v_srv, br#"{"c":"b","d":{"d":1}}"#);
    on_auth_message(&v_srv, br#"{"c":"b"}"#);
    on_auth_message(&v_srv, b"garbage");
    assert_eq!(auth_sent(&v_srv).len(), 1);
    assert_eq!(
        login_request_reply(&v_srv, &json!({ "d": {} }))["d"],
        json!({ "e": 0, "g": "comm.example", "h": 9966 })
    );
}

#[tokio::test]
async fn unit_traffic_block_from_the_dashboard() {
    let v_srv = server(json!({}));
    let v_d = connect(&v_srv, Party::new("D1", "d"));
    let mut v_g = connect(&v_srv, Party::new("G1", "g"));
    on_auth_message(&v_srv, br#"{"c":"h","d":{"name":"D1","blocked":true,"r":"x"}}"#);
    send(&v_srv, &v_d, json!({ "ty": "g", "mt": 1 }));
    assert!(v_g.sent().is_empty());
    on_auth_message(&v_srv, br#"{"c":"h","d":{"name":"GHOST","blocked":true}}"#);
    on_auth_message(&v_srv, br#"{"c":"h","d":{"name":"D1","blocked":false}}"#);
    send(&v_srv, &v_d, json!({ "ty": "g", "mt": 2 }));
    assert_eq!(v_g.sent().len(), 1);
    let v = auth_sent(&v_srv);
    assert_eq!(
        v[0],
        json!({ "c": "i", "d": { "units": ["D1"], "name": "D1", "blocked": true, "applied": true, "r": "x" } })
    );
    assert_eq!(v[1]["d"]["applied"], false);
    assert_eq!(v[2]["d"]["units"], json!([]));
}

#[tokio::test]
async fn the_info_card_lists_accounts_and_units() {
    let v_srv = server(json!({ "server_id": "S1", "public_host": "h", "server_port": 1 }));
    connect(&v_srv, Party::new("D1", "d"));
    v_srv.update_auth_server();
    let v = auth_sent(&v_srv);
    assert_eq!(v[0]["c"], "a");
    assert_eq!(v[0]["d"]["accounts"], json!(["acc1"]));
    assert_eq!(
        v[0]["d"]["accountDetails"],
        json!({ "acc1": [{ "unitName": "D1", "actorType": "d", "requestId": "D1", "blocked": false }] })
    );
    assert_eq!(v[0]["d"]["serverId"], "S1");
}
