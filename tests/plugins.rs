//! Command plugin dispatch: plugins run on their own worker, in order, on the
//! routed frame, and cannot hold up or break routing.

mod common;

use andruavserver::chat::connection::on_close;
use andruavserver::chat::plugins::{CommandPlugin, PluginConn, PLUGIN_QUEUE_MAX_BYTES, PLUGIN_QUEUE_MAX_FRAMES};
use common::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

#[derive(Default)]
struct Seen {
    frames: Mutex<Vec<(Vec<u8>, i64, Option<Value>)>>,
    threads: Mutex<Vec<std::thread::ThreadId>>,
    states_dropped: Arc<AtomicUsize>,
}

struct CountsDrop(Arc<AtomicUsize>);
impl Drop for CountsDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl CommandPlugin for Seen {
    fn process_command(&self, p_conn: &mut PluginConn<'_>, p_message: &[u8], p_null_index: i64) {
        p_conn.state.get_or_insert_with(|| Box::new(CountsDrop(self.states_dropped.clone())));
        self.threads.lock().unwrap().push(std::thread::current().id());
        self.frames.lock().unwrap().push((p_message.to_vec(), p_null_index, p_conn.account_id.cloned()));
    }
}

fn rig() -> (Arc<andruavserver::Server>, Arc<Seen>) {
    let v_srv = server(json!({}));
    let v_seen = Arc::new(Seen::default());
    v_srv.plugins.add(v_seen.clone());
    (v_srv, v_seen)
}

fn frame_json(p_frame: &[u8]) -> Value {
    serde_json::from_slice(p_frame).unwrap()
}

#[test]
fn a_plugin_runs_once_per_message_off_the_routing_thread_and_sees_the_injected_p() {
    let (v_srv, v_seen) = rig();
    let v_unit = connect(&v_srv, Party::new("u1", "d").prm(json!(3840)));
    send(&v_srv, &v_unit, json!({ "ty": "g", "mt": 1004, "p": 99 }));
    v_srv.plugins.wait_idle();
    let v_frames = v_seen.frames.lock().unwrap();
    assert_eq!(v_frames.len(), 1);
    assert_eq!(frame_json(&v_frames[0].0)["p"], 3840, "the client's p must not reach the plugin");
    assert_eq!(v_frames[0].1, -1);
    assert_eq!(v_frames[0].2, Some(json!("acc1")));
    assert_ne!(v_seen.threads.lock().unwrap()[0], std::thread::current().id());
}

#[test]
fn a_json_array_reaches_the_plugin_but_no_recipient() {
    let (v_srv, v_seen) = rig();
    let v_unit = connect(&v_srv, Party::new("u1", "d"));
    let mut v_gcs = connect(&v_srv, Party::new("gcs", "g"));
    send(&v_srv, &v_unit, json!([1, 2]));
    v_srv.plugins.wait_idle();
    assert_eq!(frame_json(&v_seen.frames.lock().unwrap()[0].0), json!([1, 2]));
    assert!(v_gcs.frames().is_empty());
}

#[test]
fn frames_of_a_connection_stay_in_order() {
    let (v_srv, v_seen) = rig();
    let v_unit = connect(&v_srv, Party::new("u1", "d"));
    for i in 0..500 {
        send(&v_srv, &v_unit, json!({ "ty": "g", "mt": 1, "n": i }));
    }
    v_srv.plugins.wait_idle();
    let v_order: Vec<i64> =
        v_seen.frames.lock().unwrap().iter().map(|f| frame_json(&f.0)["n"].as_i64().unwrap()).collect();
    assert_eq!(v_order, (0..500).collect::<Vec<i64>>());
}

#[test]
fn a_binary_null_index_points_at_the_terminator_after_the_header_grew() {
    let (v_srv, v_seen) = rig();
    let v_unit = connect(&v_srv, Party::new("u1", "d").prm(json!(4294967295u64)));
    let v_tail = [0x00u8, 0xff, 0x00, 0x7f];
    send_binary(&v_srv, &v_unit, json!({ "ty": "g", "mt": 2, "i": "x" }), &v_tail);
    v_srv.plugins.wait_idle();
    let v_frames = v_seen.frames.lock().unwrap();
    let (v_frame, v_index, _) = &v_frames[0];
    let v_index = *v_index as usize;
    assert_eq!(v_frame[v_index], 0);
    assert_eq!(&v_frame[v_index + 1..], &v_tail);
    assert_eq!(frame_json(&v_frame[..v_index])["p"], 4294967295u64);
}

struct Panics;
impl CommandPlugin for Panics {
    fn process_command(&self, _: &mut PluginConn<'_>, _: &[u8], _: i64) {
        panic!("plugin bug");
    }
}

