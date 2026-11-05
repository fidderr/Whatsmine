//! Status posts on the wire: see `crate::status`. They go out when a connection is there anyway, and
//! keep going until the other side says it has them or the day is over.

use iroh::PublicKey;

use super::{Engine, emit};
use crate::identity::key_to_string;
use crate::model::{ContactState, CoreEvent, now_ms};
use crate::protocol::{Frame, id_from_bytes, id_to_bytes};
use crate::status::{self, Backdrop, MOST_FROM_ONE, MOST_PARTS, PHOTO_PART, Post};

impl Engine {
    /// Something changed in what we owe: whoever is connected gets it now, and we look around for the
    /// others, as the app is in use.
    pub(super) fn status_out(&mut self) {
        self.timers_dirty = true;
        let peers: Vec<PublicKey> = self.sessions.keys().copied().collect();
        for peer in peers {
            if peer != self.me {
                self.flush_status(peer);
            }
        }
        self.look_around_all();
    }

    /// Whatever we owe this person: posts, that one is gone, that we looked.
    pub(super) fn flush_status(&mut self, peer: PublicKey) {
        let id = key_to_string(&peer);
        if self.db.contact(&id).map(|c| c.state) != Some(ContactState::Accepted) {
            return;
        }
        for (post_id, gone) in self.db.status_owed(&id) {
            let Some(bytes) = id_to_bytes(&post_id) else {
                continue;
            };
            if gone {
                self.send(peer, Frame::StatusGone { id: bytes });
                continue;
            }
            let Some(post) = self.db.status_post(&post_id) else {
                continue;
            };
            self.send(
                peer,
                Frame::StatusPost {
                    id: bytes,
                    at: post.at,
                    lasts: post.until - post.at,
                    text: post.text,
                    backdrop: post.backdrop.byte(),
                    parts: post.parts,
                },
            );
            if post.parts > 0
                && let Some(photo) = self.db.status_photo_parts(&post_id)
            {
                for (part, bytes_of_part) in photo.into_iter().enumerate() {
                    self.send(
                        peer,
                        Frame::StatusPhoto {
                            id: bytes,
                            part: part as u16,
                            bytes: bytes_of_part,
                        },
                    );
                }
            }
        }
        if self.settings().read_receipts {
            for post_id in self.db.status_seen_to_tell(&id) {
                if let Some(bytes) = id_to_bytes(&post_id)
                    && self.send(peer, Frame::StatusSeen { id: bytes })
                {
                    self.db.status_seen_told(&post_id);
                }
            }
        }
    }

    pub(super) fn on_status_frame(&mut self, peer: PublicKey, frame: Frame) {
        let who = key_to_string(&peer);
        // Only from someone both sides agreed to know.
        if self.db.contact(&who).map(|c| c.state) != Some(ContactState::Accepted) {
            return;
        }
        match frame {
            Frame::StatusPost {
                id,
                at,
                lasts,
                text,
                backdrop,
                parts,
            } => {
                let post_id = id_from_bytes(&id);
                let now = now_ms();
                let Some(until) = status::ends_at(now, at, lasts).filter(|_| parts <= MOST_PARTS)
                else {
                    // Over already, or more than anyone can send: no reason to hear it again.
                    self.send(peer, Frame::StatusGot { id, gone: false });
                    return;
                };
                let new = self.db.add_status(&Post {
                    id: post_id.clone(),
                    author: who.clone(),
                    text: status::clean_text(&text),
                    backdrop: Backdrop::from_byte(backdrop),
                    at: at.min(now),
                    until,
                    parts,
                    seen: false,
                    seen_told: false,
                });
                if new {
                    self.db.status_keep_newest(&who, MOST_FROM_ONE);
                    self.timers_dirty = true;
                    emit(&self.events, CoreEvent::StatusChanged);
                }
                if parts == 0 || self.db.status_photo_parts(&post_id).is_some() {
                    self.send(peer, Frame::StatusGot { id, gone: false });
                }
            }
            Frame::StatusPhoto { id, part, bytes } => {
                let post_id = id_from_bytes(&id);
                let Some(post) = self.db.status_post(&post_id) else {
                    return;
                };
                if post.author != who || part >= post.parts || bytes.len() > PHOTO_PART {
                    return;
                }
                self.db.set_status_photo_part(&post_id, part, &bytes);
                if self.db.status_photo_parts(&post_id).is_some() {
                    self.send(peer, Frame::StatusGot { id, gone: false });
                    emit(&self.events, CoreEvent::StatusChanged);
                }
            }
            Frame::StatusGot { id, gone } => {
                self.db.status_got(&id_from_bytes(&id), &who, gone);
            }
            Frame::StatusSeen { id } => {
                // Only one of ours, and only someone it went to.
                let post_id = id_from_bytes(&id);
                if self.db.status_went_to(&post_id, &who) {
                    self.db.add_status_seen_by(&post_id, &who);
                    emit(&self.events, CoreEvent::StatusChanged);
                }
            }
            Frame::StatusGone { id } => {
                let post_id = id_from_bytes(&id);
                if self
                    .db
                    .status_post(&post_id)
                    .is_some_and(|post| post.author == who)
                {
                    self.db.delete_status(&post_id);
                    emit(&self.events, CoreEvent::StatusChanged);
                }
                self.send(peer, Frame::StatusGot { id, gone: true });
            }
            _ => {}
        }
    }
}
