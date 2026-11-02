//! Message parsing and dispatch (js_chat_routing.js): messages from local
//! clients and messages relayed by the super/child servers, delivered as
//! group broadcasts, to one unit, or handled as system commands.

use serde_json::Value;
use std::borrow::Cow;
use std::sync::Arc;

use crate::chat::outbound_guard::MsgClass;
use crate::chat::relay::{forward_external_message, forward_message, Source};
use crate::chat::Ctx;
use crate::constants::*;
use crate::hub::{GroupRef, Hub, Payload};
use crate::server::Server;
use crate::util::{js_length, js_parse, js_stringify, set_field_opt};

fn split_header(p_data: &[u8]) -> (&[u8], Option<usize>) {
    match p_data.iter().position(|b| *b == 0) {
        Some(i) => (&p_data[..i], Some(i)),
        None => (p_data, None),
    }
}

/// serde_json's nesting limit: a frame this deep is not valid to us either.
const MAX_JSON_DEPTH: usize = 128;
/// A number this short without an exponent cannot overflow a double.
const SHORT_NUMBER_LEN: usize = 18;

/// A cursor over JSON text that accepts exactly what serde_json accepts,
/// without building anything.
struct JsonCursor<'a> {
    text: &'a str,
    i: usize,
}

impl JsonCursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, p_byte: u8) -> Option<()> {
        (self.peek()? == p_byte).then(|| self.i += 1)
    }

    fn hex4(&mut self) -> Option<u32> {
        let v_digits = self.text.as_bytes().get(self.i..self.i + 4)?;
        let mut v_out = 0;
        for d in v_digits {
            v_out = v_out * 16 + (*d as char).to_digit(16)?;
        }
        self.i += 4;
        Some(v_out)
    }

    /// At an opening quote: past the closing one. Returns whether the string
    /// has an escape, so its text is not its value.
    fn string(&mut self) -> Option<bool> {
        self.eat(b'"')?;
        let mut v_escaped = false;
        loop {
            let v_byte = self.peek()?;
            self.i += 1;
            match v_byte {
                b'"' => return Some(v_escaped),
                b'\\' => {
                    v_escaped = true;
                    let v_kind = self.peek()?;
                    self.i += 1;
                    match v_kind {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                        b'u' => match self.hex4()? {
                            0xDC00..=0xDFFF => return None,
                            0xD800..=0xDBFF => {
                                self.eat(b'\\')?;
                                self.eat(b'u')?;
                                if !(0xDC00..=0xDFFF).contains(&self.hex4()?) {
                                    return None;
                                }
                            }
                            _ => {}
                        },
                        _ => return None,
                    }
                }
                0..=0x1f => return None,
                _ => {}
            }
        }
    }

    fn digits(&mut self) -> Option<()> {
        self.peek().filter(u8::is_ascii_digit)?;
        while self.peek().is_some_and(|b| b.is_ascii_digit()) {
            self.i += 1;
        }
        Some(())
    }

    fn number(&mut self) -> Option<()> {
        let v_start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        if self.peek() == Some(b'0') {
            self.i += 1;
        } else {
            self.digits()?;
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            self.digits()?;
        }
        let mut v_exponent = false;
        if matches!(self.peek(), Some(b'e' | b'E')) {
            v_exponent = true;
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            self.digits()?;
        }
        // only a long number or an exponent can be out of range
        let v_token = &self.text[v_start..self.i];
        if v_exponent || v_token.len() > SHORT_NUMBER_LEN {
            js_parse(v_token)?;
        }
        Some(())
    }

    fn literal(&mut self, p_word: &str) -> Option<()> {
        self.text[self.i..].starts_with(p_word).then(|| self.i += p_word.len())
    }

    fn value(&mut self, p_depth: usize) -> Option<()> {
        match self.peek()? {
            b'{' => self.object(p_depth, |_, _, _, _| {}).map(|_| ()),
            b'[' => {
                let v_depth = p_depth.checked_sub(1).filter(|d| *d > 0)?;
                self.i += 1;
                self.skip_ws();
                if self.peek()? == b']' {
                    self.i += 1;
                    return Some(());
                }
                loop {
                    self.skip_ws();
                    self.value(v_depth)?;
                    self.skip_ws();
                    match self.peek()? {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Some(());
                        }
                        _ => return None,
                    }
                }
            }
            b'"' => self.string().map(|_| ()),
            b't' => self.literal("true"),
            b'f' => self.literal("false"),
            b'n' => self.literal("null"),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    /// At '{': past the matching '}'. Calls p_member with each member's
    /// (key text with quotes, escaped, member start, value start) and returns
    /// the index of the closing '}' and the member count.
    fn object(&mut self, p_depth: usize, mut p_member: impl FnMut(&str, bool, usize, usize)) -> Option<(usize, usize)> {
        let v_depth = p_depth.checked_sub(1).filter(|d| *d > 0)?;
        self.eat(b'{')?;
        self.skip_ws();
        if self.peek()? == b'}' {
            self.i += 1;
            return Some((self.i - 1, 0));
        }
        let mut v_members = 0;
        loop {
            self.skip_ws();
            let v_key_start = self.i;
            let v_escaped = self.string()?;
            let v_key_end = self.i;
            self.skip_ws();
            self.eat(b':')?;
            self.skip_ws();
            let v_value_start = self.i;
            self.value(v_depth)?;
            p_member(&self.text[v_key_start..v_key_end], v_escaped, v_key_start, v_value_start);
            v_members += 1;
            self.skip_ws();
            match self.peek()? {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    return Some((self.i - 1, v_members));
                }
                _ => return None,
            }
        }
    }
}

