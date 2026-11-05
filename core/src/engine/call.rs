//! Calls, one at a time per device: 1:1, or a group call where everyone
//! connects to everyone.
//!
//! Signalling is the same for both. Whoever starts a call sends
//! `CallInvite` to the other side (or to every online group member).
//! Anyone who accepts sends `CallAnswer(Accepted)` to *all* the other
//! members, so every participant learns who is in and can start sending
//! media to them. `CallEnd` means "I left the call" and also goes to all
//! members, which is how people who are not in the call know when it is over.

use std::collections::{HashMap, HashSet};

use iroh::PublicKey;
use iroh::endpoint::Connection;
use tokio::time::Instant;

use super::{Engine, emit};
use crate::calls::{CallEndReason, CallEvent, CallPhase, CallSnapshot, ROOM_MOST, room_call_id};
use crate::identity::{key_to_string, parse_key};
use crate::model::{ContactState, CoreEvent, GROUP_ID_PREFIX};
use crate::protocol::{CallResult, Frame, RING_TIMEOUT, id_from_bytes};

pub(super) struct CallState {
    pub id: [u8; 16],
    /// The group this call belongs to, for a group call.
    pub group: Option<String>,
    pub video: bool,
    /// We started it.
    pub outgoing: bool,
    pub started: Instant,
    /// We are in the call (we answered, or someone answered us).
    pub connected: Option<Instant>,
    /// Asked and not answered yet.
    pub invited: HashSet<PublicKey>,
    /// In the call right now, apart from us.
    pub peers: HashSet<PublicKey>,
    /// Peers we have told that we are in.
    pub told: HashSet<PublicKey>,
    /// Everyone who may take part.
    pub members: HashSet<PublicKey>,
    /// Who rang us (callee side).
    pub caller: Option<PublicKey>,
    /// A drop-in room: nobody rings, being alone is fine, and it stays out of the list of calls.
    pub room: bool,
    /// Our hand is up, in a room.
    pub hand: bool,
    /// Who is showing their screen to us.
    pub sharer: Option<PublicKey>,
    /// We are showing our screen: how big it is, how many frames so far, and the streams it goes
    /// out on, one per viewer.
    pub showing: Option<Showing>,
}

pub(super) struct Showing {
    pub width: u32,
    pub height: u32,
    pub number: u32,
    pub streams: HashMap<PublicKey, tokio::sync::mpsc::Sender<bytes::Bytes>>,
}

/// A group call somebody is in: it can be joined while it lasts.
pub(super) struct GroupCall {
    pub call: [u8; 16],
    pub video: bool,
    pub participants: HashSet<PublicKey>,
}

impl Engine {
    fn emit_call(&self, ev: CallEvent) {
        emit(&self.events, CoreEvent::Call(ev));
    }

    pub(super) fn set_active_call(&self, value: Option<([u8; 16], HashSet<PublicKey>)>) {
        *self
            .media_in
            .active
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = value;
    }

    /// Connections to everyone who is in the call.
    pub(super) fn call_conns(&self) -> Option<([u8; 16], Vec<Connection>)> {
        let c = self.call.as_ref().filter(|c| c.connected.is_some())?;
        let conns = c
            .peers
            .iter()
            .filter_map(|p| self.sessions.get(p).map(|s| s.conn.clone()))
            .collect();
        Some((c.id, conns))
    }

    fn conversation_of(&self, c: &CallState) -> String {
        c.group.clone().unwrap_or_else(|| {
            c.members
                .iter()
                .next()
                .map(key_to_string)
                .unwrap_or_default()
        })
    }

