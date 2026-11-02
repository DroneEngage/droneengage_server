//! CHAT ROOMS for each ACCOUNT-ID (js_andruav_chat_account_rooms.js): each
//! account has groups, each group has units (sockets) by name. Implemented
//! on the Hub, which owns the sockets.

use serde_json::{json, Map, Value};

use crate::chat::outbound_guard::{GuardConfig, MsgClass, SendResult};
use crate::hub::{Account, Group, GroupRef, Hub, Login, Payload};
use crate::util::{js_key, js_nullish, js_strict_eq, now_ms};

impl Hub {
    /// fn_getUnitKeys: the account ids (as strings).
    pub fn unit_keys(&self) -> Vec<String> {
        self.accounts.keys().cloned().collect()
    }

    /// fn_getAccountDetails: accountId -> [{unitName, actorType, requestId, blocked}]
    pub fn account_details(&self) -> Value {
        let mut v_details = Map::new();
        for (v_account_key, v_account) in &self.accounts {
            let mut v_units = Vec::new();
            for v_group in v_account.groups.values() {
                for (v_unit_name, v_conn) in &v_group.units {
                    let v_login = self.clients.get(v_conn).and_then(|c| c.login.as_ref());
                    let mut v_row = Map::new();
                    v_row.insert("unitName".into(), json!(v_unit_name));
                    match v_login {
                        Some(l) => {
                            if let Some(a) = &l.actor_type {
                                v_row.insert("actorType".into(), a.clone());
                            }
                            if let Some(r) = &l.request_id {
                                v_row.insert("requestId".into(), r.clone());
                            }
                        }
                        None => {
                            v_row.insert("actorType".into(), json!("a"));
                            v_row.insert("requestId".into(), Value::Null);
                        }
                    }
                    v_row.insert("blocked".into(), json!(self.blocked.contains(v_unit_name)));
                    v_units.push(Value::Object(v_row));
                }
            }
            v_details.insert(v_account_key.clone(), Value::Array(v_units));
        }
        Value::Object(v_details)
    }

    fn all_groups(&self) -> Vec<GroupRef> {
        let mut v_out = Vec::new();
        for (a, v_account) in &self.accounts {
            for (g, v_group) in &v_account.groups {
                v_out.push(GroupRef { account_key: a.as_str().into(), group_key: g.as_str().into(), uid: v_group.uid });
            }
        }
        v_out
    }

    /// Sends to all Ground Control Stations in ALL accounts.
    pub fn send_to_all_gcs(
        &mut self,
        p_guard: &GuardConfig,
        p_msg: &Payload,
        p_binary: bool,
        p_sender: Option<&Value>,
        p_class: MsgClass,
    ) {
        for g in self.all_groups() {
            self.group_broadcast(p_guard, &g, p_msg, p_binary, p_sender, Some("g"), p_class);
        }
    }

    /// Sends to all agents (drones) in ALL accounts.
    pub fn send_to_all_agent(
        &mut self,
        p_guard: &GuardConfig,
        p_msg: &Payload,
        p_binary: bool,
        p_sender: Option<&Value>,
        p_class: MsgClass,
    ) {
        for g in self.all_groups() {
            self.group_broadcast(p_guard, &g, p_msg, p_binary, p_sender, Some("d"), p_class);
        }
    }

    pub fn send_to_all(
        &mut self,
        p_guard: &GuardConfig,
        p_msg: &Payload,
        p_binary: bool,
        p_sender: Option<&Value>,
        p_class: MsgClass,
    ) {
        for g in self.all_groups() {
            self.group_broadcast(p_guard, &g, p_msg, p_binary, p_sender, None, p_class);
        }
    }

    /// Sends to all GCS across every group of one account.
    /// @returns true if the account exists locally.
    pub fn send_to_all_gcs_in_account(
        &mut self,
        p_guard: &GuardConfig,
        p_msg: &Payload,
        p_binary: bool,
        p_sender: Option<&Value>,
        p_account: Option<&Value>,
        p_class: MsgClass,
    ) -> bool {
        let v_key = js_key(p_account);
        let Some(v_account) = self.accounts.get(&v_key) else { return false };
        let v_groups: Vec<GroupRef> = v_account
            .groups
            .iter()
            .map(|(g, gr)| GroupRef { account_key: v_key.as_str().into(), group_key: g.as_str().into(), uid: gr.uid })
            .collect();
        for g in v_groups {
            self.group_broadcast(p_guard, &g, p_msg, p_binary, p_sender, Some("g"), p_class);
        }
        true
    }

