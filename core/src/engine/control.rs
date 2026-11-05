//! Controlling a screen during a call, on the wire. What is allowed is decided in `crate::control`:
//! this moves the questions and answers, and starts and stops the stream of input.
//!
//! Input does not use the normal sealing, because each message of that writes to the database. The
//! answer, which is sealed like anything else, carries a key made for this session alone.

use iroh::PublicKey;
use iroh::endpoint::{Connection, RecvStream};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{Engine, Internal, emit};
use crate::calls::{CallEvent, MediaIn};
use crate::control::{Checks, Controlled, Ended, Input, MOST_IN_BATCH, RawFrame, Scope, Verdict};
use crate::identity::key_to_string;
use crate::model::{ContactState, CoreEvent, Millis, now_ms};
use crate::protocol::{CONTROL_STREAM_MAGIC, ControlKey, ControlReply, Frame, id_from_bytes};

/// How long a batch can be on the wire: well over what fits in one.
const MOST_SEALED: usize = 16 * 1024;
/// Without anything to send: often enough that a short hiccup does not end it.
const STILL_HERE_EVERY: std::time::Duration = std::time::Duration::from_millis(1500);

pub(super) struct ControlSide {
    /// A clock that only goes forward.
    born: Instant,
    pub owned: Controlled,
    /// The session we let someone into.
    session: Option<OwnedSession>,
    /// We asked, and may get an answer.
    asking: Option<PublicKey>,
    /// We are in control of someone.
    driving: Option<Driving>,
    /// Our screen could be controlled, for what is shown right now.
    pub can_be_controlled: bool,
}

struct OwnedSession {
    id: [u8; 16],
    key: [u8; 32],
    who: PublicKey,
    since: Millis,
}

struct Driving {
    who: PublicKey,
    tx: mpsc::Sender<Vec<Input>>,
    session: [u8; 16],
    key: [u8; 32],
}

impl ControlSide {
    pub fn new() -> Self {
        Self {
            born: Instant::now(),
            owned: Controlled::default(),
            session: None,
            asking: None,
            driving: None,
            can_be_controlled: false,
        }
    }

    fn now(&self) -> Millis {
        self.born.elapsed().as_millis() as Millis
    }
}

impl Engine {
    fn control_gate(&self, value: Option<([u8; 16], PublicKey)>) {
        *self
            .media_in
            .control
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = value;
    }

    fn control_checks(&self, peer: PublicKey) -> Checks {
        let c = self.call.as_ref();
        let wall = now_ms();
        let began = c.map_or(wall, |c| wall - c.started.elapsed().as_millis() as Millis);
        let contact = self.db.contact(&key_to_string(&peer));
        let is_contact = contact
            .as_ref()
            .is_some_and(|c| c.state == ContactState::Accepted);
        Checks {
            turned_on: self
                .settings()
                .control_on_since
                .is_some_and(|at| at < began),
            known_for: contact
                .filter(|_| is_contact)
                .map_or(0, |c| wall.saturating_sub(c.created_at)),
            one_to_one: c.is_some_and(|c| c.group.is_none() && c.peers.len() == 1),
            sharing: c.is_some_and(|c| c.showing.is_some()),
            possible: self.control.can_be_controlled,
        }
    }

