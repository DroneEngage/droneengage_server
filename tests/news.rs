//! News commands (news.test.js): guards, the forced account scope of the WS
//! write path, and the NewsPush fan-out per scope.

mod common;

use andruavserver::chat::news::on_news_push;
use common::*;
use serde_json::json;

#[tokio::test]
async fn offline_storage_replies_not_connected() {
    let v_srv = server(json!({}));
    let mut v_gcs = connect(&v_srv, Party::new("gcs1", "g"));
    send(&v_srv, &v_gcs, json!({ "ty": "s", "mt": 9015, "ms": {} }));
    send(&v_srv, &v_gcs, json!({ "ty": "s", "mt": 9016, "ms": { "body": "hello" } }));
    send(&v_srv, &v_gcs, json!({ "ty": "s", "mt": 9017, "ms": { "newsId": "n1" } }));
    let v = v_gcs.sent();
    assert_eq!(v.iter().map(|m| m["mt"].clone()).collect::<Vec<_>>(), [json!(9015), json!(9016), json!(9017)]);
    assert!(v.iter().all(|m| m["ms"].as_str().unwrap().contains("Storage server not connected")));
}

#[tokio::test]
async fn guards_missing_body_news_id_or_account() {
    let v_srv = server(json!({}));
    let mut v_gcs = connect(&v_srv, Party::new("gcs1", "g"));
    send(&v_srv, &v_gcs, json!({ "ty": "s", "mt": 9016, "ms": { "body": "" } }));
    send(&v_srv, &v_gcs, json!({ "ty": "s", "mt": 9017, "ms": {} }));
    assert!(v_gcs.sent().is_empty());
    let mut v_no_account = connect(&v_srv, Party::new("gcs2", "g").account(serde_json::Value::Null));
    send(&v_srv, &v_no_account, json!({ "ty": "s", "mt": 9015, "ms": {} }));
    assert!(v_no_account.sent().is_empty());
}

#[tokio::test]
async fn save_news_always_forces_scope_account() {
    let v_srv = server(json!({}));
    let mut v_db = storage_link(&v_srv);
    let mut v_gcs = connect(&v_srv, Party::new("unit1", "g"));
    // trying to sneak scope='global' in is ignored
    send(
        &v_srv,
        &v_gcs,
        json!({ "ty": "s", "mt": 9016, "ms": { "scope": "global", "title": "t", "body": "hello", "priority": 1 } }),
    );
    let v_req = answer_storage(&v_srv, &mut v_db, json!({ "success": true, "ms": { "newsId": "n1" } })).await;
    assert_eq!(
        v_req["ms"],
        json!({ "scope": "account", "accountId": "acc1", "title": "t", "body": "hello", "priority": 1, "authorId": "unit1", "expiresAt": null })
    );
    settle().await;
    let v = v_gcs.sent();
    assert_eq!(v[0]["mt"], 9016);
    assert_eq!(v[0]["ms"], json!({ "s": "OK:save", "newsId": "n1" }));
}

#[tokio::test]
async fn news_push_scopes() {
    let v_srv = server(json!({}));
    let mut v_gcs1 = connect(&v_srv, Party::new("g1", "g"));
    let mut v_gcs2 = connect(&v_srv, Party::new("g2", "g").account(json!("acc2")));
    let mut v_drone = connect(&v_srv, Party::new("d1", "d"));
    on_news_push(&v_srv, Some(&json!({ "news": { "id": "n1", "scope": "global", "body": "hi" } })));
    assert_eq!(v_gcs1.sent().len(), 1);
    assert_eq!(v_gcs2.sent().len(), 1);
    on_news_push(&v_srv, Some(&json!({ "news": { "id": "n2", "scope": "account", "account_id": "acc1" } })));
    let v = v_gcs1.sent();
    assert_eq!(
        v,
        [
            json!({ "ty": "s", "sd": "_SYS_", "mt": 9018, "ms": { "id": "n2", "scope": "account", "account_id": "acc1" } })
        ]
    );
    assert!(v_gcs2.sent().is_empty());
    // a disable/delete with no scope reaches every GCS
    on_news_push(&v_srv, Some(&json!({ "news": { "id": "n1", "disabled": 1 } })));
    assert_eq!(v_gcs1.sent().len() + v_gcs2.sent().len(), 2);
    on_news_push(&v_srv, Some(&json!({})));
    on_news_push(&v_srv, None);
    assert!(v_drone.sent().is_empty(), "news only goes to GCS");
}