    /// Keeps the shared description of the current call up to date.
    pub(super) fn publish_call_state(&self) {
        let snapshot = self.call.as_ref().map(|c| {
            let mut peers: Vec<String> = c.peers.iter().map(key_to_string).collect();
            peers.sort();
            let phase = match (c.connected, c.outgoing) {
                (Some(_), _) => CallPhase::Active,
                (None, true) => CallPhase::RingingOut,
                (None, false) => CallPhase::RingingIn,
            };
            CallSnapshot {
                call_id: id_from_bytes(&c.id),
                contact_id: self.conversation_of(c),
                from: c
                    .caller
                    .map(|p| key_to_string(&p))
                    .unwrap_or_else(|| key_to_string(&self.me)),
                video: c.video,
                phase,
                connected_since: c
                    .connected
                    .map(|t| crate::model::now_ms() - t.elapsed().as_millis() as i64),
                peers,
                sharer: c.sharer.as_ref().map(key_to_string),
            }
        });
        *self.shared.call.lock().unwrap_or_else(|p| p.into_inner()) = snapshot;
    }

    fn announce_peers(&self) {
        self.publish_call_state();
        if let Some(c) = &self.call {
            let mut peers: Vec<String> = c.peers.iter().map(key_to_string).collect();
            peers.sort();
            self.emit_call(CallEvent::Participants {
                call_id: id_from_bytes(&c.id),
                peers,
            });
        }
    }

    /// Ends whatever call there is and tells the UI why.
    pub(super) fn end_call(&mut self, reason: CallEndReason) {
        self.control_call_or_sharing_over();
        self.control.can_be_controlled = false;
        let Some(c) = self.call.take() else { return };
        self.set_active_call(None);
        self.publish_call_state();
        let duration_ms = c
            .connected
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        if !c.room {
            self.db.add_call_record(&crate::rich::CallRecord {
                id: id_from_bytes(&c.id),
                contact_id: self.conversation_of(&c),
                outgoing: c.outgoing,
                video: c.video,
                at: crate::model::now_ms() - c.started.elapsed().as_millis() as i64,
                duration_ms,
                missed: c.connected.is_none(),
            });
        }
        self.emit_call(CallEvent::Ended {
            call_id: id_from_bytes(&c.id),
            contact_id: self.conversation_of(&c),
            reason,
            duration_ms,
        });
    }

    fn group_call_changed(&self, group: &str, active: bool, video: bool) {
        let mut map = self
            .shared
            .group_calls
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if active {
            map.insert(group.to_string(), video);
        } else {
            map.remove(group);
        }
        drop(map);
        self.emit_call(CallEvent::GroupCall {
            group_id: group.to_string(),
            active,
            video,
        });
    }

    /// Someone (not necessarily us) joined or left a group call.
    fn track_participant(&mut self, group: &str, call: [u8; 16], who: PublicKey, joined: bool) {
        let Some(gc) = self.group_calls.get_mut(group).filter(|g| g.call == call) else {
            return;
        };
        if joined {
            gc.participants.insert(who);
        } else {
            gc.participants.remove(&who);
        }
        let people: Vec<String> = gc.participants.iter().map(key_to_string).collect();
        if gc.participants.is_empty() {
            let video = gc.video;
            self.group_calls.remove(group);
            self.group_call_changed(group, false, video);
        }
        self.shared
            .group_call_people
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(group.to_owned(), people);
        emit(&self.events, CoreEvent::ContactsChanged);
    }

    // ---- starting and answering ----

