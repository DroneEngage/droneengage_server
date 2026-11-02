//! Message parsing and dispatch (js_chat_routing.js): messages from local
//! clients and messages relayed by the super/child servers, delivered as
//! group broadcasts, to one unit, or handled as system commands.

use serde_json::Value;
use std::sync::Arc;

use crate::chat::outbound_guard::MsgClass;
use crate::chat::relay::{forward_external_message, forward_message, Source};
use crate::chat::Ctx;
use crate::constants::*;
use crate::hub::{GroupRef, Hub, Payload};
use crate::server::Server;
use crate::util::{js_length, js_parse_bytes, js_stringify, set_field_opt};

fn split_header(p_data: &[u8]) -> (&[u8], Option<usize>) {
    match p_data.iter().position(|b| *b == 0) {
        Some(i) => (&p_data[..i], Some(i)),
        None => (p_data, None),
    }
}

/// What `v_jmsg['tg']` selects in the 'i' routing.
enum Target<'a> {
    /// no tg, or a tg whose length is not > 0
    None,
    /// `tg.length` throws (tg is null)
    Throws,
    Named(&'a Value),
}

fn target(p_jmsg: &Value) -> Target<'_> {
    match p_jmsg.as_object().and_then(|o| o.get(CONST_WS_TARGET_ID)) {
        None => Target::None,
        Some(Value::Null) => Target::Throws,
        Some(v) => match js_length(Some(v)) {
            Some(n) if n > 0 => Target::Named(v),
            _ => Target::None,
        },
    }
}

fn reserved(p_tg: &Value) -> Option<&str> {
    match p_tg.as_str() {
        Some(s @ (CONST_WS_SENDER_ALL_GCS | CONST_WS_SENDER_ALL | CONST_WS_SENDER_ALL_AGENTS)) => Some(s),
        _ => None,
    }
}

/// deliverExternalBroadcast: scoped to the originating account/group when
/// the message carries `_aid`/`_gid` and that group exists here, otherwise
/// the legacy broadcast to every account.
#[allow(clippy::too_many_arguments)]
fn deliver_external_broadcast(
    p_server: &Server,
    p_hub: &mut Hub,
    p_msg: &Payload,
    p_sender: Option<&Value>,
    p_jmsg: &Value,
    p_actor: Option<&str>,
    p_class: MsgClass,
) {
    let v_aid = p_jmsg.get("_aid");
    let v_gid = p_jmsg.get("_gid");
    if v_aid.is_some()
        && v_gid.is_some()
        && p_hub.send_to_account_group(&p_server.guard, p_msg, true, p_sender, v_aid, v_gid, p_actor, p_class)
    {
        return;
    }
    match p_actor {
        Some("g") => p_hub.send_to_all_gcs(&p_server.guard, p_msg, true, p_sender, p_class),
        Some("d") => p_hub.send_to_all_agent(&p_server.guard, p_msg, true, p_sender, p_class),
        _ => p_hub.send_to_all(&p_server.guard, p_msg, true, p_sender, p_class),
    }
}

