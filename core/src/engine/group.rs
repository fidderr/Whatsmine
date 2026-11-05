//! Group chats. Every member talks to every member directly: a group
//! message is one ordinary message sent to each of the others, each
//! acknowledged on its own, so nothing sits on any server and a member who
//! was offline simply gets what they missed when they come back.

use iroh::PublicKey;

use tokio::time::Instant;

use super::{Engine, clean_name, emit};
use crate::groups;
use crate::identity::{key_to_string, parse_key};
use crate::model::{
    Attachment, ContactState, CoreEvent, Message, MessageStatus, TransferState, now_ms,
};
use crate::protocol::{Frame, GroupState, MAX_GROUP_MEMBERS, MAX_TEXT, ReceiptKind, id_from_bytes};

/// How far back members compare what they have.
const RELAY_LOOK_BACK: u32 = 100;

/// Where it is kept that we asked to join a group.
pub(crate) fn join_asked_name(group: &str) -> String {
    format!("join-asked:{group}")
}

fn link_state_name(group: &str) -> String {
    format!("group-link:{group}")
}

pub(crate) fn group_link_state(
    db: &crate::db::Database,
    group: &str,
) -> Option<crate::model::GroupLinkState> {
    db.secret(&link_state_name(group))
        .and_then(|json| serde_json::from_slice(&json).ok())
}

pub(crate) fn save_group_link_state(
    db: &crate::db::Database,
    group: &str,
    state: &crate::model::GroupLinkState,
) {
    if let Ok(json) = serde_json::to_vec(state) {
        let _ = db.set_secret(&link_state_name(group), &json);
    }
}

impl Engine {
    fn me_id(&self) -> String {
        key_to_string(&self.me)
    }

    /// Display names for the members of `state`, for people who may not
    /// know them.
    fn group_names(&self, state: &GroupState) -> Vec<(String, String)> {
        state
            .members
            .iter()
            .map(|m| {
                let name = if *m == self.me_id() {
                    self.settings().display_name
                } else {
                    self.db
                        .contact(m)
                        .map(|c| {
                            if c.their_name.is_empty() {
                                c.nickname.unwrap_or_default()
                            } else {
                                c.their_name
                            }
                        })
                        .unwrap_or_default()
                };
                (m.clone(), name)
            })
            .collect()
    }

    pub(super) fn send_group_state(&self, peer: PublicKey, gid: &str) {
        let Some((state, sig, _)) = self.db.group(gid) else {
            return;
        };
        let names = self.group_names(&state);
        let Some(secret) = self.group_secret(&state.id) else {
            return;
        };
        self.send(
            peer,
            Frame::GroupUpdate {
                state,
                sig,
                names,
                secret,
            },
        );
    }

    /// What the members of this group find each other with.
    pub(super) fn group_secret(&self, gid: &str) -> Option<[u8; 32]> {
        self.db.secret(&groups::secret_name(gid))?.try_into().ok()
    }

    /// Sends the current state of a group to its members that are connected,
    /// and to `also_to` (people who just left the member list).
    pub(super) fn publish_group(&self, gid: &str, also_to: &[PublicKey]) {
        let Some((state, _, _)) = self.db.group(gid) else {
            return;
        };
        let mut to: Vec<PublicKey> = state
            .members
            .iter()
            .filter_map(|m| parse_key(m))
            .filter(|m| *m != self.me)
            .collect();
        to.extend_from_slice(also_to);
        for p in to {
            self.send_group_state(p, gid);
        }
    }

    /// A connection to someone we share a group with just came up.
    pub(super) fn on_group_session(&mut self, peer: PublicKey) {
        let id = key_to_string(&peer);
        let groups = self.db.groups_with_member(&id);
        if !groups.is_empty() {
            self.set_online(peer, true, None);
            for g in &groups {
                self.send_group_state(peer, g);
            }
            for m in self.db.group_unacked_for(&id) {
                self.send_message(peer, &m);
            }
            // What each of us has, so nobody has to wait for whoever wrote something.
            for g in &groups {
                let ids: Vec<[u8; 16]> = self
                    .db
                    .messages(g, None, RELAY_LOOK_BACK)
                    .iter()
                    .filter_map(|m| crate::protocol::id_to_bytes(&m.id))
                    .collect();
                self.send(
                    peer,
                    Frame::GroupHave {
                        group: g.clone(),
                        ids,
                    },
                );
            }
        }
        // A leave the admin has not heard about yet.
        for g in self.db.pending_leaves_to(&id) {
            self.send(peer, Frame::GroupLeave { group: g });
        }
    }

