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
use crate::calls::{CallEndReason, CallEvent, CallPhase, CallSnapshot};
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
        let Some(c) = self.call.take() else { return };
        self.set_active_call(None);
        self.publish_call_state();
        let duration_ms = c
            .connected
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
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
        if gc.participants.is_empty() {
            let video = gc.video;
            self.group_calls.remove(group);
            self.group_call_changed(group, false, video);
        }
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
        if invited.is_empty() {
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
        let Some(gc) = self.group_calls.get(&group) else {
            return;
        };
        let Some((state, _, true)) = self.db.group(&group) else {
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
        });
        self.publish_call_state();
        self.enter_call();
    }

    /// Leave the call (or cancel it while it rings).
    pub(super) fn call_hangup(&mut self) {
        let Some(c) = self.call.as_ref() else { return };
        let (call, group) = (c.id, c.group.clone());
        let ringing_out = c.outgoing && c.connected.is_none();
        let targets: Vec<PublicKey> = c
            .members
            .iter()
            .chain(c.peers.iter())
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        for t in targets {
            self.send(t, Frame::CallEnd { call });
        }
        if let Some(g) = &group {
            self.track_participant(g, call, self.me, false);
        }
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
        if was_peer {
            self.announce_peers();
        }
        let Some(c) = self.call.as_ref() else { return };
        let empty = c.peers.is_empty();
        if c.connected.is_some() {
            if empty {
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