/// fn_parseExternalMessage: a message received from the super server or a
/// child. The relay links deliver every message as binary (ws hands them a
/// Buffer), so p_binary is true for them.
pub fn parse_external_message(p_server: &Arc<Server>, p_data: &[u8], p_binary: bool, p_source: Option<Source>) {
    let (v_header, _) = if p_binary { split_header(p_data) } else { (p_data, None) };
    let Some(v_jmsg) = js_parse_bytes(v_header) else { return };
    if v_jmsg.is_null() {
        return;
    }
    let v_sender = v_jmsg.get(CONST_WS_SENDER_ID).cloned();
    // loop prevention: this server already relayed it
    if let Some(Value::Array(v_path)) = v_jmsg.get("_path") {
        let v_me = p_server.origin_id();
        if v_path.iter().any(|p| p.as_str() == Some(v_me.as_str())) {
            return;
        }
    }
    let v_class = p_server.guard.message_class(v_jmsg.get(CONST_WS_MESSAGE_ID));
    let v_msg = if p_binary {
        Payload::binary(p_data.to_vec())
    } else {
        Payload::text(String::from_utf8_lossy(p_data).into_owned())
    };

    let v_ty = v_jmsg.get(CONST_WS_MSG_ROUTING).and_then(Value::as_str);
    let v_forward = {
        let mut v_hub = p_server.hub();
        match v_ty {
            Some(CONST_WS_MSG_ROUTING_GROUP) => {
                deliver_external_broadcast(p_server, &mut v_hub, &v_msg, v_sender.as_ref(), &v_jmsg, None, v_class);
                true
            }
            Some(CONST_WS_MSG_ROUTING_INDIVIDUAL) => match target(&v_jmsg) {
                Target::Throws => {
                    println!("TypeError: Cannot read properties of null (reading 'length')");
                    p_server.log_error(
                        "Bad Parsing Message:CONST_WS_MSG_ROUTING_INDIVIDUAL",
                        Some("fn_parseExternalMessage"),
                        None,
                    );
                    false
                }
                Target::Named(v_tg) => match reserved(v_tg) {
                    Some(CONST_WS_SENDER_ALL_GCS) => {
                        deliver_external_broadcast(
                            p_server,
                            &mut v_hub,
                            &v_msg,
                            v_sender.as_ref(),
                            &v_jmsg,
                            Some("g"),
                            v_class,
                        );
                        true
                    }
                    Some(CONST_WS_SENDER_ALL) => {
                        deliver_external_broadcast(
                            p_server,
                            &mut v_hub,
                            &v_msg,
                            v_sender.as_ref(),
                            &v_jmsg,
                            None,
                            v_class,
                        );
                        true
                    }
                    Some(_) => {
                        deliver_external_broadcast(
                            p_server,
                            &mut v_hub,
                            &v_msg,
                            v_sender.as_ref(),
                            &v_jmsg,
                            Some("d"),
                            v_class,
                        );
                        true
                    }
                    None => {
                        // one-to-one: not propagated further
                        v_hub.send_individual_id(
                            &p_server.guard,
                            &v_msg,
                            true,
                            Some(v_tg),
                            v_sender.as_ref(),
                            v_jmsg.get("_gid"),
                            v_class,
                        );
                        false
                    }
                },
                Target::None => {
                    // no target: everyone in the originating account/group
                    deliver_external_broadcast(p_server, &mut v_hub, &v_msg, v_sender.as_ref(), &v_jmsg, None, v_class);
                    true
                }
            },
            _ => false,
        }
    };
    if v_forward {
        forward_external_message(p_server, &v_msg, p_binary, p_source);
    }
}

/// The routing decision for a local message, taken under the hub lock.
enum Route {
    None,
    /// relay to the mesh (after local delivery)
    Forward,
    /// a system command, handled after the hub is released
    System(Box<(Ctx, Value)>),
}