    /// Sends a message of ours in a group to everyone connected.
    pub(super) fn send_group_message(&mut self, gid: &str, message_id: &str) {
        let (Some(m), Some((state, _, true))) = (self.db.message(message_id), self.db.group(gid))
        else {
            return;
        };
        for member in state.members.iter().filter_map(|k| parse_key(k)) {
            if member != self.me && self.sessions.contains_key(&member) {
                self.send_message(member, &m);
            }
        }
    }

    pub(super) fn on_group_update(
        &mut self,
        peer: PublicKey,
        state: GroupState,
        sig: Vec<u8>,
        names: Vec<(String, String)>,
        secret: [u8; 32],
    ) {
        let sender = key_to_string(&peer);
        let me = self.me_id();
        if !groups::is_group_id(&state.id)
            || state.members.len() > MAX_GROUP_MEMBERS
            || state.name.is_empty()
            || clean_name(&state.name) != state.name
            || !groups::verify(&state, &sig)
        {
            return;
        }
        let local = self.db.group(&state.id);
        // The last state of a group can leave nobody but the one telling: an owner who left.
        let in_it = state.members.contains(&sender)
            || local
                .as_ref()
                .is_some_and(|(cur, _, _)| cur.members.contains(&sender));
        if !in_it || (!state.closed && !state.members.contains(&state.admin)) {
            return;
        }
        let asked = self.db.secret(&join_asked_name(&state.id)).is_some();
        match &local {
            Some((cur, cur_sig, _)) => {
                if !groups::may_change(cur, &state) {
                    return;
                }
                // Two admins at the same moment: the same one wins everywhere.
                if state.version < cur.version
                    || (state.version == cur.version && sig.as_slice() <= cur_sig.as_slice())
                {
                    return;
                }
            }
            None => {
                // Only a contact's group, or one we asked to join, can add us.
                let by_contact = state.members.iter().any(|m| {
                    state.is_manager(m)
                        && self
                            .db
                            .contact(m)
                            .is_some_and(|c| c.state == ContactState::Accepted)
                });
                if !(by_contact || asked) || !state.members.contains(&me) || state.closed {
                    return;
                }
            }
        }
        if asked && state.members.contains(&me) {
            self.joined_through_link(&state);
        }
        if let Some((cur, _, _)) = &local {
            for gone in cur.members.iter().filter(|m| !state.members.contains(m)) {
                self.db.remove_member_acks(&state.id, gone);
            }
        }
        let names: Vec<(String, String)> = names
            .into_iter()
            .map(|(k, n)| (k, clean_name(&n)))
            .collect();
        let still_in = state.members.contains(&me) && !state.closed;
        if self
            .db
            .save_group(&state, &sig, still_in, &names, &me)
            .is_err()
        {
            return;
        }
        if !state.members.contains(&me) {
            self.db.set_group_left(&state.id, false);
        }
        let _ = self.db.set_secret(&groups::secret_name(&state.id), &secret);
        self.published = None;
        self.db.prune_group_peers();
        self.refresh_group_messages(&state.id);
        // Group members talk to each other directly: meet the ones we have no connection to.
        if still_in {
            for member in state.members.iter().filter(|m| **m != me) {
                if let Some(key) = crate::identity::parse_key(member)
                    && !self.sessions.contains_key(&key)
                {
                    // Worth a few tries: they may not have heard of the group yet either.
                    self.blips.insert(key, Instant::now());
                    self.wanted_dirty = true;
                }
            }
        }
        emit(&self.events, CoreEvent::ContactsChanged);
    }