    pub(super) fn call_invite(&mut self, target: String, call: [u8; 16], video: bool) {
        let ended = |e: &Engine, reason| {
            e.emit_call(CallEvent::Ended {
                call_id: id_from_bytes(&call),
                contact_id: target.clone(),
                reason,
                duration_ms: 0,
            });
        };
        if self.call.is_some() {
            return ended(self, CallEndReason::Busy);
        }
        let (group, members): (Option<String>, HashSet<PublicKey>) =
            if target.starts_with(GROUP_ID_PREFIX) {
                let Some((state, _, true)) = self.db.group(&target) else {
                    return ended(self, CallEndReason::Lost);
                };
                if state.rules.room {
                    // A room has no ringing: this is coming in.
                    return self.call_join(target);
                }
                let members = state
                    .members
                    .iter()
                    .filter_map(|m| parse_key(m))
                    .filter(|m| *m != self.me)
                    .collect();
                (Some(target.clone()), members)
            } else {
                let Some(peer) = parse_key(&target) else {
                    return ended(self, CallEndReason::Lost);
                };
                (None, HashSet::from([peer]))
            };
        let invited: HashSet<PublicKey> = members
            .iter()
            .copied()
            .filter(|m| self.sessions.contains_key(m))
            .collect();
        // Whoever is not connected gets dialled, and rung as soon as they are there.
        for m in &members {
            if !invited.contains(m) {
                self.backoff.remove(m);
            }
        }
        if members.is_empty() {
            return ended(self, CallEndReason::Lost);
        }
        if let Some(g) = &group {
            self.group_calls.insert(
                g.clone(),
                GroupCall {
                    call,
                    video,
                    participants: HashSet::from([self.me]),
                },
            );
            self.group_call_changed(g, true, video);
        }
        for p in &invited {
            self.send(
                *p,
                Frame::CallInvite {
                    call,
                    video,
                    group: group.clone(),
                },
            );
        }
        self.set_active_call(Some((call, members.clone())));
        self.call = Some(CallState {
            id: call,
            group,
            video,
            outgoing: true,
            started: Instant::now(),
            connected: None,
            invited,
            peers: HashSet::new(),
            told: HashSet::new(),
            members,
            caller: None,
            room: false,
            hand: false,
            sharer: None,
            showing: None,
        });
        self.publish_call_state();
    }

    /// Becomes part of the call: tells everyone who can be told.
    fn enter_call(&mut self) {
        let Some(c) = self.call.as_mut() else { return };
        c.connected = Some(Instant::now());
        let (call, group, targets) = (
            c.id,
            c.group.clone(),
            c.members
                .iter()
                .copied()
                .filter(|m| self.sessions.contains_key(m))
                .collect::<Vec<_>>(),
        );
        let conversation = self.conversation_of(self.call.as_ref().expect("call"));
        if let Some(g) = &group {
            self.track_participant(g, call, self.me, true);
        }
        for t in targets {
            self.send(
                t,
                Frame::CallAnswer {
                    call,
                    result: CallResult::Accepted,
                },
            );
            if let Some(c) = self.call.as_mut() {
                c.told.insert(t);
            }
        }
        self.emit_call(CallEvent::Connected {
            call_id: id_from_bytes(&call),
            contact_id: conversation,
        });
        self.announce_peers();
    }

    pub(super) fn call_answer(&mut self, accept: bool) {
        let Some(c) = self
            .call
            .as_ref()
            .filter(|c| !c.outgoing && c.connected.is_none())
        else {
            return;
        };
        let (call, caller) = (c.id, c.caller);
        if accept {
            self.enter_call();
        } else {
            if let Some(p) = caller {
                self.send(
                    p,
                    Frame::CallAnswer {
                        call,
                        result: CallResult::Declined,
                    },
                );
            }
            self.end_call(CallEndReason::Declined);
        }
    }

    /// Joins a group call that is already going on.
    pub(super) fn call_join(&mut self, group: String) {
        if self.call.is_some() {
            return;
        }
        let Some((state, _, true)) = self.db.group(&group) else {
            return;
        };
        let room = state.rules.room;
        if room {
            let call = room_call_id(&group);
            let in_it = self
                .group_calls
                .get(&group)
                .map_or(0, |gc| gc.participants.len());
            if in_it >= ROOM_MOST {
                self.emit_call(CallEvent::RoomFull { group_id: group });
                return;
            }
            if !self.group_calls.contains_key(&group) {
                self.group_calls.insert(
                    group.clone(),
                    GroupCall {
                        call,
                        video: false,
                        participants: HashSet::new(),
                    },
                );
                self.group_call_changed(&group, true, false);
            }
            // Whoever is around hears the room is on, without anything ringing.
            for m in state.members.iter().filter_map(|m| parse_key(m)) {
                if m != self.me && self.sessions.contains_key(&m) {
                    self.send(
                        m,
                        Frame::CallInvite {
                            call,
                            video: false,
                            group: Some(group.clone()),
                        },
                    );
                }
            }
        }
        let Some(gc) = self.group_calls.get(&group) else {
            return;
        };
        let (call, video) = (gc.call, gc.video);
        let peers: HashSet<PublicKey> = gc
            .participants
            .iter()
            .copied()
            .filter(|p| *p != self.me)
            .collect();
        let members: HashSet<PublicKey> = state
            .members
            .iter()
            .filter_map(|m| parse_key(m))
            .filter(|m| *m != self.me)
            .collect();
        let mut allowed = members.clone();
        allowed.extend(peers.iter().copied());
        self.set_active_call(Some((call, allowed)));
        self.call = Some(CallState {
            id: call,
            group: Some(group),
            video,
            outgoing: false,
            started: Instant::now(),
            connected: None,
            invited: HashSet::new(),
            peers,
            told: HashSet::new(),
            members,
            caller: None,
            room,
            hand: false,
            sharer: None,
            showing: None,
        });
        self.publish_call_state();
        self.enter_call();
    }