/// What a frame's JSON header is, and where the fields routing reads are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderKind {
    Object,
    /// not routed and gets no permission; plugins still see it
    Array,
    /// null, a string, a number or a boolean: dropped
    Other,
}

/// A validated JSON header, scanned without building a Value. For a key that
/// appears twice the last one counts, as in JSON.parse.
pub struct ScannedHeader<'a> {
    text: &'a str,
    pub kind: HeaderKind,
    /// index of the object's closing '}'
    close: usize,
    members: usize,
    p_count: usize,
    ty: Option<usize>,
    tg: Option<usize>,
    mt: Option<usize>,
    sd: Option<usize>,
    aid: Option<usize>,
    gid: Option<usize>,
    path: Option<usize>,
}

/// None when p_text is not valid JSON (serde_json's rules, its depth limit too).
pub fn scan_header(p_text: &str) -> Option<ScannedHeader<'_>> {
    let mut v_out = ScannedHeader {
        text: p_text,
        kind: HeaderKind::Other,
        close: 0,
        members: 0,
        p_count: 0,
        ty: None,
        tg: None,
        mt: None,
        sd: None,
        aid: None,
        gid: None,
        path: None,
    };
    let mut c = JsonCursor { text: p_text, i: 0 };
    c.skip_ws();
    match c.peek()? {
        b'{' => {
            let (v_close, v_members) = c.object(MAX_JSON_DEPTH, |p_key, p_escaped, _, p_value| {
                let v_key = if p_escaped { js_parse(p_key).and_then(|k| k.as_str().map(str::to_string)) } else { None };
                let v_name = v_key.as_deref().unwrap_or(&p_key[1..p_key.len() - 1]);
                match v_name {
                    "p" => v_out.p_count += 1,
                    CONST_WS_MSG_ROUTING => v_out.ty = Some(p_value),
                    CONST_WS_TARGET_ID => v_out.tg = Some(p_value),
                    CONST_WS_MESSAGE_ID => v_out.mt = Some(p_value),
                    CONST_WS_SENDER_ID => v_out.sd = Some(p_value),
                    "_aid" => v_out.aid = Some(p_value),
                    "_gid" => v_out.gid = Some(p_value),
                    "_path" => v_out.path = Some(p_value),
                    _ => {}
                }
            })?;
            v_out.kind = HeaderKind::Object;
            v_out.close = v_close;
            v_out.members = v_members;
        }
        b'[' => {
            c.value(MAX_JSON_DEPTH)?;
            v_out.kind = HeaderKind::Array;
        }
        _ => c.value(MAX_JSON_DEPTH)?,
    }
    c.skip_ws();
    (c.peek().is_none()).then_some(v_out)
}

