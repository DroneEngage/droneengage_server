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

// ---- routing without a Value: the header scan and the permission splice ----

/// Sends these exact bytes as a text frame.
fn send_raw(p_srv: &std::sync::Arc<andruavserver::Server>, p_from: &FakeClient, p_text: &str) {
    andruavserver::chat::routing::parse_message(p_srv, p_from.conn, p_text.as_bytes(), false);
}

/// What a client received, as text, so duplicate keys stay visible.
fn raw_text(p_client: &mut FakeClient) -> Vec<String> {
    let mut v_out = Vec::new();
    while let Ok(o) = p_client.rx.try_recv() {
        match o {
            andruavserver::hub::Out::Text(t) => v_out.push(t.to_string()),
            andruavserver::hub::Out::Binary(b) => v_out.push(String::from_utf8_lossy(&b).into_owned()),
            andruavserver::hub::Out::Close => {}
        }
    }
    v_out
}

/// How many top-level `p` members a delivered text has, under any spelling.
fn p_count(p_text: &str) -> usize {
    p_text.matches(r#""p":"#).count() + p_text.matches(r#""\u0070""#).count() + p_text.matches(r#""\u0070":"#).count()
}

fn old_frame(p_header: &str, p_prm: Option<Value>) -> String {
    let mut v_jmsg = andruavserver::util::js_parse(p_header).unwrap();
    if v_jmsg.is_object() {
        andruavserver::util::set_field_opt(&mut v_jmsg, "p", p_prm);
    }
    andruavserver::util::js_stringify(&v_jmsg)
}

/// Same value as JS sees it: numbers compare as doubles, key order is ignored.
fn same_as_js(p_a: &Value, p_b: &Value) -> bool {
    match (p_a, p_b) {
        // re-printing a double can move its last bit, so old and new may differ by that
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            a == b || (a - b).abs() <= a.abs().max(b.abs()) * 1e-15
        }
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_as_js(x, y)),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|o| same_as_js(v, o)))
        }
        _ => p_a == p_b,
    }
}

const FRAMES: &[&str] = &[
    r#"{}"#,
    r#" { } "#,
    r#"{"ty":"i","tg":"D1","mt":1,"ms":{"a":1.0,"b":[1e3,12345678901234567890,-0,0.5e-3]}}"#,
    r#"{"p":1}"#,
    r#"{"ty":"g", "p" : 5 , "mt":2}"#,
    r#"{"p":1,"p":2,"\u0070":3,"x":{"p":"nested stays"}}"#,
    "{\n\t\"ty\" : \"s\" ,\r\n\"mt\":9008,\"ms\":{\"en\":true}\n}\n",
    r#"{"\u0074y":"g","t\u0067":"x","ty":"i","tg":"\u00441","sd":"a\"b\\c\/\n\u00e9\ud83d\ude00"}"#,
    r#"{"_aid":"a","_gid":7,"_path":["s1","s2"],"mt":"6502"}"#,
    r#"{"tg":null,"ty":"i"}"#,
    r#"{"tg":[],"mt":null,"ty":5}"#,
    r#"{"tg":["x"],"ty":{"a":1}}"#,
    r#"[1,{"p":2},"x",null,true]"#,
    r#"[]"#,
    r#"null"#,
    r#"true"#,
    r#"12"#,
    r#""str""#,
    r#"{"a":"日本語","é":1}"#,
    r#"{"a":1e308,"b":-1.5E+2,"c":1E-999}"#,
];

fn check_against_serde(p_text: &str) {
    use andruavserver::chat::routing::{scan_header, HeaderKind};
    let v_serde: Result<Value, _> = serde_json::from_str(p_text);
    let v_scan = scan_header(p_text);
    assert_eq!(v_scan.is_some(), v_serde.is_ok(), "validity differs on {:?}: {:?}", p_text, v_serde.err());
    let (Some(v_scan), Ok(v_serde)) = (v_scan, v_serde) else { return };
    match &v_serde {
        Value::Object(o) => {
            assert_eq!(v_scan.kind, HeaderKind::Object, "{:?}", p_text);
            let v_ty = o.get("ty").and_then(Value::as_str);
            assert_eq!(v_scan.routing().as_deref(), v_ty, "ty of {:?}", p_text);
            assert_eq!(v_scan.target().as_ref(), o.get("tg"), "tg of {:?}", p_text);
            assert_eq!(v_scan.message_id().as_ref(), o.get("mt"), "mt of {:?}", p_text);
            assert_eq!(v_scan.sender().as_ref(), o.get("sd"), "sd of {:?}", p_text);
            assert_eq!(v_scan.account_id().as_ref(), o.get("_aid"), "_aid of {:?}", p_text);
            assert_eq!(v_scan.group_id().as_ref(), o.get("_gid"), "_gid of {:?}", p_text);
            assert_eq!(v_scan.path().as_ref(), o.get("_path"), "_path of {:?}", p_text);
        }
        Value::Array(_) => assert_eq!(v_scan.kind, HeaderKind::Array, "{:?}", p_text),
        _ => assert_eq!(v_scan.kind, HeaderKind::Other, "{:?}", p_text),
    }
    for v_prm in [Some(json!(3840)), Some(json!(4294967295u64)), None] {
        let v_new = v_scan.with_permission(v_prm.as_ref());
        let v_new_value: Value = serde_json::from_str(&v_new).unwrap_or_else(|e| panic!("{:?} -> {:?}: {}", p_text, v_new, e));
        let v_old: Value = serde_json::from_str(&old_frame(p_text, v_prm.clone())).unwrap();
        assert!(same_as_js(&v_new_value, &v_old), "{:?}: new {:?} old {:?}", p_text, v_new, v_old);
    }
}

