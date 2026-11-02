//! GL-0087: the party socket writer (chat::connection::conn_writer) batches
//! queued frames into one flush instead of flushing each one. These tests
//! run the real writer over an in-memory duplex WebSocket: a stalled peer
//! makes frames pile up so batches form; a drained peer gets a lone frame
//! immediately, with no timer anywhere.

use andruavserver::chat::connection::{conn_writer, WRITER_FLUSHES};
use andruavserver::hub::{ConnHandle, Out};
use futures_util::StreamExt;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::DuplexStream;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// A server-side conn_writer on a small duplex plus the client-side read
/// half. The small capacity is what makes a backlog possible.
async fn writer_pair(
    p_capacity: usize,
) -> (tokio::task::JoinHandle<()>, WebSocketStream<DuplexStream>, std::sync::Arc<ConnHandle>) {
    let (v_srv, v_cli) = tokio::io::duplex(p_capacity);
    let v_ws = WebSocketStream::from_raw_socket(v_srv, Role::Server, None).await;
    let (v_sink, _v_stream) = futures_util::StreamExt::split(v_ws);
    let (v_handle, v_rx) = ConnHandle::new(None);
    let v_handle2 = v_handle.clone();
    let v_buffered = v_handle.buffered_counter();
    let v_task = tokio::spawn(async move {
        let _v_sink = conn_writer(v_rx, v_sink, v_buffered).await;
    });
    (v_task, WebSocketStream::from_raw_socket(v_cli, Role::Client, None).await, v_handle2)
}

/// WRITER_FLUSHES is process-global, so the tests run one at a time.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn text_frame(p_tag: u64) -> Out {
    Out::Text(format!("{{\"n\":{},\"pad\":\"{}\"}}", p_tag, "x".repeat(150)).into())
}

async fn read_msg(p_ws: &mut WebSocketStream<DuplexStream>) -> Option<Message> {
    p_ws.next().await.and_then(Result::ok)
}

#[tokio::test]
async fn lone_frame_flushes_immediately() {
    let _g = TEST_LOCK.lock().unwrap();
    let (_t, mut v_cli, v_handle) = writer_pair(64 * 1024).await;
    let v_f0 = WRITER_FLUSHES.load(Ordering::Relaxed);
    v_handle.send(text_frame(1));
    // a lone frame must arrive now — no timer waits for a batch to fill
    let v_msg = tokio::time::timeout(Duration::from_millis(500), read_msg(&mut v_cli))
        .await
        .expect("lone frame must be flushed immediately")
        .expect("frame");
    assert!(matches!(v_msg, Message::Text(_)));
    assert_eq!(WRITER_FLUSHES.load(Ordering::Relaxed), v_f0 + 1);
    v_handle.send(text_frame(2));
    tokio::time::timeout(Duration::from_millis(500), read_msg(&mut v_cli)).await.unwrap().unwrap();
    assert_eq!(WRITER_FLUSHES.load(Ordering::Relaxed), v_f0 + 2);
}

#[tokio::test]
async fn backlog_frames_leave_in_few_flushes_in_order() {
    let _g = TEST_LOCK.lock().unwrap();
    // a peer that does not read lets the queue grow: frames leave in
    // batches once draining starts, not one flush each
    let (v_task, mut v_cli, v_handle) = writer_pair(2 * 1024).await;
    const N: u64 = 200;
    for i in 0..N {
        v_handle.send(text_frame(i));
    }
    // let the writer block on the full duplex with frames still queued
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(v_handle.buffered() > 0, "stalled socket must keep bufferedAmount");
    let v_f0 = WRITER_FLUSHES.load(Ordering::Relaxed);
    for i in 0..N {
        let v_msg = tokio::time::timeout(Duration::from_secs(10), read_msg(&mut v_cli))
            .await
            .expect("frame must arrive")
            .expect("frame");
        let v_text = match v_msg {
            Message::Text(t) => t,
            other => panic!("expected text, got {:?}", other),
        };
        assert!(v_text.contains(&format!("\"n\":{}", i)), "order preserved: {}", v_text);
    }
    let v_flushes = WRITER_FLUSHES.load(Ordering::Relaxed) - v_f0;
    assert!(v_flushes > 0 && v_flushes < N / 4, "backlog must batch: {} flushes for {} frames", v_flushes, N);
    // everything the handle counted as queued is written now
    for _ in 0..20 {
        if v_handle.buffered() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(v_handle.buffered(), 0);
    v_task.abort();
}

#[tokio::test]
async fn close_flushes_what_came_before_and_ends_writer() {
    let _g = TEST_LOCK.lock().unwrap();
    let (v_task, mut v_cli, v_handle) = writer_pair(2 * 1024).await;
    for i in 0..10 {
        v_handle.send(text_frame(i));
    }
    v_handle.close();
    for i in 0..10 {
        let v_msg = tokio::time::timeout(Duration::from_secs(5), read_msg(&mut v_cli)).await.unwrap().unwrap();
        assert!(v_msg.to_text().unwrap().contains(&format!("\"n\":{}", i)), "order before close");
    }
    let v_msg = tokio::time::timeout(Duration::from_secs(5), read_msg(&mut v_cli)).await.unwrap().unwrap();
    assert!(matches!(v_msg, Message::Close(_)), "close arrives after the queued frames");
    let _ = tokio::time::timeout(Duration::from_secs(5), v_task).await;
}

#[tokio::test]
async fn buffered_falls_only_after_flush() {
    let _g = TEST_LOCK.lock().unwrap();
    // while the peer stalls, bufferedAmount must keep the unwritten bytes —
    // outbound_guard reads it for high-water and hard-cap decisions
    let (v_task, mut v_cli, v_handle) = writer_pair(1024).await;
    for i in 0..30 {
        v_handle.send(text_frame(i));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let v_stalled = v_handle.buffered();
    assert!(v_stalled > 0, "unwritten backlog must stay counted, got {}", v_stalled);
    // it must not drop on its own while nothing is written
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(v_handle.buffered(), v_stalled, "bufferedAmount falls only after a real flush");
    // drain: buffered falls to zero as flushes land
    let mut v_seen = 0;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_secs(5), read_msg(&mut v_cli)).await {
        v_seen += 1;
        if v_seen >= 30 {
            break;
        }
    }
    assert_eq!(v_seen, 30);
    for _ in 0..20 {
        if v_handle.buffered() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(v_handle.buffered(), 0);
    v_task.abort();
}