impl ScannedHeader<'_> {
    /// The value starting at p_start, parsed on its own.
    fn value_at(&self, p_start: Option<usize>) -> Option<Value> {
        let mut c = JsonCursor { text: self.text, i: p_start? };
        c.value(MAX_JSON_DEPTH)?;
        js_parse(&self.text[p_start?..c.i])
    }

    /// `ty` when it is a string.
    pub fn routing(&self) -> Option<Cow<'_, str>> {
        let v_start = self.ty?;
        let mut c = JsonCursor { text: self.text, i: v_start };
        if self.text.as_bytes()[v_start] != b'"' {
            return None;
        }
        if c.string()? {
            return js_parse(&self.text[v_start..c.i]).and_then(|v| v.as_str().map(|s| Cow::Owned(s.to_string())));
        }
        Some(Cow::Borrowed(&self.text[v_start + 1..c.i - 1]))
    }

    pub fn target(&self) -> Option<Value> {
        self.value_at(self.tg)
    }

    pub fn message_id(&self) -> Option<Value> {
        self.value_at(self.mt)
    }

    pub fn sender(&self) -> Option<Value> {
        self.value_at(self.sd)
    }

    pub fn account_id(&self) -> Option<Value> {
        self.value_at(self.aid)
    }

    pub fn group_id(&self) -> Option<Value> {
        self.value_at(self.gid)
    }

    pub fn path(&self) -> Option<Value> {
        self.value_at(self.path)
    }

    /// The header with p_prm as its only top-level `p`, under any spelling of
    /// the key: this is the only guard against a client faking its permission.
    /// No permission means no `p` at all. Anything but an object is unchanged.
    pub fn with_permission(&self, p_prm: Option<&Value>) -> String {
        if self.kind != HeaderKind::Object {
            return self.text.to_string();
        }
        let v_prm = p_prm.map(js_stringify);
        let mut v_out = String::with_capacity(self.text.len() + 16);
        if self.p_count == 0 {
            let Some(v_prm) = v_prm else { return self.text.to_string() };
            v_out.push_str(&self.text[..self.close]);
            if self.members > 0 {
                v_out.push(',');
            }
            v_out.push_str("\"p\":");
            v_out.push_str(&v_prm);
            v_out.push_str(&self.text[self.close..]);
            return v_out;
        }
        // a client-sent p: keep every other member and drop all of its copies
        let mut v_starts = Vec::with_capacity(self.members);
        let mut c = JsonCursor { text: self.text, i: self.text.find('{').unwrap_or(0) };
        let _ = c.object(MAX_JSON_DEPTH, |p_key, p_escaped, p_start, _| {
            let v_is_p = if p_escaped { js_parse(p_key).is_some_and(|k| k == "p") } else { p_key == "\"p\"" };
            v_starts.push((p_start, v_is_p));
        });
        v_out.push('{');
        let mut v_first = true;
        for (i, (v_start, v_is_p)) in v_starts.iter().enumerate() {
            if *v_is_p {
                continue;
            }
            let v_next = v_starts.get(i + 1).map(|n| n.0).unwrap_or(self.close);
            // a member runs up to the next one, less its comma and whitespace
            let v_member = self.text[*v_start..v_next].trim_end_matches([' ', '\n', '\r', '\t', ',']);
            if !v_first {
                v_out.push(',');
            }
            v_out.push_str(v_member);
            v_first = false;
        }
        if let Some(v_prm) = v_prm {
            if !v_first {
                v_out.push(',');
            }
            v_out.push_str("\"p\":");
            v_out.push_str(&v_prm);
        }
        v_out.push('}');
        v_out
    }
}