#[test]
fn the_scan_accepts_what_serde_json_accepts_on_known_frames() {
    let mut v_frames: Vec<String> = FRAMES.iter().map(|s| s.to_string()).collect();
    for v_bad in [
        "", " ", "{", "}", "[", "]", "{,}", "[,]", "{\"a\"}", "{\"a\":}", "{\"a\":1,}", "[1,]", "{a:1}", "{1:1}",
        "{\"a\":1}x", "{\"a\":1}{", "01", "-", "1.", ".5", "1e", "1e+", "+1", "-01", "tru", "nul", "truex", "NaN",
        "\"", "\"\\", "\"\\x\"", "\"\\u12\"", "\"\\u12G4\"", "\"\\ud800\"", "\"\\ud800x\"", "\"\\ud800\\n\"",
        "\"\\ud800\\u0041\"", "\"\\udc00\"", "\"a\tb\"", "\"a\nb\"", "1e999", "-1e999", "1e400", "{\"a\":1e999}",
        "[1e309]", "\u{feff}{}", "{\"a\":1,,\"b\":2}", "[1 2]", "{\"a\" 1}", "\u{a0}{}",
    ] {
        v_frames.push(v_bad.to_string());
    }
    v_frames.push("1".repeat(400));
    v_frames.push(format!("0.{}", "1".repeat(400)));
    v_frames.push(format!("{{\"a\":{}}}", "9".repeat(310)));
    for v_depth in [1, 2, 64, 126, 127, 128, 129, 300] {
        v_frames.push(format!("{}1{}", "[".repeat(v_depth), "]".repeat(v_depth)));
        v_frames.push(format!("{}1{}", "{\"a\":".repeat(v_depth), "}".repeat(v_depth)));
        v_frames.push(format!("{{\"tg\":{}1{}}}", "[".repeat(v_depth), "]".repeat(v_depth)));
    }
    for f in &v_frames {
        check_against_serde(f);
    }
}

#[test]
fn the_scan_accepts_what_serde_json_accepts_on_mutated_frames() {
    // a fixed seed, so a failure reproduces
    let mut v_state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut fn_next = move |p_max: usize| {
        v_state ^= v_state << 13;
        v_state ^= v_state >> 7;
        v_state ^= v_state << 17;
        (v_state % p_max as u64) as usize
    };
    let v_tokens = [
        "\"", "\\", ",", ":", "{", "}", "[", "]", " ", "\n", "0", "1", "-", "+", ".", "e", "E", "p", "\\u0070", "\\ud83d",
        "\\ude00", "\\n", "null", "true", "1e999", "\u{e9}", "\t", "ty", "tg", "\"p\":1,", "\"tg\":null,",
    ];
    for _ in 0..200_000 {
        let mut v_chars: Vec<char> = FRAMES[fn_next(FRAMES.len())].chars().collect();
        for _ in 0..=fn_next(3) {
            let v_at = fn_next(v_chars.len() + 1);
            match fn_next(3) {
                0 if v_at < v_chars.len() => {
                    v_chars.remove(v_at);
                }
                1 if v_at < v_chars.len() => {
                    let v_token: Vec<char> = v_tokens[fn_next(v_tokens.len())].chars().collect();
                    v_chars.splice(v_at..v_at + 1, v_token);
                }
                _ => {
                    let v_token: Vec<char> = v_tokens[fn_next(v_tokens.len())].chars().collect();
                    v_chars.splice(v_at..v_at, v_token);
                }
            }
        }
        check_against_serde(&v_chars.iter().collect::<String>());
    }
}