    pub(super) fn on_control_frame(&mut self, peer: PublicKey, frame: Frame) {
        let Some(c) = self.call.as_ref() else { return };
        let call_id = id_from_bytes(&c.id);
        match frame {
            Frame::ControlAsk { call } if call == c.id && c.peers.contains(&peer) => {
                let (checks, now) = (self.control_checks(peer), self.control.now());
                let from = key_to_string(&peer);
                match self.control.owned.ask(&from, &checks, now) {
                    Ok(()) => emit(
                        &self.events,
                        CoreEvent::Call(CallEvent::ControlAsked { call_id, from }),
                    ),
                    Err(why) => {
                        self.send(
                            peer,
                            Frame::ControlAnswer {
                                call,
                                answer: ControlReply::No(why.for_them()),
                            },
                        );
                        emit(
                            &self.events,
                            CoreEvent::Call(CallEvent::ControlAskedAnyway { call_id, from, why }),
                        );
                    }
                }
            }
            Frame::ControlAnswer { call, answer }
                if call == c.id && self.control.asking == Some(peer) && c.sharer == Some(peer) =>
            {
                self.control.asking = None;
                let who = key_to_string(&peer);
                match answer {
                    ControlReply::No(why) => emit(
                        &self.events,
                        CoreEvent::Call(CallEvent::ControlRefused {
                            call_id,
                            from: who,
                            why,
                        }),
                    ),
                    ControlReply::Yes(ControlKey {
                        session,
                        key,
                        scope,
                    }) => {
                        let Some(conn) = self.sessions.get(&peer).map(|s| s.conn.clone()) else {
                            return;
                        };
                        self.control.driving = Some(Driving {
                            who: peer,
                            tx: open_control_stream(conn, session, key),
                            session,
                            key,
                        });
                        emit(
                            &self.events,
                            CoreEvent::Call(CallEvent::ControlOn {
                                call_id,
                                who,
                                driving: true,
                                scope,
                                until: None,
                            }),
                        );
                    }
                }
            }
            Frame::ControlStop { call } if call == c.id && c.peers.contains(&peer) => {
                if self.control.session.as_ref().is_some_and(|s| s.who == peer)
                    || self.control.owned.asked_by() == Some(key_to_string(&peer).as_str())
                {
                    self.control_owner_side_ends(Ended::Them, false);
                }
                if self.control.driving.as_ref().is_some_and(|d| d.who == peer) {
                    self.control_stop_driving(false);
                }
            }
            _ => {}
        }
    }

    /// We would like to control what the other side shows.
    pub(super) fn control_ask(&mut self) {
        let Some(c) = self.call.as_ref().filter(|c| c.group.is_none()) else {
            return;
        };
        let (call, Some(sharer)) = (c.id, c.sharer) else {
            return;
        };
        if self.control.driving.is_none() {
            self.control.asking = Some(sharer);
            self.send(sharer, Frame::ControlAsk { call });
        }
    }

    /// What the owner said. Only ever from this device's own screen.
    pub(super) fn control_answer(&mut self, yes: bool, scope: Scope) {
        let Some(c) = self.call.as_ref() else { return };
        let call = c.id;
        let call_id = id_from_bytes(&call);
        let Some(from) = self.control.owned.asked_by().map(str::to_owned) else {
            return;
        };
        let Some(peer) = crate::identity::parse_key(&from) else {
            return;
        };
        let id: [u8; 16] = crate::vault::random();
        let now = self.control.now();
        let Some(grant) = self.control.owned.answer(yes, scope, id, now) else {
            // Too soon: the question stays, as if nothing was pressed.
            if self.control.owned.asked_by().is_none() {
                self.send(
                    peer,
                    Frame::ControlAnswer {
                        call,
                        answer: ControlReply::No(crate::control::Refusal::SaidNo),
                    },
                );
            }
            return;
        };
        let key: [u8; 32] = crate::vault::random();
        self.control.session = Some(OwnedSession {
            id,
            key,
            who: peer,
            since: now_ms(),
        });
        self.control_gate(Some((id, peer)));
        self.send(
            peer,
            Frame::ControlAnswer {
                call,
                answer: ControlReply::Yes(ControlKey {
                    session: id,
                    key,
                    scope: grant.scope,
                }),
            },
        );
        emit(
            &self.events,
            CoreEvent::Call(CallEvent::ControlOn {
                call_id,
                who: from,
                driving: false,
                scope: grant.scope,
                until: Some(now_ms() + (grant.until - now)),
            }),
        );
    }

    /// We are done, whichever side we are on.
    pub(super) fn control_stop(&mut self) {
        self.control.asking = None;
        self.control_stop_driving(true);
        self.control_owner_side_ends(Ended::Owner, true);
    }

    /// The owner touched the mouse or a key.
    pub(super) fn control_owner_busy(&mut self) {
        let now = self.control.now();
        self.control.owned.owner_busy(now);
    }

    /// More time, by the owner's own hand.
    pub(super) fn control_extend(&mut self) {
        let now = self.control.now();
        let Some(until) = self.control.owned.extend(now) else {
            return;
        };
        let (Some(c), Some(session)) = (self.call.as_ref(), self.control.session.as_ref()) else {
            return;
        };
        emit(
            &self.events,
            CoreEvent::Call(CallEvent::ControlOn {
                call_id: id_from_bytes(&c.id),
                who: key_to_string(&session.who),
                driving: false,
                scope: self.control.owned.scope().unwrap_or(Scope::Window),
                until: Some(now_ms() + (until - now)),
            }),
        );
    }