    pub(super) fn on_group_leave(&mut self, peer: PublicKey, gid: String) {
        let sender = key_to_string(&peer);
        let me = self.me_id();
        let Some((mut state, _, true)) = self.db.group(&gid) else {
            return;
        };
        // Only the owner and admins change the member list; everyone else follows the next signed
        // state. Only the owner takes away an admin.
        let may = state.admin == me || (state.is_manager(&me) && !state.is_manager(&sender));
        if !may || !state.members.contains(&sender) || sender == me {
            return;
        }
        state.members.retain(|m| *m != sender);
        state.rules.admins.retain(|m| *m != sender);
        state.version += 1;
        self.db.remove_member_acks(&gid, &sender);
        self.save_and_publish(state, &[peer]);
    }

    /// Signs a new state of a group we administer, stores it and sends it.
    pub(super) fn save_and_publish(&mut self, mut state: GroupState, also_to: &[PublicKey]) {
        state.rules.signer = self.me_id();
        let sig = groups::sign(&self.key, &state);
        let names = self.group_names(&state);
        let me = self.me_id();
        let active = state.members.contains(&me) && !state.closed;
        if self
            .db
            .save_group(&state, &sig, active, &names, &me)
            .is_ok()
        {
            self.refresh_group_messages(&state.id);
            self.publish_group(&state.id, also_to);
            emit(&self.events, CoreEvent::ContactsChanged);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_group_msg(
        &mut self,
        peer: PublicKey,
        mid: [u8; 16],
        text: String,
        sent_at: i64,
        group: String,
        extra: Vec<u8>,
        sig: Vec<u8>,
    ) {
        let sender = key_to_string(&peer);
        if text.len() > MAX_TEXT || !self.group_allows(&group, &sender) {
            return;
        }
        let mut parsed = crate::rich::MessageExtra::from_wire(&extra);
        // Whatever cannot be checked is not passed on, but they are who they are: it still counts.
        if groups::verify_message(&peer, &group, &mid, &text, sent_at, &extra, &sig) {
            parsed.proof = Some(crate::rich::GroupProof {
                text: text.clone(),
                sent_at,
                wire: extra,
                sig,
            });
        }
        let message = Message {
            id: id_from_bytes(&mid),
            contact_id: group,
            outgoing: false,
            body: text,
            sent_at: sent_at.min(now_ms()),
            received_at: now_ms(),
            status: MessageStatus::Delivered,
            read: false,
            attachment: None,
            sender: Some(sender),
            extra: parsed,
        };
        if self.db.insert_message(&message).unwrap_or(false) {
            emit(&self.events, CoreEvent::MessageAdded(message));
            emit(&self.events, CoreEvent::ContactsChanged);
        }
        self.send(
            peer,
            Frame::Receipt {
                kind: ReceiptKind::Delivered,
                ids: vec![mid],
            },
        );
    }

    /// May `sender` write in `group` as far as we are concerned?
    pub(super) fn group_allows(&self, group: &str, sender: &str) -> bool {
        if !self.db.is_group_member(group, sender) || !self.db.is_group_member(group, &self.me_id())
        {
            return false;
        }
        self.db
            .group(group)
            .is_some_and(|(state, _, _)| !state.rules.only_admins_send || state.is_manager(sender))
    }

    /// Someone says what they have of a group: pass on what they lack, as far as they were in it.
    pub(super) fn on_group_have(&self, peer: PublicKey, group: &str, ids: &[[u8; 16]]) {
        let who = key_to_string(&peer);
        let Some(since) = self.db.member_since(group, &who) else {
            return;
        };
        if !self.db.is_group_member(group, &self.me_id()) {
            return;
        }
        for m in self.db.messages(group, None, RELAY_LOOK_BACK) {
            let Some(mid) = crate::protocol::id_to_bytes(&m.id) else {
                continue;
            };
            if ids.contains(&mid) || m.sent_at < since || m.attachment.is_some() {
                continue;
            }
            // Whoever wrote it knows who has it already: that goes its own way.
            if m.sender.as_deref() == Some(who.as_str()) {
                continue;
            }
            if m.outgoing {
                if m.status == MessageStatus::Scheduled || m.extra.unsent {
                    continue;
                }
                let wire = m.extra.to_wire();
                let sig = groups::sign_message(&self.key, group, &mid, &m.body, m.sent_at, &wire);
                self.send(
                    peer,
                    Frame::GroupRelay {
                        group: group.to_owned(),
                        author: *self.me.as_bytes(),
                        id: mid,
                        text: m.body,
                        sent_at: m.sent_at,
                        extra: wire,
                        sig,
                    },
                );
                continue;
            }
            let (Some(proof), Some(author)) = (
                m.extra.proof.clone(),
                m.sender.as_deref().and_then(parse_key),
            ) else {
                continue;
            };
            if m.extra.unsent {
                continue;
            }
            self.send(
                peer,
                Frame::GroupRelay {
                    group: group.to_owned(),
                    author: *author.as_bytes(),
                    id: mid,
                    text: proof.text,
                    sent_at: proof.sent_at,
                    extra: proof.wire,
                    sig: proof.sig,
                },
            );
        }
    }

    /// Something someone else in the group wrote, passed on. It counts because they signed it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_group_relay(
        &mut self,
        peer: PublicKey,
        group: String,
        author: [u8; 32],
        mid: [u8; 16],
        text: String,
        sent_at: i64,
        wire: Vec<u8>,
        sig: Vec<u8>,
    ) {
        let (who_tells, Ok(author_key)) = (key_to_string(&peer), PublicKey::from_bytes(&author))
        else {
            return;
        };
        let writer = key_to_string(&author_key);
        if writer == self.me_id()
            || text.len() > MAX_TEXT
            || !self.group_allows(&group, &who_tells)
            || !self.group_allows(&group, &writer)
            || !groups::verify_message(&author_key, &group, &mid, &text, sent_at, &wire, &sig)
        {
            return;
        }
        let mut parsed = crate::rich::MessageExtra::from_wire(&wire);
        parsed.proof = Some(crate::rich::GroupProof {
            text: text.clone(),
            sent_at,
            wire,
            sig,
        });
        let message = Message {
            id: id_from_bytes(&mid),
            contact_id: group,
            outgoing: false,
            body: text,
            sent_at: sent_at.min(now_ms()),
            received_at: now_ms(),
            status: MessageStatus::Delivered,
            read: false,
            attachment: None,
            sender: Some(writer),
            extra: parsed,
        };
        if self.db.insert_message(&message).unwrap_or(false) {
            self.timers_dirty = true;
            emit(&self.events, CoreEvent::MessageAdded(message));
            emit(&self.events, CoreEvent::ContactsChanged);
        }
    }

    // ---- joining by link ----

    /// Someone used a link to one of our groups.
    pub(super) fn on_join_ask(
        &mut self,
        peer: PublicKey,
        join: &crate::requests::JoinAsk,
        name: &str,
        request: &crate::requests::Request,
    ) {
        let id = key_to_string(&peer);
        let me = self.me_id();
        let Some((mut state, _, true)) = self.db.group(&join.group) else {
            return;
        };
        let Some(mut link) = group_link_state(&self.db, &join.group) else {
            return;
        };
        if link.token != join.token || !state.is_manager(&me) || state.members.contains(&id) {
            return;
        }
        if state.members.len() >= MAX_GROUP_MEMBERS {
            return;
        }
        // To find them, and for the group to go on.
        if self.db.contact(&id).is_none()
            && self
                .db
                .upsert_contact(&id, ContactState::GroupPeer, Some(name), None)
                .is_err()
        {
            return;
        }
        let mut keys = self.db.contact_keys(&id).unwrap_or_default();
        keys.location = Some(request.location);
        self.db.set_contact_keys(&id, &keys);
        if link.approve {
            if !link.asking.iter().any(|a| a.id == id) {
                link.asking.push(crate::model::JoinAsker {
                    id: id.clone(),
                    name: name.to_owned(),
                    greeting: request.greeting.chars().take(140).collect(),
                });
                save_group_link_state(&self.db, &join.group, &link);
                emit(
                    &self.events,
                    CoreEvent::RequestReceived {
                        contact_id: id,
                        name: name.to_owned(),
                    },
                );
                emit(&self.events, CoreEvent::ContactsChanged);
            }
            return;
        }
        state.members.push(id);
        state.members.sort();
        state.version += 1;
        self.save_and_publish(state, &[]);
        self.blips.insert(peer, Instant::now());
        self.wanted_dirty = true;
    }

    /// We are in: who let us in is no longer someone we are waiting for.
    fn joined_through_link(&mut self, state: &GroupState) {
        let _ = self.db.delete_secret(&join_asked_name(&state.id));
        for member in &state.members {
            let Some(mut keys) = self.db.contact_keys(member) else {
                continue;
            };
            if keys.join.as_ref().is_some_and(|j| j.group == state.id) {
                keys.join = None;
                keys.invite = None;
                self.db.set_contact_keys(member, &keys);
                if let Some(key) = parse_key(member) {
                    self.requests_out.remove(&key);
                }
                if self.db.contact(member).map(|c| c.state) == Some(ContactState::PendingOut) {
                    let _ = self
                        .db
                        .upsert_contact(member, ContactState::GroupPeer, None, None);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_group_file_offer(
        &mut self,
        peer: PublicKey,
        mid: [u8; 16],
        sent_at: i64,
        attachment: Attachment,
        caption: String,
        group: String,
        extra: Vec<u8>,
    ) {
        let sender = key_to_string(&peer);
        if caption.len() > MAX_TEXT || !self.group_allows(&group, &sender) {
            return;
        }
        self.on_file_offer(
            peer,
            Message {
                id: id_from_bytes(&mid),
                contact_id: group,
                outgoing: false,
                body: caption,
                sent_at: sent_at.min(now_ms()),
                received_at: now_ms(),
                status: MessageStatus::Delivered,
                read: false,
                attachment: Some(attachment),
                sender: Some(sender),
                extra: crate::rich::MessageExtra::from_wire(&extra),
            },
        );
    }

    /// A member confirmed (or, for a file, resolved) one of our group
    /// messages. True if the message was a group message.
    pub(super) fn group_receipt(&mut self, peer: PublicKey, mid: &[u8; 16]) -> bool {
        let id = key_to_string(&peer);
        let message_id = id_from_bytes(mid);
        let Some(m) = self.db.message(&message_id) else {
            return false;
        };
        if !m.outgoing || !groups::is_group_id(&m.contact_id) {
            return false;
        }
        if self.db.is_group_member(&m.contact_id, &id) {
            self.db.set_group_ack(&message_id, &id, 2);
            self.refresh_group_message(&message_id);
        }
        true
    }

    /// Once every member has the message, it is delivered.
    fn refresh_group_message(&mut self, message_id: &str) {
        let (got, total) = self.db.group_ack_counts(message_id);
        if total == 0 || got < total {
            return;
        }
        let Some(m) = self.db.message(message_id) else {
            return;
        };
        if self
            .db
            .advance_status(&m.contact_id, message_id, MessageStatus::Delivered)
        {
            emit(
                &self.events,
                CoreEvent::MessageStatus {
                    contact_id: m.contact_id.clone(),
                    message_id: message_id.to_string(),
                    status: MessageStatus::Delivered,
                },
            );
        }
        if m.attachment
            .as_ref()
            .is_some_and(|a| a.state == TransferState::Transferring)
            && let Some(m) = self.db.set_transfer(message_id, TransferState::Done, None)
        {
            emit(&self.events, CoreEvent::MessageUpdated(m));
        }
    }

    /// After the member list changed, messages everyone remaining has may now be delivered.
    pub(super) fn refresh_group_messages(&mut self, gid: &str) {
        for m in self.db.group_pending_messages(gid) {
            self.refresh_group_message(&m.id);
        }
    }

    /// A member refused a file (over their limit): that is their answer.
    pub(super) fn group_declined(&mut self, peer: PublicKey, mid: &[u8; 16]) -> bool {
        self.group_receipt(peer, mid)
    }

    pub(super) fn may_fetch(
        &self,
        m: &Message,
        peer_id: &str,
        state: Option<ContactState>,
    ) -> bool {
        (m.contact_id == peer_id && state == Some(ContactState::Accepted))
            || (groups::is_group_id(&m.contact_id)
                && self.db.is_group_member(&m.contact_id, peer_id))
    }
}