#[tokio::test]
async fn a_duplicate_or_escaped_p_never_reaches_a_receiver() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    for v_frame in [
        r#"{"ty":"i","tg":"D1","p":1,"p":2}"#,
        r#"{"ty":"i","tg":"D1","\u0070":1}"#,
        r#"{"ty":"i","tg":"D1","p":1,"\u0070":2,"p":3,"x":{"p":"inner"}}"#,
        r#"{"\u0070":1,"ty":"i","tg":"D1"}"#,
    ] {
        send_raw(&v_srv, &r.g1, v_frame);
        let v_got = raw_text(&mut r.d1);
        assert_eq!(v_got.len(), 1, "{}", v_frame);
        let v_value: Value = serde_json::from_str(&v_got[0]).unwrap();
        assert_eq!(v_value["p"], 3840, "{}", v_got[0]);
        let v_inner = v_value.get("x").is_some() as usize;
        assert_eq!(p_count(&v_got[0]), 1 + v_inner, "only the injected p at the top level: {}", v_got[0]);
    }
}

#[tokio::test]
async fn with_no_permission_every_p_is_removed() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    let v_none = connect(&v_srv, Party::new("N1", "g").no_prm());
    for v_frame in [
        r#"{"ty":"i","tg":"D1"}"#,
        r#"{"ty":"i","tg":"D1","p":1}"#,
        r#"{"p":1,"p":2,"ty":"i","tg":"D1"}"#,
        r#"{"ty":"i","tg":"D1","\u0070":1}"#,
    ] {
        send_raw(&v_srv, &v_none, v_frame);
        let v_got = raw_text(&mut r.d1);
        assert_eq!(v_got.len(), 1, "{}", v_frame);
        assert_eq!(p_count(&v_got[0]), 0, "{}", v_got[0]);
        assert_eq!(serde_json::from_str::<Value>(&v_got[0]).unwrap(), json!({ "ty": "i", "tg": "D1" }));
    }
}

#[tokio::test]
async fn escaped_and_duplicate_routing_fields_route_like_json_parse() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    // tg "D1" and ty "i", spelled with escapes
    send_raw(&v_srv, &r.g1, r#"{"\u0074y":"\u0069","t\u0067":"\u00441","mt":1}"#);
    assert_eq!(r.d1.frames().len(), 1);
    assert!(r.g2.frames().is_empty());
    // the last duplicate wins: not the broadcast the first ty asks for
    send_raw(&v_srv, &r.g1, r#"{"ty":"g","ty":"i","tg":"G2","tg":"D1","mt":1}"#);
    assert_eq!(r.d1.frames().len(), 1);
    assert!(r.g2.frames().is_empty());
    // an escape in sd passes through as the same value
    send_raw(&v_srv, &r.g1, r#"{"ty":"i","tg":"D1","sd":"a\"\u00e9"}"#);
    assert_eq!(r.d1.sent()[0]["sd"], "a\"é");
}

#[tokio::test]
async fn edge_frames_are_dropped_like_today() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    for v_frame in ["{bad", "null", "12", "\"x\"", "true", r#"{"ty":"i","tg":null}"#, "[1,2]", "{\"a\":1e999}"] {
        send_raw(&v_srv, &r.g1, v_frame);
        assert!(r.d1.frames().is_empty(), "{}", v_frame);
        assert!(r.g2.frames().is_empty(), "{}", v_frame);
    }
    let v_deep = format!("{{\"ty\":\"g\",\"a\":{}1{}}}", "[".repeat(200), "]".repeat(200));
    send_raw(&v_srv, &r.g1, &v_deep);
    assert!(r.d1.frames().is_empty(), "past the depth limit");
}

#[tokio::test]
async fn an_empty_object_gets_the_permission_as_valid_json() {
    let v_srv = server(json!({}));
    let r = room(&v_srv);
    let v_seen = std::sync::Arc::new(Frames::default());
    v_srv.plugins.add(v_seen.clone());
    for v_frame in ["{}", " { } "] {
        send_raw(&v_srv, &r.g1, v_frame);
    }
    v_srv.plugins.wait_idle();
    for f in v_seen.0.lock().unwrap().iter() {
        assert_eq!(serde_json::from_slice::<Value>(f).unwrap(), json!({ "p": 3840 }));
    }
    assert_eq!(v_seen.0.lock().unwrap().len(), 2);
}

#[derive(Default)]
struct Frames(std::sync::Mutex<Vec<Vec<u8>>>);
impl andruavserver::chat::plugins::CommandPlugin for Frames {
    fn process_command(&self, _: &mut andruavserver::chat::plugins::PluginConn<'_>, p_message: &[u8], _: i64) {
        self.0.lock().unwrap().push(p_message.to_vec());
    }
}

#[tokio::test]
async fn a_binary_splice_leaves_the_tail_untouched() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    let v_tail = [0u8, 0xff, 0, 0x22, 0x70, 0x22, 0x3a, 0x31];
    for v_header in [r#"{"ty":"i","tg":"G1","mt":6502}"#, r#"{"ty":"i","tg":"G1","mt":6502,"p":1,"p":2}"#, "{\"ty\":\"i\",\"tg\":\"G1\",\"mt\":6502 }"] {
        let mut v_data = v_header.as_bytes().to_vec();
        v_data.push(0);
        v_data.extend_from_slice(&v_tail);
        andruavserver::chat::routing::parse_message(&v_srv, r.d1.conn, &v_data, true);
        match &r.g1.frames()[0] {
            Frame::Binary(h, p) => {
                assert_eq!(h["p"], json!(4294967295u64), "{}", v_header);
                assert_eq!(p, &v_tail.to_vec(), "{}", v_header);
            }
            f => panic!("expected binary, got {:?}", f),
        }
    }
}

