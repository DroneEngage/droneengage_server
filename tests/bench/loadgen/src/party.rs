//! A benchmark party: one WebSocket login (de_auth frame) plus the send and
//! receive loops of a drone or a GCS.

use droneengage_server_common::net::{frame_bytes, ws_connect, ClientStream, TlsVerify};
use droneengage_server_common::util::{field, js_parse_bytes, now_ms};
use futures_util::stream::SplitStream;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::stats::AccountStats;

/// Frames a drone may queue ahead of the socket writer; when the queue is
/// full the sender stalls (counted — a client-side limit).
const SEND_QUEUE: usize = 512;

/// TYPE_AndruavMessage_LightTelemetry (latest_wins in the shipped config).
pub const MT_TELEMETRY: i64 = 2022;
/// TYPE_AndruavMessage_MAVLINK (binary telemetry, latest_wins).
pub const MT_BINARY: i64 = 6502;
/// The bench echo reply type.
pub const MT_ECHO: i64 = 9999;

/// A connected party: a bounded send queue plus the socket read half.
pub struct Party {
    pub tx: mpsc::Sender<Message>,
    pub source: SplitStream<WebSocketStream<ClientStream>>,
}

/// The telemetry frame (~220 B, like routing_benchmark.py). Binary frames
/// carry the same JSON header, a 0 byte, then p_payload.
pub fn telemetry(
    p_sd: &str,
    p_tg: Option<&str>,
    p_mt: i64,
    p_seq: u64,
    p_pad: &str,
    p_binary: bool,
    p_payload: &[u8],
) -> Message {
    let v_tg = p_tg.map(|t| format!("\"tg\":\"{}\",", t)).unwrap_or_default();
    let v_header = format!(
        "{{\"ty\":\"i\",{}\"sd\":\"{}\",\"mt\":{},\"ms\":{{\"rt\":{},\"sq\":{},\"lat\":30.0444,\"lng\":31.2357,\"alt\":112.5,\"hdg\":270,\"sat\":14,\"fix\":3,\"s1\":88,\"s2\":77,\"b\":\"{}\"}}}}",
        v_tg,
        p_sd,
        p_mt,
        now_ms(),
        p_seq,
        p_pad
    );
    if p_binary {
        let mut v_bytes = v_header.into_bytes();
        v_bytes.push(0);
        v_bytes.extend_from_slice(p_payload);
        Message::Binary(v_bytes.into())
    } else {
        Message::Text(v_header.into())
    }
}

/// The GCS echo: a small one-to-one reply carrying the original rt.
fn echo(p_gcs: &str, p_unit: &[u8], p_rt: &[u8]) -> Message {
    let v_msg = format!(
        "{{\"ty\":\"i\",\"tg\":\"{}\",\"sd\":\"{}\",\"mt\":{},\"ms\":{{\"rt\":{}}}}}",
        String::from_utf8_lossy(p_unit),
        p_gcs,
        MT_ECHO,
        String::from_utf8_lossy(p_rt)
    );
    Message::Text(v_msg.into())
}

/// The header of a frame: for binary everything before the first 0.
fn header_of(p_data: &[u8], p_binary: bool) -> &[u8] {
    if p_binary {
        let v_i = p_data.iter().position(|b| *b == 0).unwrap_or(p_data.len());
        &p_data[..v_i]
    } else {
        p_data
    }
}

/// `"key":<digits>` of a header, without a JSON parse.
fn field_num<'a>(p_data: &'a [u8], p_key: &str) -> Option<&'a [u8]> {
    let v_key = format!("\"{}\":", p_key);
    let v_pos = p_data.windows(v_key.len()).position(|w| w == v_key.as_bytes())?;
    let v_start = v_pos + v_key.len();
    let v_end = p_data[v_start..]
        .iter()
        .position(|b| !b.is_ascii_digit())
        .map(|i| v_start + i)
        .unwrap_or(p_data.len());
    Some(&p_data[v_start..v_end])
}

/// `"key":"<chars>"` of a header, without a JSON parse.
fn field_str<'a>(p_data: &'a [u8], p_key: &str) -> Option<&'a [u8]> {
    let v_key = format!("\"{}\":\"", p_key);
    let v_pos = p_data.windows(v_key.len()).position(|w| w == v_key.as_bytes())?;
    let v_start = v_pos + v_key.len();
    let v_end = p_data[v_start..].iter().position(|b| *b == b'"').map(|i| v_start + i).unwrap_or(p_data.len());
    Some(&p_data[v_start..v_end])
}

