//! Server-to-server mesh forwarding (js_chat_relay.js). See
//! wiki/MessagePropagation.md.
//!
//! A message forwarded to the parent (super) server or to the children gets
//! this server appended to its `_path` (loop prevention); a LOCAL sender's
//! message also gets `_gid`/`_aid`, the sender's group and account, for
//! scoped delivery on the receiving server. A binary message (JSON header,
//! a 0 byte, binary payload) keeps its payload after the header.

use serde_json::Value;
use std::sync::Arc;

use crate::hub::Payload;
use crate::server::Server;
use crate::util::{js_parse_bytes, js_stringify, set_field};

/// Where an external message came from (excluded when it is forwarded on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// a child of this super server (connection id)
    Child(u64),
    /// the parent this server is a child of
    Parent,
}

/// Appends p_origin to `_path` and, with p_group, sets `_gid`/`_aid`.
/// Returns the original message (logging p_what) when the header can't be
/// parsed.
pub fn with_relay_metadata(p_msg: &Payload, p_origin: &str, p_group: Option<(Value, Value)>, p_what: &str) -> Payload {
    let v_bytes = p_msg.as_bytes();
    let (v_json, v_tail): (&[u8], &[u8]) = if p_msg.is_binary() {
        match v_bytes.iter().position(|b| *b == 0) {
            Some(i) => (&v_bytes[..i], &v_bytes[i..]),
            None => (v_bytes, &[]),
        }
    } else {
        (v_bytes, &[])
    };
    let mut v_jmsg = match js_parse_bytes(v_json) {
        Some(v @ Value::Object(_)) => v,
        Some(Value::Array(a)) => {
            // properties set on an array are dropped by JSON.stringify
            let v_text = js_stringify(&Value::Array(a));
            return if p_msg.is_binary() {
                let mut v_out = v_text.into_bytes();
                v_out.extend_from_slice(v_tail);
                Payload::binary(v_out)
            } else {
                Payload::text(v_text)
            };
        }
        _ => {
            println!("{}: Unexpected token in JSON", p_what);
            return p_msg.clone();
        }
    };
    let mut v_path = match v_jmsg.get("_path") {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    v_path.push(Value::String(p_origin.to_string()));
    set_field(&mut v_jmsg, "_path", Value::Array(v_path));
    if let Some((v_gid, v_aid)) = p_group {
        set_field(&mut v_jmsg, "_gid", v_gid);
        set_field(&mut v_jmsg, "_aid", v_aid);
    }
    let v_text = js_stringify(&v_jmsg);
    if p_msg.is_binary() {
        let mut v_out = v_text.into_bytes();
        v_out.extend_from_slice(v_tail);
        Payload::binary(v_out)
    } else {
        Payload::text(v_text)
    }
}

/// forwardMessage: a local client's message to the relay tree. p_group is
/// the sender's (group id, account id) when it is in a group. With no mesh
/// link the message goes nowhere — the flags are checked before the JSON
/// round-trip so a standalone server pays nothing here.
pub fn forward_message(p_server: &Arc<Server>, p_msg: &Payload, p_binary: bool, p_group: Option<(Value, Value)>) {
    let v_super = p_server.cfg.is_true("enable_super_server");
    let v_relay = p_server.cfg.is_true("enable_persistant_relay");
    if !v_super && !v_relay {
        return;
    }
    let v_msg =
        with_relay_metadata(p_msg, &p_server.origin_id(), p_group, "Failed to parse message for relay metadata");
    if v_super {
        p_server.parent.forward_message(&v_msg, p_binary, None);
    }
    if v_relay {
        p_server.child.forward_message(&v_msg, p_binary);
    }
}

/// forwardExternalMessage: a relayed broadcast propagates further in the
/// tree (Child A -> Parent -> Child B/C + Grandparent), never back to its
/// source child. With no mesh link there is nothing to propagate to —
/// checked before the JSON round-trip.
pub fn forward_external_message(p_server: &Arc<Server>, p_msg: &Payload, p_binary: bool, p_source: Option<Source>) {
    let v_super = p_server.cfg.is_true("enable_super_server");
    let v_relay = p_server.cfg.is_true("enable_persistant_relay");
    if !v_super && !v_relay {
        return;
    }
    let v_msg =
        with_relay_metadata(p_msg, &p_server.origin_id(), None, "Failed to parse external message for path tracking");
    if v_relay {
        p_server.child.forward_message(&v_msg, p_binary);
    }
    if v_super {
        let v_exclude = match p_source {
            Some(Source::Child(id)) => Some(id),
            _ => None,
        };
        p_server.parent.forward_message(&v_msg, p_binary, v_exclude);
    }
}