/// A frame's header as text: invalid UTF-8 becomes U+FFFD, as Buffer.toString()
/// does, so a receiver never gets invalid UTF-8 in JSON.
fn header_text(p_header: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(p_header)
}

/// What `v_jmsg['tg']` selects in the 'i' routing.
enum Target<'a> {
    /// no tg, or a tg whose length is not > 0
    None,
    /// `tg.length` throws (tg is null)
    Throws,
    Named(&'a Value),
}

fn target(p_tg: Option<&Value>) -> Target<'_> {
    match p_tg {
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
    p_aid: Option<&Value>,
    p_gid: Option<&Value>,
    p_actor: Option<&str>,
    p_class: MsgClass,
) {
    if p_aid.is_some()
        && p_gid.is_some()
        && p_hub.send_to_account_group(&p_server.guard, p_msg, true, p_sender, p_aid, p_gid, p_actor, p_class)
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
    let v_text = header_text(v_header);
    let Some(v_scan) = scan_header(&v_text) else { return };
    // anything but an object has no ty: nothing to deliver or relay
    if v_scan.kind != HeaderKind::Object {
        return;
    }
    let v_sender = v_scan.sender();
    // loop prevention: this server already relayed it
    if let Some(Value::Array(v_path)) = v_scan.path() {
        let v_me = p_server.origin_id();
        if v_path.iter().any(|p| p.as_str() == Some(v_me.as_str())) {
            return;
        }
    }
    let v_class = p_server.guard.message_class(v_scan.message_id().as_ref());
    let (v_aid, v_gid) = (v_scan.account_id(), v_scan.group_id());
    let v_msg = if p_binary {
        Payload::binary(p_data.to_vec())
    } else {
        Payload::text(String::from_utf8_lossy(p_data).into_owned())
    };

    let v_ty = v_scan.routing();
    let v_tg = v_scan.target();
    let v_forward = {
        let mut v_hub = p_server.hub();
        match v_ty.as_deref() {
            Some(CONST_WS_MSG_ROUTING_GROUP) => {
                deliver_external_broadcast(p_server, &mut v_hub, &v_msg, v_sender.as_ref(), v_aid.as_ref(), v_gid.as_ref(), None, v_class);
                true
            }
            Some(CONST_WS_MSG_ROUTING_INDIVIDUAL) => match target(v_tg.as_ref()) {
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
                            v_aid.as_ref(),
                            v_gid.as_ref(),
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
                            v_aid.as_ref(),
                            v_gid.as_ref(),
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
                            v_aid.as_ref(),
                            v_gid.as_ref(),
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
                            v_gid.as_ref(),
                            v_class,
                        );
                        false
                    }
                },
                Target::None => {
                    // no target: everyone in the originating account/group
                    deliver_external_broadcast(p_server, &mut v_hub, &v_msg, v_sender.as_ref(), v_aid.as_ref(), v_gid.as_ref(), None, v_class);
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

/// fn_parseMessage: a message from a local client socket. The hub is held
/// twice and only for lookups and delivery: reading what the sender may do,
/// then routing. Scanning the header and building the frame need neither, so
/// busy connections do that work side by side. A socket's messages arrive one
/// at a time from its read loop, so their order is kept.
pub fn parse_message(p_server: &Arc<Server>, p_conn: u64, p_data: &[u8], p_binary: bool) {
    let (v_header, v_null_index) =
        if p_binary { split_header(p_data) } else { (p_data, p_data.iter().position(|b| *b == 0)) };
    let v_plugins = !p_server.plugins.is_empty();
    let (v_prm, v_sender, v_is_gcs, v_is_drone, v_account) = {
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
        let Some(v_status) = v_client.status.as_mut() else {
            // unauthorized socket (no m_status yet)
            return;
        };
        if p_binary {
            v_status.btx += p_data.len() as u64;
        } else {
            v_status.ttx += p_data.len() as u64;
        }
        let Some(v_login) = v_client.login.as_ref() else { return };
        (
            v_login.prm.clone(),
            v_login.sender_id.clone(),
            v_login.actor_is(CONST_ACTOR_TYPE_GCS),
            v_login.actor_is(CONST_ACTOR_TYPE_DRONE),
            if v_plugins { v_login.account_id.clone() } else { None },
        )
    };

    let v_text = header_text(v_header);
    let Some(v_scan) = scan_header(&v_text) else { return };
    if v_scan.kind == HeaderKind::Other {
        return;
    }
    // inject the permission AUTH granted: the only way to make sure a
    // GCS can't fake it
    let v_frame = v_scan.with_permission(v_prm.as_ref());
    // where the 0 terminator lands now that the header has its new length
    let v_frame_null_index = v_null_index.map(|_| v_frame.len());
    let v_msg = if p_binary {
        let mut v_out = v_frame.into_bytes();
        if let Some(i) = v_null_index {
            v_out.extend_from_slice(&p_data[i..]);
        }
        Payload::binary(v_out)
    } else {
        Payload::text(v_frame)
    };
    if v_plugins {
        p_server.plugins.dispatch(p_conn, v_account.as_ref(), &v_msg, v_frame_null_index);
    }
    let v_class = p_server.guard.message_class(v_scan.message_id().as_ref());
    let v_tg = v_scan.target();
    let v_ty = v_scan.routing();
    // the rare path: system commands read the whole message
    let mut v_system = None;
    if v_ty.as_deref() == Some(CONST_WS_MSG_ROUTING_SYSTEM) {
        let Some(mut v_jmsg) = js_parse(&v_text) else { return };
        set_field_opt(&mut v_jmsg, "p", v_prm);
        v_system = Some(v_jmsg);
    }
    let v_mesh = p_server.cfg.is_true("enable_super_server") || p_server.cfg.is_true("enable_persistant_relay");

    let (v_route, v_group) = {
        let mut v_hub = p_server.hub();
        // the socket is routed as it is now: gone or out of its group, nothing is sent
        let Some(v_client) = v_hub.client(p_conn) else { return };
        let v_group = v_client.group.clone();
        let v_guard = &p_server.guard;
        let fn_to_group = |hub: &mut Hub, actor: Option<&str>| {
            if let Some(g) = &v_group {
                hub.group_broadcast(v_guard, g, &v_msg, p_binary, v_sender.as_ref(), actor, v_class);
            }
        };

        let v_route = match v_ty.as_deref() {
            Some(CONST_WS_MSG_ROUTING_GROUP) => {
                fn_to_group(&mut v_hub, if v_is_gcs { None } else { Some("g") });
                Route::Forward
            }
            Some(CONST_WS_MSG_ROUTING_INDIVIDUAL) => match target(v_tg.as_ref()) {
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
            Some(CONST_WS_MSG_ROUTING_SYSTEM) => match (Ctx::from_hub(&v_hub, p_conn), v_system) {
                (Some(c), Some(v_jmsg)) => Route::System(Box::new((c, v_jmsg))),
                _ => Route::None,
            },
            _ => Route::None,
        };
        let v_needs_ids = v_mesh && matches!(v_route, Route::Forward);
        let v_group_ids = if v_needs_ids { v_group.as_ref().and_then(|g| group_ids(&v_hub, g)) } else { None };
        (v_route, v_group_ids)
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
    let v_account = p_hub.accounts.get(&*p_group.account_key)?;
    let v_group = v_account.groups.get(&*p_group.group_key).filter(|g| g.uid == p_group.uid)?;
    Some((v_group.id.clone(), v_account.id.clone()))
}
