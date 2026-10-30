//! Outbound backpressure guard (js_outbound_guard.js).
//!
//! Wraps socket sends with a per-socket bufferedAmount check so that one slow
//! client cannot make the server queue unlimited data. Messages are
//! classified as:
//!   - latest_wins: stale telemetry is worthless; skipped when the target
//!     socket is above the high-water mark.
//!   - must_deliver: commands, acks, errors, system messages; always sent
//!     until the hard cap.
//!
//! A socket above the hard cap is terminated — the client reconnects fresh.
//!
//! Config (server.config, "outbound_guard" block; defaults apply if missing):
//!   enabled                    master switch (false restores old behaviour)
//!   high_water_bytes           above this, latest_wins messages are skipped
//!   hard_cap_bytes             above this, the socket is terminated
//!   latest_wins_message_types  numeric mt values classified as latest_wins

use serde_json::{Map, Value};

use crate::config::Config;
use crate::hub::{Client, Payload};
use crate::util::{is_true, js_number, js_strict_eq};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgClass {
    LatestWins,
    MustDeliver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResult {
    Sent,
    Skipped,
    Terminated,
}

#[derive(Debug, Clone)]
pub struct GuardConfig {
    pub enabled: bool,
    pub high_water_bytes: f64,
    pub hard_cap_bytes: f64,
    pub latest_wins_message_types: Vec<Value>,
}

impl GuardConfig {
    /// Defaults merged with the outbound_guard block (Object.assign).
    pub fn from_config(p_config: &Config) -> GuardConfig {
        let mut v_cfg = Map::new();
        v_cfg.insert("enabled".into(), Value::Bool(true));
        v_cfg.insert("high_water_bytes".into(), Value::from(262144));
        v_cfg.insert("hard_cap_bytes".into(), Value::from(4194304));
        v_cfg.insert("latest_wins_message_types".into(), Value::Array(Vec::new()));
        if let Some(Value::Object(v_user)) = p_config.get("outbound_guard") {
            for (k, v) in v_user {
                v_cfg.insert(k.clone(), v.clone());
            }
        }
        GuardConfig {
            enabled: is_true(v_cfg.get("enabled")),
            high_water_bytes: js_number(v_cfg.get("high_water_bytes")),
            hard_cap_bytes: js_number(v_cfg.get("hard_cap_bytes")),
            latest_wins_message_types: match v_cfg.get("latest_wins_message_types") {
                Some(Value::Array(a)) => a.clone(),
                _ => Vec::new(),
            },
        }
    }

    /// The delivery class of a message type. Unknown types are must_deliver.
    pub fn message_class(&self, p_mt: Option<&Value>) -> MsgClass {
        if p_mt.is_some() && self.latest_wins_message_types.iter().any(|v| js_strict_eq(Some(v), p_mt)) {
            MsgClass::LatestWins
        } else {
            MsgClass::MustDeliver
        }
    }

    /// Sends p_payload to p_client honoring the backpressure bands:
    ///   buffered < high_water              -> send
    ///   high_water <= buffered < hard_cap  -> send must_deliver, skip latest_wins
    ///   buffered >= hard_cap               -> terminate socket
    pub fn safe_send(
        &self,
        p_client: &mut Client,
        p_payload: &Payload,
        p_binary: bool,
        p_class: MsgClass,
    ) -> SendResult {
        if !self.enabled {
            p_client.handle.send(p_payload.with_binary(p_binary));
            return SendResult::Sent;
        }
        let v_buffered = p_client.handle.buffered() as f64;
        if v_buffered >= self.hard_cap_bytes {
            if let Some(s) = p_client.status.as_mut() {
                s.terminated_for_backlog += 1;
            }
            p_client.handle.terminate();
            println!(
                "outbound_guard: terminated socket '{}' bufferedAmount={}",
                p_client.name_key(),
                p_client.handle.buffered()
            );
            return SendResult::Terminated;
        }
        if v_buffered >= self.high_water_bytes && p_class == MsgClass::LatestWins {
            if let Some(s) = p_client.status.as_mut() {
                s.dropped += 1;
            }
            return SendResult::Skipped;
        }
        p_client.handle.send(p_payload.with_binary(p_binary));
        SendResult::Sent
    }
}