    /// Our hand goes up or down, in a room.
    pub(super) fn call_hand(&mut self, up: bool) {
        let Some(c) = self.call.as_mut().filter(|c| c.room) else {
            return;
        };
        c.hand = up;
        let (call, peers) = (c.id, c.peers.iter().copied().collect::<Vec<_>>());
        for p in peers {
            self.send(p, Frame::CallHand { call, up });
        }
    }

    /// Tells everyone we are out of this call, and notes it for ourselves.
    fn say_we_left(&mut self) {
        let Some(c) = self.call.as_ref() else { return };
        let (call, group) = (c.id, c.group.clone());
        let targets: HashSet<PublicKey> = c.members.iter().chain(c.peers.iter()).copied().collect();
        for t in targets {
            self.send(t, Frame::CallEnd { call });
        }
        if let Some(g) = &group {
            self.track_participant(g, call, self.me, false);
        }
    }

    /// Leave the call (or cancel it while it rings).
    pub(super) fn call_hangup(&mut self) {
        let Some(c) = self.call.as_ref() else { return };
        let ringing_out = c.outgoing && c.connected.is_none();
        self.say_we_left();
        self.end_call(if ringing_out {
            CallEndReason::Cancelled
        } else {
            CallEndReason::Hangup
        });
    }

    pub(super) fn call_video_on(&self, on: bool) {
        let Some(c) = self.call.as_ref().filter(|c| c.connected.is_some()) else {
            return;
        };
        for p in c.peers.iter() {
            self.send(*p, Frame::CallVideo { call: c.id, on });
        }
    }

    /// We started or stopped showing our screen. Closing the streams is what ends them.
    pub(super) fn call_screen_on(&mut self, on: bool, width: u32, height: u32, can_control: bool) {
        if !on {
            self.control_call_or_sharing_over();
        }
        self.control.can_be_controlled = on && can_control;
        let can_control = self.control.can_be_controlled;
        let Some(c) = self.call.as_mut().filter(|c| c.connected.is_some()) else {
            return;
        };
        c.showing = on.then(|| Showing {
            width,
            height,
            number: 0,
            streams: HashMap::new(),
        });
        let (call, peers) = (c.id, c.peers.iter().copied().collect::<Vec<_>>());
        for p in peers {
            self.send(
                p,
                Frame::CallScreen {
                    call,
                    on,
                    width,
                    height,
                    can_control,
                },
            );
        }
    }