/// Connects and logs in with the de_auth frame; the temp key comes from the
/// AUTH stub (real login) or is any non-empty string under
/// local_server_enabled.
pub async fn connect(p_host: &str, p_port: &str, p_name: &str, p_key: &str, p_actor: &str) -> Result<Party, String> {
    let v_ws = ws_connect(p_host, p_port, Some(&TlsVerify::None)).await?;
    let (mut v_sink, mut v_source) = v_ws.split();
    let (v_tx, mut v_rx) = mpsc::channel::<Message>(SEND_QUEUE);
    // writer: queued frames go out in one flush each round (one TLS record
    // and one socket write per burst, not per frame)
    tokio::spawn(async move {
        while let Some(v_msg) = v_rx.recv().await {
            if v_sink.feed(v_msg).await.is_err() {
                break;
            }
            for _ in 0..64 {
                match v_rx.try_recv() {
                    Ok(v_msg) => {
                        if v_sink.feed(v_msg).await.is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
            if v_sink.flush().await.is_err() {
                break;
            }
        }
    });
    let v_auth =
        format!("{{\"ty\":\"s\",\"mt\":\"de_auth\",\"f\":\"{}\",\"s\":\"{}\",\"at\":\"{}\"}}", p_key, p_name, p_actor);
    v_tx.send(Message::Text(v_auth.into())).await.map_err(|_| "auth send failed")?;
    // the ack arrives within the server's AUTH_FRAME_TIMEOUT (8 s)
    let v_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let v_msg = tokio::time::timeout_at(v_deadline, v_source.next())
            .await
            .map_err(|_| format!("de_auth ack timeout for {}", p_name))?;
        let Some(Ok(v_msg)) = v_msg else { return Err(format!("socket closed during de_auth of {}", p_name)) };
        let Some(v_data) = frame_bytes(&v_msg) else { continue };
        let Some(v_parsed) = js_parse_bytes(v_data) else { continue };
        if field(&v_parsed, "mt").and_then(Value::as_str) != Some("de_auth_ack") {
            continue;
        }
        return match field(&v_parsed, "r").and_then(Value::as_str) {
            Some("ok") => Ok(Party { tx: v_tx, source: v_source }),
            other => Err(format!("de_auth rejected {}: {:?}", p_name, other)),
        };
    }
}

/// The read loop shared by drones and GCS sockets. On each inbound frame:
/// count it (delivered at a receiver, or a completed round when the socket
/// is a drone collecting its echoes), check the sender belongs to the
/// account, and sample latency. When p_echo is set, answer every frame with
/// the echo reply (p2p mode).
pub async fn read_loop(
    mut p_source: SplitStream<WebSocketStream<ClientStream>>,
    p_tx: mpsc::Sender<Message>,
    p_stats: Arc<AccountStats>,
    p_own: Arc<HashSet<String>>,
    p_echo: Option<String>,
    p_count_rounds: bool,
    p_stray_only: bool,
) {
    while let Some(v_msg) = p_source.next().await {
        let Ok(v_msg) = v_msg else { break };
        let v_binary = matches!(v_msg, Message::Binary(_));
        let Some(v_data) = frame_bytes(&v_msg) else { continue };
        let v_header = header_of(v_data, v_binary);
        let v_sd = field_str(v_header, "sd");
        if p_stray_only {
            p_stats.stray.fetch_add(1, Ordering::Relaxed);
        } else if p_count_rounds {
            p_stats.rounds.fetch_add(1, Ordering::Relaxed);
        } else {
            p_stats.delivered.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(_v_sd) = v_sd.filter(|s| !p_own.contains(String::from_utf8_lossy(s).as_ref())) {
            p_stats.violations.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(v_rt) = field_num(v_header, "rt")
            .and_then(|r| std::str::from_utf8(r).ok())
            .and_then(|r| r.parse::<i64>().ok())
        {
            if p_count_rounds {
                p_stats.add_rtt(now_ms() - v_rt);
            } else {
                p_stats.add_oneway(now_ms() - v_rt);
            }
        }
        if let (Some(v_gcs), Some(v_sd), Some(v_rt)) = (p_echo.as_deref(), v_sd, field_num(v_header, "rt")) {
            if p_tx.send(echo(v_gcs, v_sd, v_rt)).await.is_err() {
                break;
            }
        }
    }
    p_stats.closed.fetch_add(1, Ordering::Relaxed);
}