    /// A broadcast scoped to one account/group (relayed messages).
    /// actor: 'g', 'd' or None (all). @returns true if the group exists locally.
    #[allow(clippy::too_many_arguments)]
    pub fn send_to_account_group(
        &mut self,
        p_guard: &GuardConfig,
        p_msg: &Payload,
        p_binary: bool,
        p_sender: Option<&Value>,
        p_account: Option<&Value>,
        p_group: Option<&Value>,
        p_actor: Option<&str>,
        p_class: MsgClass,
    ) -> bool {
        let v_account_key = js_key(p_account);
        let v_group_key = js_key(p_group);
        let Some(v_group) = self.accounts.get(&v_account_key).and_then(|a| a.groups.get(&v_group_key)) else {
            return false;
        };
        let v_ref = GroupRef { account_key: v_account_key.as_str().into(), group_key: v_group_key.as_str().into(), uid: v_group.uid };
        self.group_broadcast(p_guard, &v_ref, p_msg, p_binary, p_sender, p_actor, p_class);
        true
    }

    /// fn_sendTIndividualId: one-to-one delivery of a relayed message.
    /// @returns false where the JavaScript code called the not-found callback.
    #[allow(clippy::too_many_arguments)]
    pub fn send_individual_id(
        &mut self,
        p_guard: &GuardConfig,
        p_msg: &Payload,
        p_binary: bool,
        p_target: Option<&Value>,
        p_sender: Option<&Value>,
        p_group_id: Option<&Value>,
        p_class: MsgClass,
    ) -> bool {
        let Some(v_target) = self.active_sender(p_target) else { return false };
        if !self.clients[&v_target].handle.is_open() {
            return false;
        }
        let v_target_group = self.clients[&v_target].group.clone();
        let v_sender = self.active_sender(p_sender);
        let v_sender_group = v_sender.and_then(|s| self.clients[&s].group.clone());
        let v_deliver = if Hub::same_group(v_sender_group.as_ref(), v_target_group.as_ref()) {
            true
        } else if v_sender.is_none() && p_group_id.is_some() {
            // message from a super server: validate with the injected group id
            self.group_id_is(v_target_group.as_ref(), p_group_id)
        } else {
            false
        };
        if !v_deliver {
            return false;
        }
        let v_client = self.clients.get_mut(&v_target).unwrap();
        p_guard.safe_send(v_client, p_msg, p_binary, p_class);
        true
    }

    /// Adds a logged-in socket to its account/group (both created when
    /// missing). @returns false when the group already has that unit name.
    pub fn add_member_to_account_group(&mut self, p_conn: u64) -> bool {
        let Some(v_login) = self.clients.get(&p_conn).and_then(|c| c.login.clone()) else { return false };
        let v_account_key = v_login.account_key();
        if self.accounts.contains_key(&v_account_key) {
            println!("account {} already exists", v_account_key);
        } else {
            println!("account {} created", v_account_key);
            self.accounts.insert(
                v_account_key.clone(),
                Account { id: v_login.account_id.clone().unwrap_or(Value::Null), groups: Default::default() },
            );
        }
        let v_group_key = js_key(v_login.group_id.as_ref());
        let v_has_group = self.accounts[&v_account_key].groups.contains_key(&v_group_key);
        if v_has_group {
            println!("group {} already exists", v_group_key);
        } else {
            println!("group {} created", v_group_key);
            self.m_next_group_uid += 1;
            let v_uid = self.m_next_group_uid;
            self.accounts.get_mut(&v_account_key).unwrap().groups.insert(
                v_group_key.clone(),
                Group {
                    id: v_login.group_id.clone().unwrap_or(Value::Null),
                    uid: v_uid,
                    units: Default::default(),
                    creation_date: now_ms(),
                },
            );
        }
        let v_unit_key = v_login.sender_key();
        let v_group = self.accounts.get_mut(&v_account_key).unwrap().groups.get_mut(&v_group_key).unwrap();
        if v_group.units.contains_key(&v_unit_key) {
            println!("addMember [{}] already EXISTS. Don't Override", v_unit_key);
            return false;
        }
        println!("addMember [{}] Added", v_unit_key);
        v_group.units.insert(v_unit_key, p_conn);
        let v_ref = GroupRef { account_key: v_account_key.as_str().into(), group_key: v_group_key.as_str().into(), uid: v_group.uid };
        let v_client = self.clients.get_mut(&p_conn).unwrap();
        v_client.name = v_login.sender_id.clone();
        v_client.group = Some(v_ref);
        true
    }

    /// fn_del_member_fromGroup: removes a socket from its own group (logout).
    pub fn del_member_from_group(&mut self, p_conn: u64) {
        let Some(v_client) = self.clients.get_mut(&p_conn) else { return };
        let Some(v_group) = v_client.group.clone() else {
            println!("del_member_fromGroup: Nothing to Delete");
            return;
        };
        if js_nullish(v_client.name.as_ref()) {
            v_client.group = None;
            println!("No unit name to remove");
            return;
        }
        let v_name = v_client.name_key();
        self.delete_member_by_name(&v_group, &v_name, false);
    }