    /// One frame of our screen, to everyone watching. A viewer who is behind misses it, and says
    /// so when the number jumps.
    pub(super) fn call_screen_frame(
        &mut self,
        ts: u32,
        key: bool,
        codec: crate::calls::ScreenCodec,
        data: &[u8],
    ) {
        if data.len() > crate::protocol::MAX_VIDEO_FRAME {
            return;
        }
        let conns: Vec<(PublicKey, Connection)> = self
            .call
            .as_ref()
            .into_iter()
            .flat_map(|c| c.peers.iter())
            .filter_map(|p| self.sessions.get(p).map(|s| (*p, s.conn.clone())))
            .collect();
        let Some(c) = self.call.as_mut() else { return };
        let call = c.id;
        let Some(showing) = c.showing.as_mut() else {
            return;
        };
        let number = showing.number;
        showing.number = showing.number.wrapping_add(1);
        let frame = crate::calls::screen_frame(number, ts, key, codec, data);
        showing
            .streams
            .retain(|p, tx| !tx.is_closed() && conns.iter().any(|(q, _)| q == p));
        for (peer, conn) in conns {
            let tx = showing
                .streams
                .entry(peer)
                .or_insert_with(|| crate::calls::open_screen_stream(conn, call));
            let _ = tx.try_send(frame.clone());
        }
    }

    /// We lost our place in the screen we are watching.
    pub(super) fn call_screen_key(&self) {
        let Some(c) = self.call.as_ref() else { return };
        if let Some(sharer) = c.sharer {
            self.send(sharer, Frame::CallKeyRequest { call: c.id });
        }
    }

    /// Gives up on a call nobody answered.
    pub(super) fn call_tick(&mut self) {
        let Some(c) = self.call.as_ref() else { return };
        if c.connected.is_some() || c.started.elapsed() < RING_TIMEOUT {
            return;
        }
        if c.outgoing {
            let (call, invited): (_, Vec<_>) = (c.id, c.invited.iter().copied().collect());
            for p in invited {
                self.send(p, Frame::CallEnd { call });
            }
            if let Some(g) = self.call.as_ref().and_then(|c| c.group.clone()) {
                self.track_participant(&g, call, self.me, false);
            }
        }
        self.end_call(CallEndReason::Missed);
    }

    // ---- what happens when somebody else does something ----

    /// Someone is no longer in the call (left, or the connection dropped).
    pub(super) fn call_peer_gone(&mut self, peer: PublicKey, lost: bool) {
        let Some(c) = self.call.as_mut() else { return };
        let was_peer = c.peers.remove(&peer);
        c.invited.remove(&peer);
        c.told.remove(&peer);
        if c.sharer == Some(peer) {
            c.sharer = None;
            let call_id = id_from_bytes(&c.id);
            self.control_stop_driving_if(peer);
            self.emit_call(CallEvent::PeerScreen {
                call_id,
                from: key_to_string(&peer),
                on: false,
                width: 0,
                height: 0,
                can_control: false,
            });
        }
        if was_peer {
            self.announce_peers();
        }
        let Some(c) = self.call.as_ref() else { return };
        if c.room {
            return;
        }
        let empty = c.peers.is_empty();
        if c.connected.is_some() {
            if empty {
                // The last one in a group call says so too: else everyone who heard of the call
                // goes on seeing it, and this device as well.
                if c.group.is_some() {
                    self.say_we_left();
                }
                self.end_call(if lost {
                    CallEndReason::Lost
                } else {
                    CallEndReason::Hangup
                });
            }
        } else if c.outgoing {
            if empty && c.invited.is_empty() {
                self.end_call(if lost {
                    CallEndReason::Lost
                } else {
                    CallEndReason::Declined
                });
            }
        } else if empty {
            // Still ringing and everyone who could pick up is gone.
            self.end_call(CallEndReason::Missed);
        }
    }