#[test]
fn a_panicking_plugin_leaves_routing_and_later_plugins_working() {
    let v_srv = server(json!({}));
    v_srv.plugins.add(Arc::new(Panics));
    let v_seen = Arc::new(Seen::default());
    v_srv.plugins.add(v_seen.clone());
    let v_unit = connect(&v_srv, Party::new("u1", "d"));
    let mut v_gcs = connect(&v_srv, Party::new("gcs", "g"));
    for _ in 0..2 {
        send(&v_srv, &v_unit, json!({ "ty": "g", "mt": 1 }));
    }
    v_srv.plugins.wait_idle();
    assert_eq!(v_gcs.frames().len(), 2);
    assert_eq!(v_seen.frames.lock().unwrap().len(), 2);
}

#[test]
fn plugin_state_is_dropped_when_the_socket_closes() {
    let (v_srv, v_seen) = rig();
    let v_unit = connect(&v_srv, Party::new("u1", "d"));
    send(&v_srv, &v_unit, json!({ "ty": "g", "mt": 1 }));
    v_srv.plugins.wait_idle();
    assert_eq!(v_seen.states_dropped.load(Ordering::SeqCst), 0);
    on_close(&v_srv, v_unit.conn);
    v_srv.plugins.wait_idle();
    assert_eq!(v_seen.states_dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn plugin_state_is_dropped_for_a_socket_kicked_out_by_a_newer_login() {
    let (v_srv, v_seen) = rig();
    let v_old = connect(&v_srv, Party::new("u1", "d"));
    send(&v_srv, &v_old, json!({ "ty": "g", "mt": 1 }));
    v_srv.plugins.wait_idle();
    let v_login = v_srv.hub().client(v_old.conn).unwrap().login.clone();
    v_srv.hub().del_member_from_account_by_name(v_login.as_ref(), true);
    assert!(v_srv.hub().client(v_old.conn).unwrap().terminated);
    on_close(&v_srv, v_old.conn);
    v_srv.plugins.wait_idle();
    assert_eq!(v_seen.states_dropped.load(Ordering::SeqCst), 1);
}

/// Blocks on its first frame until released, like a plugin stuck on a disk.
struct Stuck {
    release: Mutex<mpsc::Receiver<()>>,
    done: AtomicUsize,
}
impl CommandPlugin for Stuck {
    fn process_command(&self, _: &mut PluginConn<'_>, _: &[u8], _: i64) {
        let _ = self.release.lock().unwrap().recv();
        self.done.fetch_add(1, Ordering::SeqCst);
    }
}

fn stuck_rig() -> (Arc<andruavserver::Server>, Arc<Stuck>, mpsc::Sender<()>) {
    let v_srv = server(json!({}));
    let (v_tx, v_rx) = mpsc::channel();
    let v_stuck = Arc::new(Stuck { release: Mutex::new(v_rx), done: AtomicUsize::new(0) });
    v_srv.plugins.add(v_stuck.clone());
    (v_srv, v_stuck, v_tx)
}

#[test]
fn a_stuck_plugin_hits_the_frame_limit_counts_drops_and_never_delays_routing() {
    let (v_srv, v_stuck, v_release) = stuck_rig();
    let v_unit = connect(&v_srv, Party::new("u1", "d"));
    let mut v_gcs = connect(&v_srv, Party::new("gcs", "g"));
    let v_extra = 100;
    let v_sent = PLUGIN_QUEUE_MAX_FRAMES + v_extra;
    for _ in 0..v_sent {
        send(&v_srv, &v_unit, json!({ "ty": "g", "mt": 1 }));
    }
    assert_eq!(v_gcs.frames().len(), v_sent, "routing went on while the plugin was stuck");
    let v_dropped = v_srv.plugins.dropped(0) as usize;
    // the worker may or may not have taken the first frame off the queue yet
    assert!((v_extra - 1..=v_extra).contains(&v_dropped), "dropped {}", v_dropped);
    for _ in 0..v_sent {
        let _ = v_release.send(());
    }
    v_srv.plugins.wait_idle();
    assert_eq!(v_stuck.done.load(Ordering::SeqCst) + v_dropped, v_sent, "every frame ran or was counted");
}

#[test]
fn a_stuck_plugin_hits_the_byte_limit() {
    let (v_srv, _v_stuck, v_release) = stuck_rig();
    let v_unit = connect(&v_srv, Party::new("u1", "d"));
    let v_mb = vec![7u8; 1024 * 1024];
    let v_fit = PLUGIN_QUEUE_MAX_BYTES / v_mb.len();
    for _ in 0..v_fit + 10 {
        send_binary(&v_srv, &v_unit, json!({ "ty": "g", "mt": 2 }), &v_mb);
    }
    assert!(v_srv.plugins.dropped(0) >= 9, "dropped {}", v_srv.plugins.dropped(0));
    drop(v_release);
    v_srv.plugins.wait_idle();
}

#[test]
fn no_plugins_means_nothing_is_queued() {
    let v_srv = server(json!({}));
    assert!(v_srv.plugins.is_empty());
    let v_unit = connect(&v_srv, Party::new("u1", "d"));
    let mut v_gcs = connect(&v_srv, Party::new("gcs", "g"));
    send(&v_srv, &v_unit, json!({ "ty": "g", "mt": 1 }));
    assert_eq!(v_gcs.frames().len(), 1);
}