/// fn_parseMessage: a message from a local client socket.
pub fn parse_message(p_server: &Arc<Server>, p_conn: u64, p_data: &[u8], p_binary: bool) {
    let (v_route, v_msg, v_group) = {
        let mut v_hub = p_server.hub();
        // admin-blocked units stay connected but their traffic is dropped
        let v_blocked = match v_hub.client(p_conn) {
            None => return,
            Some(c) => c.login.as_ref().map(|l| v_hub.is_sender_blocked(l.sender_id.as_ref())).unwrap_or(false),
        };
        if v_blocked {
            return;
        }
        let v_client = v_hub.client_mut(p_conn).unwrap();
        let (v_header, v_null_index) =
            if p_binary { split_header(p_data) } else { (p_data, p_data.iter().position(|b| *b == 0)) };
        let Some(v_status) = v_client.status.as_mut() else {
            // unauthorized socket (no m_status yet)
            return;
        };
        if p_binary {
            v_status.btx += p_data.len() as u64;
        } else {
            v_status.ttx += p_data.len() as u64;
        }
        let Some(mut v_jmsg) = js_parse_bytes(v_header) else { return };
        // inject the permission AUTH granted: the only way to make sure a
        // GCS can't fake it
        let Some(v_login) = v_client.login.clone() else { return };
        match &mut v_jmsg {
            Value::Object(_) => set_field_opt(&mut v_jmsg, "p", v_login.prm.clone()),
            Value::Array(_) => {}
            _ => return,
        }
        let v_text = js_stringify(&v_jmsg);
        // where the 0 terminator lands now that the header has its new length
        let v_frame_null_index = v_null_index.map(|_| v_text.len());
        let v_msg = if p_binary {
            let mut v_out = v_text.into_bytes();
            if let Some(i) = v_null_index {
                v_out.extend_from_slice(&p_data[i..]);
            }
            Payload::binary(v_out)
        } else {
            Payload::text(v_text)
        };

        p_server.plugins.dispatch(p_conn, v_login.account_id.as_ref(), &v_msg, v_frame_null_index);

        let v_class = p_server.guard.message_class(v_jmsg.get(CONST_WS_MESSAGE_ID));
        let v_group = v_client.group.clone();
        let v_sender = v_login.sender_id.clone();
        let v_is_gcs = v_login.actor_is(CONST_ACTOR_TYPE_GCS);
        let v_is_drone = v_login.actor_is(CONST_ACTOR_TYPE_DRONE);
        let v_guard = &p_server.guard;
        let fn_to_group = |hub: &mut Hub, actor: Option<&str>| {
            if let Some(g) = &v_group {
                hub.group_broadcast(v_guard, g, &v_msg, p_binary, v_sender.as_ref(), actor, v_class);
            }
        };

        let v_route = match v_jmsg.get(CONST_WS_MSG_ROUTING).and_then(Value::as_str) {
            Some(CONST_WS_MSG_ROUTING_GROUP) => {
                fn_to_group(&mut v_hub, if v_is_gcs { None } else { Some("g") });
                Route::Forward
            }
            Some(CONST_WS_MSG_ROUTING_INDIVIDUAL) => match target(&v_jmsg) {
                Target::Throws => {
                    println!("TypeError: Cannot read properties of null (reading 'length')");
                    p_server.log_error(
                        "Bad Parsing Message:CONST_WS_MSG_ROUTING_INDIVIDUAL",
                        Some("fn_parseMessage"),
                        None,
                    );
                    Route::None
                }
                Target::Named(v_tg) => match reserved(v_tg) {
                    Some(CONST_WS_SENDER_ALL_GCS) => {
                        fn_to_group(&mut v_hub, Some("g"));
                        Route::Forward
                    }
                    Some(CONST_WS_SENDER_ALL) => {
                        fn_to_group(&mut v_hub, None);
                        Route::Forward
                    }
                    Some(_) => {
                        fn_to_group(&mut v_hub, Some("d"));
                        Route::Forward
                    }
                    None => {
                        // one-to-one; a unit not in the sender's group is
                        // tried on the mesh (dropped there if absent, never
                        // stored for later — TASK-P4A-09)
                        match &v_group {
                            Some(g) => {
                                if v_hub.group_send_individual(v_guard, g, &v_msg, p_binary, Some(v_tg), v_class) {
                                    Route::None
                                } else {
                                    Route::Forward
                                }
                            }
                            None => Route::None,
                        }
                    }
                },
                Target::None => {
                    if v_is_drone {
                        // default broadcast for agents is to GCS only
                        fn_to_group(&mut v_hub, Some("g"));
                        Route::Forward
                    } else if v_is_gcs {
                        // default broadcast for GCS is to all units
                        fn_to_group(&mut v_hub, None);
                        Route::Forward
                    } else {
                        Route::None
                    }
                }
            },
            Some(CONST_WS_MSG_ROUTING_SYSTEM) => match Ctx::from_hub(&v_hub, p_conn) {
                Some(c) => Route::System(Box::new((c, v_jmsg.clone()))),
                None => Route::None,
            },
            _ => Route::None,
        };
        let v_group_ids = v_group.as_ref().and_then(|g| group_ids(&v_hub, g));
        (v_route, v_msg, v_group_ids)
    };
    match v_route {
        Route::None => {}
        Route::Forward => forward_message(p_server, &v_msg, p_binary, v_group),
        Route::System(v_cmd) => {
            let (c, v_jmsg) = *v_cmd;
            crate::chat::system_commands::on_system_message(p_server, c, v_jmsg)
        }
    }
}

/// (group m_ID, account m_accountID) of a group, for _gid/_aid.
fn group_ids(p_hub: &Hub, p_group: &GroupRef) -> Option<(Value, Value)> {
    let v_account = p_hub.accounts.get(&p_group.account_key)?;
    let v_group = v_account.groups.get(&p_group.group_key).filter(|g| g.uid == p_group.uid)?;
    Some((v_group.id.clone(), v_account.id.clone()))
}