    pub(super) fn on_call_frame(&mut self, peer: PublicKey, frame: Frame) {
        let id = key_to_string(&peer);
        match frame {
            Frame::CallInvite { call, video, group } => {
                let allowed = match &group {
                    Some(g) => {
                        self.db.is_group_member(g, &id)
                            && self.db.is_group_member(g, &key_to_string(&self.me))
                    }
                    None => self
                        .db
                        .contact(&id)
                        .is_some_and(|c| c.state == ContactState::Accepted),
                };
                if !allowed {
                    return;
                }
                if let Some(g) = &group
                    && self.db.group(g).is_some_and(|(s, _, _)| s.rules.room)
                {
                    // A room rings nobody. And it has one call, not one anyone can make up.
                    if call == room_call_id(g) && !self.group_calls.contains_key(g) {
                        self.group_calls.insert(
                            g.clone(),
                            GroupCall {
                                call,
                                video: false,
                                participants: HashSet::new(),
                            },
                        );
                        self.group_call_changed(g, true, false);
                    }
                    if call == room_call_id(g) {
                        self.track_participant(g, call, peer, true);
                    }
                    return;
                }
                if let Some(g) = &group {
                    let fresh = !self.group_calls.get(g).is_some_and(|gc| gc.call == call);
                    if fresh {
                        self.group_calls.insert(
                            g.clone(),
                            GroupCall {
                                call,
                                video,
                                participants: HashSet::from([peer]),
                            },
                        );
                        self.group_call_changed(g, true, video);
                    }
                }
                if let Some(c) = &self.call {
                    // A repeated invite for the call we already show is fine.
                    if c.id != call {
                        self.send(
                            peer,
                            Frame::CallAnswer {
                                call,
                                result: CallResult::Busy,
                            },
                        );
                    }
                    return;
                }
                let members: HashSet<PublicKey> = match &group {
                    Some(g) => self
                        .db
                        .group(g)
                        .map(|(s, _, _)| s.members)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|m| parse_key(m))
                        .filter(|m| *m != self.me)
                        .collect(),
                    None => HashSet::from([peer]),
                };
                self.set_active_call(Some((call, members.clone())));
                let conversation = group.clone().unwrap_or_else(|| id.clone());
                self.call = Some(CallState {
                    id: call,
                    group,
                    video,
                    outgoing: false,
                    started: Instant::now(),
                    connected: None,
                    invited: HashSet::new(),
                    peers: HashSet::from([peer]),
                    told: HashSet::new(),
                    members,
                    caller: Some(peer),
                    room: false,
                    hand: false,
                    sharer: None,
                    showing: None,
                });
                self.publish_call_state();
                self.emit_call(CallEvent::Incoming {
                    call_id: id_from_bytes(&call),
                    contact_id: conversation,
                    from: id,
                    video,
                });
            }
            Frame::CallAnswer { call, result } => {
                let group_of_call = self
                    .group_calls
                    .iter()
                    .find(|(_, g)| g.call == call)
                    .map(|(k, _)| k.clone());
                if result == CallResult::Accepted
                    && let Some(g) = &group_of_call
                {
                    self.track_participant(g, call, peer, true);
                }
                let Some(c) = self
                    .call
                    .as_mut()
                    .filter(|c| c.id == call && c.members.contains(&peer))
                else {
                    return;
                };
                match result {
                    CallResult::Accepted => {
                        c.invited.remove(&peer);
                        let newly = c.peers.insert(peer);
                        let first = c.outgoing && c.connected.is_none();
                        if first {
                            self.enter_call();
                        } else if newly {
                            self.announce_peers();
                        }
                        // Whoever just came in sees our hand, and what is being shown.
                        if let Some((call, true)) = self
                            .call
                            .as_ref()
                            .filter(|c| c.room)
                            .map(|c| (c.id, c.hand))
                        {
                            self.send(peer, Frame::CallHand { call, up: true });
                        }
                        if let Some((call, width, height)) = self
                            .call
                            .as_ref()
                            .and_then(|c| c.showing.as_ref().map(|s| (c.id, s.width, s.height)))
                        {
                            let can_control = self.control.can_be_controlled;
                            self.send(
                                peer,
                                Frame::CallScreen {
                                    call,
                                    on: true,
                                    width,
                                    height,
                                    can_control,
                                },
                            );
                        }
                        // Make sure they know we are in, too.
                        let tell = self
                            .call
                            .as_mut()
                            .filter(|c| c.connected.is_some() && !c.told.contains(&peer))
                            .map(|c| {
                                c.told.insert(peer);
                                c.id
                            });
                        if let Some(call) = tell {
                            self.send(
                                peer,
                                Frame::CallAnswer {
                                    call,
                                    result: CallResult::Accepted,
                                },
                            );
                        }
                    }
                    CallResult::Declined | CallResult::Busy => {
                        c.invited.remove(&peer);
                        let reason = if result == CallResult::Busy {
                            CallEndReason::Busy
                        } else {
                            CallEndReason::Declined
                        };
                        let one_to_one = c.group.is_none();
                        let waiting =
                            c.connected.is_none() && c.invited.is_empty() && c.peers.is_empty();
                        if (one_to_one || waiting) && c.outgoing {
                            self.end_call(reason);
                        }
                    }
                }
            }
            Frame::CallEnd { call } => {
                if let Some(g) = self
                    .group_calls
                    .iter()
                    .find(|(_, g)| g.call == call)
                    .map(|(k, _)| k.clone())
                {
                    self.track_participant(&g, call, peer, false);
                }
                if self.call.as_ref().is_some_and(|c| c.id == call) {
                    self.call_peer_gone(peer, false);
                }
            }
            Frame::CallScreen {
                call,
                on,
                width,
                height,
                can_control,
            } => {
                let Some(c) = self
                    .call
                    .as_mut()
                    .filter(|c| c.id == call && c.peers.contains(&peer))
                else {
                    return;
                };
                c.sharer = on.then_some(peer);
                if !on {
                    self.control_stop_driving_if(peer);
                }
                self.publish_call_state();
                self.emit_call(CallEvent::PeerScreen {
                    call_id: id_from_bytes(&call),
                    from: id,
                    on,
                    width,
                    height,
                    can_control,
                });
            }
            Frame::CallHand { call, up }
                if self
                    .call
                    .as_ref()
                    .is_some_and(|c| c.id == call && c.room && c.peers.contains(&peer)) =>
            {
                self.emit_call(CallEvent::PeerHand {
                    call_id: id_from_bytes(&call),
                    from: id,
                    up,
                });
            }
            Frame::CallKeyRequest { call }
                if self.call.as_ref().is_some_and(|c| {
                    c.id == call && c.peers.contains(&peer) && c.showing.is_some()
                }) =>
            {
                self.emit_call(CallEvent::ScreenKeyWanted {
                    call_id: id_from_bytes(&call),
                });
            }
            Frame::CallVideo { call, on }
                if self
                    .call
                    .as_ref()
                    .is_some_and(|c| c.id == call && c.peers.contains(&peer)) =>
            {
                self.emit_call(CallEvent::PeerVideo {
                    call_id: id_from_bytes(&call),
                    from: id,
                    on,
                });
            }
            _ => {}
        }
    }

    /// A connection dropped: drop the person from calls too.
    /// Someone we are ringing but could not reach before is there now.
    pub(super) fn call_on_session(&mut self, peer: PublicKey) {
        let Some(c) = self.call.as_mut() else { return };
        if c.room && c.connected.is_some() && c.members.contains(&peer) {
            let (call, group) = (c.id, c.group.clone());
            c.told.insert(peer);
            self.send(
                peer,
                Frame::CallInvite {
                    call,
                    video: false,
                    group,
                },
            );
            self.send(
                peer,
                Frame::CallAnswer {
                    call,
                    result: CallResult::Accepted,
                },
            );
            return;
        }
        if !c.outgoing
            || c.connected.is_some()
            || !c.members.contains(&peer)
            || !c.invited.insert(peer)
        {
            return;
        }
        let frame = Frame::CallInvite {
            call: c.id,
            video: c.video,
            group: c.group.clone(),
        };
        self.send(peer, frame);
    }

    pub(super) fn call_session_closed(&mut self, peer: PublicKey) {
        let groups: Vec<(String, [u8; 16])> = self
            .group_calls
            .iter()
            .filter(|(_, g)| g.participants.contains(&peer))
            .map(|(k, g)| (k.clone(), g.call))
            .collect();
        for (g, call) in groups {
            self.track_participant(&g, call, peer, false);
        }
        if self.call.is_some() {
            self.call_peer_gone(peer, true);
        }
    }
}

/// Group calls we know about, by group id.
pub(super) type GroupCalls = HashMap<String, GroupCall>;