    /// fn_del_member_fromAccountByName: removes the sockets with the login's
    /// senderID from every group of its account (terminating them when
    /// asked), and the account when no group is left.
    pub fn del_member_from_account_by_name(&mut self, p_login: Option<&Login>, p_terminate: bool) {
        let Some(v_login) = p_login else { return };
        if js_nullish(v_login.account_id.as_ref()) || js_nullish(v_login.sender_id.as_ref()) {
            return;
        }
        let v_account_key = v_login.account_key();
        let Some(v_account) = self.accounts.get(&v_account_key) else {
            println!("info: no account associated with socket .... This could be a brand new socket.");
            return;
        };
        let v_groups: Vec<GroupRef> = v_account
            .groups
            .iter()
            .map(|(g, gr)| GroupRef { account_key: v_account_key.as_str().into(), group_key: g.as_str().into(), uid: gr.uid })
            .collect();
        let v_name = v_login.sender_key();
        for g in v_groups {
            self.delete_member_by_name(&g, &v_name, p_terminate);
        }
        if self.accounts.get(&v_account_key).map(|a| a.groups.is_empty()).unwrap_or(false) {
            self.accounts.shift_remove(&v_account_key);
        }
    }

    /// Group.fn_deleteMemberByName: deletes the socket with that name (even
    /// if it is not the caller's socket); an emptied group is removed from
    /// its account.
    pub fn delete_member_by_name(&mut self, p_group: &GroupRef, p_name: &str, p_terminate: bool) {
        let Some(v_account) = self.accounts.get_mut(&*p_group.account_key) else { return };
        let Some(v_group) = v_account.groups.get_mut(&*p_group.group_key).filter(|g| g.uid == p_group.uid) else {
            return;
        };
        let Some(v_old) = v_group.units.shift_remove(p_name) else { return };
        println!("deleteMemberByName: deleteMember {}", p_name);
        if v_group.units.is_empty() {
            v_account.groups.shift_remove(&*p_group.group_key);
        }
        if let Some(v_client) = self.clients.get_mut(&v_old) {
            v_client.group = None;
            if p_terminate {
                println!("deleteMemberByName: terminateSocket {}", p_name);
                v_client.terminated = true;
                v_client.handle.terminate();
            }
        }
    }

    /// Group.fn_broadcast: every unit of the group except the sender
    /// (by login senderID), optionally only one actor type.
    #[allow(clippy::too_many_arguments)]
    pub fn group_broadcast(
        &mut self,
        p_guard: &GuardConfig,
        p_group: &GroupRef,
        p_msg: &Payload,
        p_binary: bool,
        p_sender: Option<&Value>,
        p_actor: Option<&str>,
        p_class: MsgClass,
    ) {
        let Some(v_group) = self.accounts.get(&*p_group.account_key).and_then(|a| a.groups.get(&*p_group.group_key))
        else {
            return;
        };
        // a socket found dead leaves the group after the loop: the only unit
        // this could remove is one the loop has already passed (for...in)
        let mut v_dead = Vec::new();
        for v_conn in v_group.units.values() {
            let Some(v_client) = self.clients.get_mut(v_conn) else { continue };
            let Some(v_login) = v_client.login.as_ref() else { continue };
            if js_strict_eq(v_login.sender_id.as_ref(), p_sender) {
                continue;
            }
            if let Some(a) = p_actor {
                if !v_login.actor_is(a) {
                    continue;
                }
            }
            if p_guard.safe_send(v_client, p_msg, p_binary, p_class) == SendResult::Terminated {
                v_dead.push(*v_conn);
            }
        }
        for v_conn in v_dead {
            self.handle_orphan(v_conn);
        }
    }

    /// Group.fn_sendToIndividual: the unit by name in this group, no sender
    /// or ready-state checks. @returns false when the name is not in the group.
    pub fn group_send_individual(
        &mut self,
        p_guard: &GuardConfig,
        p_group: &GroupRef,
        p_msg: &Payload,
        p_binary: bool,
        p_target: Option<&Value>,
        p_class: MsgClass,
    ) -> bool {
        let v_target = js_key(p_target);
        let Some(v_conn) = self.group(p_group).and_then(|g| g.units.get(&v_target).copied()) else { return false };
        let Some(v_client) = self.clients.get_mut(&v_conn) else { return true };
        if p_guard.safe_send(v_client, p_msg, p_binary, p_class) == SendResult::Terminated {
            self.handle_orphan(v_conn);
        }
        true
    }

    /// Group.fn_handleOrphanSocket: a socket found dead (terminated for
    /// backlog) leaves its group, loses its name and is terminated.
    pub fn handle_orphan(&mut self, p_conn: u64) {
        let Some(v_client) = self.clients.get(&p_conn) else { return };
        let Some(v_group) = v_client.group.clone() else {
            // socket.m__group.fn_deleteMemberByName throws when it has no group
            return;
        };
        let v_name = v_client.name_key();
        self.delete_member_by_name(&v_group, &v_name, false);
        println!("unit undefined found dead");
        if let Some(c) = self.clients.get_mut(&p_conn) {
            c.name = None;
            c.handle.terminate();
        }
    }
}
