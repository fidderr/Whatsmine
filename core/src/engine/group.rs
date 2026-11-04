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
            || state.members.is_empty()
            || !state.members.contains(&state.admin)
            || !state.members.contains(&sender)
            || state.name.is_empty()
            || clean_name(&state.name) != state.name
            || !groups::verify(&state, &sig)
        {
            return;
        }
        let local = self.db.group(&state.id);
        match &local {
            Some((cur, _, _)) => {
                if cur.admin != state.admin || state.version <= cur.version {
                    return;
                }
            }
            None => {
                // Only a contact's group can add us; nobody else gets a say.
                let admin_is_contact = self
                    .db
                    .contact(&state.admin)
                    .is_some_and(|c| c.state == ContactState::Accepted);
                if !admin_is_contact || !state.members.contains(&me) || state.closed {
                    return;
                }
            }
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
        // Only the admin changes the member list; everyone else follows the
        // admin's next signed state.
        if state.admin != me || !state.members.contains(&sender) || sender == me {
            return;
        }
        state.members.retain(|m| *m != sender);
        state.version += 1;
        self.db.remove_member_acks(&gid, &sender);
        self.save_and_publish(state, &[peer]);
    }

    /// Signs a new state of a group we administer, stores it and sends it.
    pub(super) fn save_and_publish(&mut self, state: GroupState, also_to: &[PublicKey]) {
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

    pub(super) fn on_group_msg(
        &mut self,
        peer: PublicKey,
        mid: [u8; 16],
        text: String,
        sent_at: i64,
        group: String,
    ) {
        let sender = key_to_string(&peer);
        if text.len() > MAX_TEXT || !self.group_allows(&group, &sender) {
            return;
        }
        let message = Message {
            id: id_from_bytes(&mid),
            contact_id: group,
            outgoing: false,
            body: text,
            sent_at,
            received_at: now_ms(),
            status: MessageStatus::Delivered,
            read: false,
            attachment: None,
            sender: Some(sender),
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
        self.db.is_group_member(group, sender) && self.db.is_group_member(group, &self.me_id())
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
                sent_at,
                received_at: now_ms(),
                status: MessageStatus::Delivered,
                read: false,
                attachment: Some(attachment),
                sender: Some(sender),
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
