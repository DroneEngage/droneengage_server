//! Task and mission commands (tasks.test.js): body parsing, the shared task
//! fields, the storage-not-connected replies, and forwarding to the storage
//! server.

mod common;

use andruavserver::chat::parse_message_body;
use andruavserver::chat::tasks::fill_task_params;
use common::*;
use serde_json::{json, Map, Value};

#[test]
fn message_body_object_unchanged_string_parsed() {
    let v_body = json!({ "ai": "acc@x.com", "mt": 1 });
    assert_eq!(parse_message_body(&json!({ "ms": v_body })), Some(v_body.clone()));
    assert_eq!(parse_message_body(&json!({ "ms": v_body.to_string() })), Some(v_body));
    assert_eq!(parse_message_body(&json!({ "ms": "{bad" })), None, "JSON.parse throws");
    assert_eq!(parse_message_body(&json!({})), None);
}

#[test]
fn fill_task_params_maps_the_shared_fields() {
    let v_mms =
        json!({ "ac": "code", "ai": "acc", "ps": "p1", "gn": "grp", "s": "snd", "r": "rcv", "mt": 7, "ip": true });
    let mut v_params = Map::new();
    fill_task_params(&v_mms, &mut v_params, false, false);
    assert_eq!(
        Value::Object(v_params),
        json!({ "accessCode": "code", "accountID": "acc", "party_sid": "p1", "groupName": "grp", "sender": "snd", "receiver": "rcv", "messageType": 7, "isPermanent": true })
    );
    let mut v_params = Map::new();
    fill_task_params(&json!({ "ai": "acc" }), &mut v_params, false, false);
    assert_eq!(v_params.keys().collect::<Vec<_>>(), ["accountID"]);
    let mut v_with = Map::new();
    fill_task_params(&json!({ "lts": 42, "t": { "a": 1, "b": [2, 3] } }), &mut v_with, true, true);
    assert_eq!(v_with["largerThan_SID"], 42);
    assert_eq!(v_with["task"], r#"{"a":1,"b":[2,3]}"#);
    let mut v_without = Map::new();
    fill_task_params(&json!({ "lts": 42, "t": {} }), &mut v_without, false, false);
    assert!(v_without.is_empty());
}

fn unit(p_srv: &std::sync::Arc<andruavserver::Server>) -> FakeClient {
    connect(p_srv, Party::new("unit1", "d"))
}

#[tokio::test]
async fn offline_storage_replies_not_connected_for_every_task_command() {
    let v_srv = server(json!({}));
    let mut v_unit = unit(&v_srv);
    for mt in [9001, 9002, 9003, 9004] {
        send(&v_srv, &v_unit, json!({ "ty": "s", "mt": mt, "ms": { "ai": "acc@x.com" } }));
        let v = v_unit.sent();
        assert_eq!(v.len(), 1, "mt {}", mt);
        assert_eq!(v[0]["mt"], mt);
        assert_eq!(v[0]["ty"], "i");
        assert_eq!(v[0]["tg"], "unit1");
        assert_eq!(v[0]["sd"], "_SYS_");
        assert!(v[0]["ms"].as_str().unwrap().contains("Storage server not connected (state: dc)"));
    }
    for mt in [9010, 9011, 9012] {
        send(&v_srv, &v_unit, json!({ "ty": "s", "mt": mt, "ms": { "unitId": "u", "missionId": "m" } }));
        let v = v_unit.sent();
        assert_eq!(v[0]["mt"], mt);
        assert_eq!(v[0]["ty"], "s", "mission replies are system messages");
    }
}

#[tokio::test]
async fn task_handlers_ignore_a_missing_or_empty_account_id() {
    let v_srv = server(json!({}));
    let mut v_unit = unit(&v_srv);
    for mt in [9001, 9002, 9003, 9004] {
        send(&v_srv, &v_unit, json!({ "ty": "s", "mt": mt, "ms": { "mt": 1 } }));
        send(&v_srv, &v_unit, json!({ "ty": "s", "mt": mt, "ms": { "ai": "" } }));
        send(&v_srv, &v_unit, json!({ "ty": "s", "mt": mt, "ms": "not json" }));
    }
    assert!(v_unit.sent().is_empty());
}

#[tokio::test]
async fn load_tasks_is_forwarded_and_each_task_answered() {
    let v_srv = server(json!({}));
    let mut v_db = storage_link(&v_srv);
    let mut v_unit = unit(&v_srv);
    send(&v_srv, &v_unit, json!({ "ty": "s", "mt": 9001, "ms": { "ai": "acc@x.com", "mt": 1024 } }));
    let v_req = answer_storage(
        &v_srv,
        &mut v_db,
        json!({ "success": true, "ms": { "tasks": [{ "taskId": "t1", "data": { "a": 1 }, "messageType": 1024 }, { "taskId": "t2", "data": 2 }] } }),
    )
    .await;
    assert_eq!(v_req["mt"], 9001);
    assert_eq!(v_req["ms"], json!({ "unitId": "acc@x.com" }));
    settle().await;
    let v = v_unit.sent();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0]["mt"], 1024);
    assert_eq!(v[0]["ms"], json!({ "a": 1 }));
    assert_eq!(v[0]["sid"], "t1");
    // no messageType: the request's (absent) messageType
    assert!(v[1].get("mt").is_none());
    assert_eq!(v[1]["sid"], "t2");
}