#[tokio::test]
async fn a_binary_header_with_invalid_utf8_reaches_receivers_as_valid_utf8() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    let mut v_data = br#"{"ty":"i","tg":"G1","mt":6502,"note":"a"#.to_vec();
    v_data.push(0xff);
    v_data.extend_from_slice(br#"b"}"#);
    let v_old = old_frame(&String::from_utf8_lossy(&v_data), Some(json!(4294967295u64)));
    v_data.push(0);
    v_data.extend_from_slice(&[1, 2, 3]);
    andruavserver::chat::routing::parse_message(&v_srv, r.d1.conn, &v_data, true);
    let v_out = r.g1.rx.try_recv().unwrap();
    let andruavserver::hub::Out::Binary(v_bytes) = v_out else { panic!("expected binary") };
    let i = v_bytes.iter().position(|b| *b == 0).unwrap();
    let v_text = std::str::from_utf8(&v_bytes[..i]).expect("valid UTF-8");
    assert_eq!(serde_json::from_str::<Value>(v_text).unwrap(), serde_json::from_str::<Value>(&v_old).unwrap());
    assert_eq!(&v_bytes[i + 1..], &[1, 2, 3]);
}

// ---- the hub is not held while a frame is built ----

#[tokio::test]
async fn busy_senders_side_by_side_keep_their_own_order_and_lose_nothing() {
    let v_srv = server(json!({}));
    let mut v_gcs = connect(&v_srv, Party::new("GCS", "g"));
    let v_per_sender: i64 = 500;
    let v_threads: Vec<_> = (0..8)
        .map(|n| {
            let v_srv = v_srv.clone();
            let v_name = format!("D{}", n);
            let v_drone = connect(&v_srv, Party::new(&v_name, "d"));
            std::thread::spawn(move || {
                for i in 0..v_per_sender {
                    let v_frame = format!(r#"{{"ty":"g","mt":1,"sd":"D{}","n":{}}}"#, n, i);
                    andruavserver::chat::routing::parse_message(&v_srv, v_drone.conn, v_frame.as_bytes(), false);
                }
            })
        })
        .collect();
    for t in v_threads {
        t.join().unwrap();
    }
    let v_got = v_gcs.sent();
    assert_eq!(v_got.len() as i64, 8 * v_per_sender, "nothing lost");
    for n in 0..8 {
        let v_name = format!("D{}", n);
        let v_seq: Vec<i64> = v_got.iter().filter(|m| m["sd"] == v_name.as_str()).map(|m| m["n"].as_i64().unwrap()).collect();
        assert_eq!(v_seq, (0..v_per_sender).collect::<Vec<i64>>(), "{} out of order", v_name);
    }
}

#[tokio::test]
async fn a_message_from_a_socket_that_left_its_group_reaches_nobody() {
    let v_srv = server(json!({}));
    let mut r = room(&v_srv);
    let v_login = v_srv.hub().client(r.d1.conn).unwrap().login.clone();
    v_srv.hub().del_member_from_account_by_name(v_login.as_ref(), true);
    send(&v_srv, &r.d1, json!({ "ty": "g", "mt": 1 }));
    assert!(r.g1.frames().is_empty());
    andruavserver::chat::connection::on_close(&v_srv, r.d1.conn);
    send(&v_srv, &r.d1, json!({ "ty": "g", "mt": 2 }));
    assert!(r.g1.frames().is_empty());
}