    /// The app saw that the screen locked, or that what is shown went away.
    pub(super) fn control_not_safe(&mut self) {
        self.control_owner_side_ends(Ended::NotSafeAnyMore, true);
    }

    /// Puts this on a stream of its own, as it is. For tests.
    pub(super) fn control_raw(&mut self, right_session: bool, frames: Vec<RawFrame>) {
        let Some(driving) = &self.control.driving else {
            return;
        };
        let Some(conn) = self.sessions.get(&driving.who).map(|s| s.conn.clone()) else {
            return;
        };
        let session = if right_session {
            driving.session
        } else {
            [9; 16]
        };
        let key = driving.key;
        tokio::spawn(async move {
            let Ok(mut send) = conn.open_uni().await else {
                return;
            };
            let mut header = [0u8; crate::files::STREAM_HEADER];
            header[..4].copy_from_slice(CONTROL_STREAM_MAGIC);
            header[4..20].copy_from_slice(&session);
            let _ = send.write_all(&header).await;
            for frame in frames {
                let bytes = match frame {
                    RawFrame::Sealed(batch) => crate::control::seal_batch(&key, &batch),
                    RawFrame::WrongKey(batch) => crate::control::seal_batch(&[1; 32], &batch),
                    RawFrame::Bytes(bytes) => Some(bytes),
                };
                let Some(bytes) = bytes else { continue };
                let _ = send.write_all(&(bytes.len() as u32).to_be_bytes()).await;
                let _ = send.write_all(&bytes).await;
            }
            // Stays open a little: the other side decides when it is over.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        });
    }

    /// Input we want to send to whoever we control.
    pub(super) fn control_send(&mut self, events: Vec<Input>) {
        let Some(driving) = &self.control.driving else {
            return;
        };
        for part in events.chunks(MOST_IN_BATCH) {
            // A full queue means the other side is far behind: more of the same does not help.
            let _ = driving.tx.try_send(part.to_vec());
        }
    }

    pub(super) fn on_control_batch(&mut self, peer: PublicKey, session: [u8; 16], sealed: &[u8]) {
        let Some(mine) = self.control.session.as_ref() else {
            return;
        };
        if mine.who != peer || mine.id != session {
            return;
        }
        let Some(batch) = crate::control::open_batch(&mine.key, sealed) else {
            return self.control_strike();
        };
        let who = key_to_string(&peer);
        let now = self.control.now();
        match self.control.owned.input(&who, &session, batch, now) {
            Verdict::Use(events) if !events.is_empty() => {
                let sink = self
                    .control_sink
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clone();
                if let Some(sink) = sink {
                    let _ = sink.try_send(events);
                }
            }
            Verdict::Ended(why) => self.control_owner_side_ends(why, true),
            Verdict::Use(_) | Verdict::Waiting | Verdict::Dropped => {}
        }
    }

    pub(super) fn control_strike(&mut self) {
        if let Some(why) = self.control.owned.strike() {
            self.control_owner_side_ends(why, true);
        }
    }

    /// Time passing.
    pub(super) fn control_tick(&mut self) {
        let was_on = self.control.owned.is_on();
        let now = self.control.now();
        if let Some(why) = self.control.owned.tick(now) {
            self.control_owner_side_ends(why, true);
        } else if was_on && !self.control.owned.is_on() {
            self.control_owner_side_ends(Ended::TimeUp, true);
        }
    }

    pub(super) fn control_is_on(&self) -> bool {
        self.control.owned.is_on()
    }

    /// The sharing stopped, or the call ended.
    pub(super) fn control_call_or_sharing_over(&mut self) {
        self.control.asking = None;
        self.control_stop_driving(false);
        self.control_owner_side_ends(Ended::CallOrSharing, true);
    }

    /// What we were controlling is not shown any more.
    pub(super) fn control_stop_driving_if(&mut self, peer: PublicKey) {
        if self.control.driving.as_ref().is_some_and(|d| d.who == peer) {
            self.control_stop_driving(false);
        }
    }

    fn control_stop_driving(&mut self, tell: bool) {
        // Dropping the sender ends the stream.
        let Some(driving) = self.control.driving.take() else {
            return;
        };
        let who = key_to_string(&driving.who);
        let Some(c) = self.call.as_ref() else { return };
        let call = c.id;
        if tell {
            self.send(driving.who, Frame::ControlStop { call });
        }
        emit(
            &self.events,
            CoreEvent::Call(CallEvent::ControlOff {
                call_id: id_from_bytes(&call),
                who,
                driving: true,
                why: if tell { Ended::Owner } else { Ended::Them },
            }),
        );
    }

    /// Over on the side being controlled, or the question is off. After this nothing gets through.
    fn control_owner_side_ends(&mut self, why: Ended, tell: bool) {
        self.control_gate(None);
        let session = self.control.session.take();
        let asked = self.control.owned.asked_by().map(str::to_owned);
        self.control.owned.end();
        let who = session.as_ref().map(|s| key_to_string(&s.who)).or(asked);
        let Some(who) = who else { return };
        if let Some(session) = &session {
            self.db.add_control_log(&crate::control::LogEntry {
                who: who.clone(),
                whole_screen: self.control_scope_was_whole,
                from: session.since,
                to: now_ms(),
                why,
            });
        }
        let Some(c) = self.call.as_ref() else {
            return;
        };
        let call = c.id;
        if tell && let Some(peer) = crate::identity::parse_key(&who) {
            self.send(peer, Frame::ControlStop { call });
        }
        emit(
            &self.events,
            CoreEvent::Call(CallEvent::ControlOff {
                call_id: id_from_bytes(&call),
                who,
                driving: false,
                why,
            }),
        );
    }
}

/// The side in control: batches go out one after the other, each with the next number, and a sign of
/// life when there is nothing else. The stream ends when the sender is dropped.
fn open_control_stream(
    conn: Connection,
    session: [u8; 16],
    key: [u8; 32],
) -> mpsc::Sender<Vec<Input>> {
    let (tx, mut rx) = mpsc::channel::<Vec<Input>>(64);
    tokio::spawn(async move {
        let Ok(mut send) = conn.open_uni().await else {
            return;
        };
        let mut header = [0u8; crate::files::STREAM_HEADER];
        header[..4].copy_from_slice(CONTROL_STREAM_MAGIC);
        header[4..20].copy_from_slice(&session);
        if send.write_all(&header).await.is_err() {
            return;
        }
        let mut number = 0u64;
        let mut beat = tokio::time::interval(STILL_HERE_EVERY);
        loop {
            let events = tokio::select! {
                events = rx.recv() => match events {
                    Some(events) => events,
                    None => break,
                },
                _ = beat.tick() => vec![Input::StillHere],
            };
            number += 1;
            let batch = crate::control::Batch { number, events };
            let Some(sealed) = crate::control::seal_batch(&key, &batch) else {
                continue;
            };
            let length = (sealed.len() as u32).to_be_bytes();
            if send.write_all(&length).await.is_err() || send.write_all(&sealed).await.is_err() {
                return;
            }
        }
        let _ = send.finish();
    });
    tx
}

/// The side being controlled. A stream that does not belong to the session of this person is closed
/// before anything is read from it.
pub(super) async fn receive_control(
    mut recv: RecvStream,
    header: [u8; crate::files::STREAM_HEADER],
    peer: PublicKey,
    media_in: &MediaIn,
    tx: tokio::sync::mpsc::UnboundedSender<Internal>,
) {
    let mut session = [0u8; 16];
    session.copy_from_slice(&header[4..20]);
    let expected = *media_in.control.lock().unwrap_or_else(|p| p.into_inner());
    if expected != Some((session, peer)) {
        let _ = recv.stop(1u32.into());
        return;
    }
    loop {
        let mut length = [0u8; 4];
        if recv.read_exact(&mut length).await.is_err() {
            return;
        }
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MOST_SEALED {
            let _ = tx.send(Internal::ControlStrike);
            let _ = recv.stop(1u32.into());
            return;
        }
        let mut sealed = vec![0u8; length];
        if recv.read_exact(&mut sealed).await.is_err() {
            return;
        }
        if tx
            .send(Internal::ControlBatch {
                peer,
                session,
                sealed,
            })
            .is_err()
        {
            return;
        }
    }
}