#[tokio::test]
async fn save_tasks_gives_the_task_a_stable_id_and_replies_done() {
    let v_srv = server(json!({}));
    let mut v_db = storage_link(&v_srv);
    let mut v_unit = unit(&v_srv);
    send(&v_srv, &v_unit, json!({ "ty": "s", "mt": 9002, "ms": { "ai": "u", "messageType": 7, "t": { "x": 1 } } }));
    let v_req = answer_storage(&v_srv, &mut v_db, json!({ "success": true })).await;
    let v_task = &v_req["ms"]["tasks"][0];
    assert_eq!(v_task["taskId"], v_task["data"]["id"]);
    assert_eq!(v_task["name"], 7);
    settle().await;
    assert_eq!(v_unit.sent()[0]["ms"], "Done");
    // disable: the task id from mms.t.id; an unsuccessful reply answers nothing
    send(&v_srv, &v_unit, json!({ "ty": "s", "mt": 9004, "ms": { "ai": "u", "t": { "id": "k" } } }));
    let v_req = answer_storage(&v_srv, &mut v_db, json!({ "success": false })).await;
    assert_eq!(v_req["ms"], json!({ "unitId": "u", "taskIds": ["k"] }));
    settle().await;
    assert!(v_unit.sent().is_empty());
}

#[tokio::test]
async fn missions_are_scoped_to_the_login_account_and_forwarded_verbatim() {
    let v_srv = server(json!({}));
    let mut v_db = storage_link(&v_srv);
    let mut v_unit = unit(&v_srv);
    send(
        &v_srv,
        &v_unit,
        json!({ "ty": "s", "mt": 9010, "ms": { "list": true, "kind": "plan", "accountId": "spoof" } }),
    );
    let v_req = answer_storage(&v_srv, &mut v_db, json!({ "success": false, "ms": { "s": "ERROR:quota" } })).await;
    assert_eq!(v_req["ms"], json!({ "accountId": "acc1", "list": true, "kind": "plan" }));
    settle().await;
    assert_eq!(v_unit.sent()[0]["ms"], json!({ "s": "ERROR:quota" }));
    send(&v_srv, &v_unit, json!({ "ty": "s", "mt": 9010, "ms": { "unitId": "u1" } }));
    answer_storage(&v_srv, &mut v_db, json!({ "success": true })).await;
    settle().await;
    assert_eq!(v_unit.sent()[0]["ms"], "Error: storage returned no payload");
}
