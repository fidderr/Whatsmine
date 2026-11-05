//! The wire protocol between two WhatsMine devices.
//!
//! One QUIC bi-directional stream per connection carries length-prefixed
//! `postcard` frames: a 4-byte big-endian length, then the encoded [`Frame`].
//!
//! The first frame each way proves who is there ([`Frame::Auth`]). After that only
//! [`Frame::Sealed`] counts: everything else is sealed end to end inside it, so what the
//! transport encrypts is encrypted twice, and a message can also travel another way.

use iroh::endpoint::{RecvStream, SendStream};
use serde::{Deserialize, Serialize};

/// ALPN for this protocol. Bump the suffix on incompatible changes.
pub const ALPN: &[u8] = b"whatsmine/2";

pub const PROTO_VERSION: u16 = 2;

/// Upper bound for one frame, to stop a peer from making us allocate wildly.
pub const MAX_FRAME: usize = 256 * 1024;

/// Upper bound for one message body, in bytes.
pub const MAX_TEXT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Frame {
    /// The very first frame each way: who is here, proven with the person key over this very
    /// connection, and what it takes to start an encrypted session with them.
    Auth {
        person: [u8; 32],
        curve: [u8; 32],
        one_time: [u8; 32],
        sig: Vec<u8>,
    },
    /// Everything after that, end to end. Inside is one encoded [`Frame`].
    Sealed {
        kind: u8,
        bytes: Vec<u8>,
    },
    /// "I cannot read what you sealed": start the session over, and send it again.
    NeedSession,
    /// The first frame inside the seal, in both directions.
    Hello {
        version: u16,
        name: String,
        /// Whether the sender agrees to have their last seen shown.
        share_last_seen: bool,
    },
    /// "I accept you as a contact", and how to find me. Sent on every connection while accepted,
    /// so a peer that missed it, or an earlier location of mine, catches up.
    Accept {
        name: String,
        location: [u8; 32],
        /// Their public link, for passing them on to someone.
        card: String,
    },
    Msg {
        id: [u8; 16],
        text: String,
        sent_at: i64,
        /// Set for a group message: the group's id.
        group: Option<String>,
        /// Replies, polls and the rest, as `MessageExtra::to_wire`. Nothing for a plain text.
        extra: Vec<u8>,
        /// In a group: the writer's signature, so anyone in it can pass this on.
        sig: Vec<u8>,
    },
    /// "These are the group's last messages I have."
    GroupHave {
        group: String,
        ids: Vec<[u8; 16]>,
    },
    /// A message of someone else in the group, passed on with their signature.
    GroupRelay {
        group: String,
        author: [u8; 32],
        id: [u8; 16],
        text: String,
        sent_at: i64,
        extra: Vec<u8>,
        sig: Vec<u8>,
    },
    Receipt {
        kind: ReceiptKind,
        ids: Vec<[u8; 16]>,
    },
    Typing,
    /// Our profile picture (JPEG, at most [`MAX_AVATAR`] bytes), or `None`
    /// to remove it. Sent on every session start and when it changes.
    Profile {
        avatar: Option<Vec<u8>>,
    },
    /// A file message. Re-sent until delivered, like `Msg`. The bytes follow
    /// on a separate stream once the receiver asks with `FileWant`.
    FileOffer {
        id: [u8; 16],
        sent_at: i64,
        name: String,
        mime: String,
        size: u64,
        sha256: [u8; 32],
        caption: String,
        duration_ms: Option<u64>,
        group: Option<String>,
        extra: Vec<u8>,
    },
    /// "Send me this file, starting at byte `offset`" (resume support).
    FileWant {
        id: [u8; 16],
        offset: u64,
    },
    /// "I won't take this file" (over my size limit).
    FileDecline {
        id: [u8; 16],
    },
    /// "I'm calling you." One call at a time per device.
    CallInvite {
        call: [u8; 16],
        video: bool,
        /// Set for a group call: the group's id.
        group: Option<String>,
    },
    /// The callee's answer to a [`Frame::CallInvite`].
    CallAnswer {
        call: [u8; 16],
        result: CallResult,
    },
    /// Hang up (or cancel a call that is still ringing).
    CallEnd {
        call: [u8; 16],
    },
    /// The sender switched their camera on or off during a call.
    CallVideo {
        call: [u8; 16],
        on: bool,
    },
    /// The state of a group, signed by its admin so anyone can pass it on.
    GroupUpdate {
        state: GroupState,
        /// Ed25519 signature by `state.admin` over `state.signing_bytes()`.
        sig: Vec<u8>,
        /// Display names (key, name) for members the receiver may not know.
        names: Vec<(String, String)>,
        /// What the members find each other with. A new one when someone is removed.
        secret: [u8; 32],
    },
    /// "I am leaving this group."
    GroupLeave {
        group: String,
    },
    /// Something happened to a message, or to the chat. Sent until confirmed, in the order it
    /// happened. `change` is a `MessageChange` as text, so a kind from a newer version can be
    /// confirmed without being understood.
    Change {
        id: [u8; 16],
        target: [u8; 16],
        change: Vec<u8>,
        group: Option<String>,
    },
    ChangeAck {
        id: [u8; 16],
    },
    /// Someone started or stopped showing their screen in this call.
    CallScreen {
        call: [u8; 16],
        on: bool,
        width: u32,
        height: u32,
        /// This device could be controlled, if its owner agrees. Says nothing about whether they
        /// turned that on: that is for them to know.
        can_control: bool,
    },
    /// "Send a whole picture": someone just started watching, or lost their place.
    CallKeyRequest {
        call: [u8; 16],
    },
    /// One change, to our own other device. Sent until confirmed, in order.
    Sibling {
        seq: i64,
        event: Vec<u8>,
    },
    SiblingAck {
        seq: i64,
    },
    /// May I control the screen you are showing?
    ControlAsk {
        call: [u8; 16],
    },
    ControlAnswer {
        call: [u8; 16],
        answer: ControlReply,
    },
    /// Whoever sends this is done with it.
    ControlStop {
        call: [u8; 16],
    },
    /// Something we post, for a day. Sent until they say they have it.
    StatusPost {
        id: [u8; 16],
        at: crate::model::Millis,
        lasts: crate::model::Millis,
        text: String,
        backdrop: u8,
        /// How many `StatusPhoto` follow.
        parts: u16,
    },
    StatusPhoto {
        id: [u8; 16],
        part: u16,
        bytes: Vec<u8>,
    },
    /// We have all of it. `gone`: and we heard it was taken back.
    StatusGot {
        id: [u8; 16],
        gone: bool,
    },
    /// We looked at it.
    StatusSeen {
        id: [u8; 16],
    },
    /// Taken back.
    StatusGone {
        id: [u8; 16],
    },
    /// In a room: a hand goes up, or down.
    CallHand {
        call: [u8; 16],
        up: bool,
    },
}

/// What the admin signs. A higher `version` replaces a lower one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupState {
    pub id: String,
    pub name: String,
    pub admin: String,
    pub version: u64,
    /// Keys of every member, the admin included, sorted.
    pub members: Vec<String>,
    /// The owner ended the group.
    pub closed: bool,
    pub rules: GroupRules,
}

/// Who does what in a group. Signed along with the rest.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GroupRules {
    /// Besides the owner.
    pub admins: Vec<String>,
    /// Who signed this version.
    pub signer: String,
    /// Only admins can send: for announcements.
    pub only_admins_send: bool,
    pub only_admins_add: bool,
    pub only_admins_edit: bool,
    /// How long someone waits between messages. 0 means no waiting.
    pub slow_ms: u32,
    pub about: String,
    /// A drop-in room: its call is just there for whoever comes in, and nobody rings.
    pub room: bool,
}

impl GroupState {
    /// The owner or an admin.
    pub fn is_manager(&self, who: &str) -> bool {
        self.admin == who || self.rules.admins.iter().any(|a| a == who)
    }
}

impl GroupState {
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut canon = self.clone();
        canon.members.sort();
        canon.rules.admins.sort();
        let mut out = b"whatsmine-group-v1".to_vec();
        out.extend(postcard::to_stdvec(&canon).unwrap_or_default());
        out
    }
}

/// Most members one group may have. Each message is sealed for each member, and members who are
/// around pass on what others missed.
pub const MAX_GROUP_MEMBERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallResult {
    Accepted,
    Declined,
    /// Already in another call.
    Busy,
}

/// First byte of a call-audio datagram: `0xA1 | call id (16) | ts (u32 BE) | opus`.
pub const AUDIO_DATAGRAM_MAGIC: u8 = 0xA1;

/// First bytes of a call-video uni stream, in the same 28-byte header shape
/// as file streams: `"WMV1" | call id (16) | ts (u32 BE) | flags (u32 BE)`,
/// then one encoded frame until the stream ends. Flag bit 0 = key frame.
pub const VIDEO_STREAM_MAGIC: &[u8; 4] = b"WMV1";

/// What the one being asked says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlReply {
    Yes(ControlKey),
    No(crate::control::Refusal),
}

/// What it takes to send input for one session. Made for this session, and it dies with it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlKey {
    pub session: [u8; 16],
    pub key: [u8; 32],
    pub scope: crate::control::Scope,
}

/// Frames can end up in logs. This one does not take its key along.
impl std::fmt::Debug for ControlKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlKey")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// First bytes of a shown screen's uni stream, in the same 28-byte header shape: `"WMS1"`, the call
/// id and 8 unused bytes. Then frame after frame: length (u32 BE), number (u32 BE), time (u32 BE),
/// flags (one byte: bit 0 a whole picture, bits 1 and 2 the codec), and the frame. A screen is text
/// more often than not: late is better than wrong, so it comes in order on one stream.
pub const SCREEN_STREAM_MAGIC: &[u8; 4] = b"WMS1";
/// Starts a stream of input for someone who was let in, see `control.rs`.
pub const CONTROL_STREAM_MAGIC: &[u8; 4] = b"WMI1";

/// Largest encoded video frame we accept.
pub const MAX_VIDEO_FRAME: usize = 1024 * 1024;

/// How long an unanswered call rings.
pub const RING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// Largest profile picture we send or accept.
pub const MAX_AVATAR: usize = 128 * 1024;

/// First bytes of every file-data stream, before the 16-byte id and the
/// 8-byte big-endian offset.
pub const FILE_STREAM_MAGIC: &[u8; 4] = b"WMF1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptKind {
    Delivered,
    Read,
}

pub fn encode(frame: &Frame) -> anyhow::Result<Vec<u8>> {
    let body = postcard::to_stdvec(frame)?;
    anyhow::ensure!(body.len() <= MAX_FRAME, "frame too large");
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn decode(body: &[u8]) -> anyhow::Result<Frame> {
    Ok(postcard::from_bytes(body)?)
}

pub async fn write_frame(send: &mut SendStream, frame: &Frame) -> anyhow::Result<()> {
    send.write_all(&encode(frame)?).await?;
    Ok(())
}

/// Reads one frame. The outer error means the stream is broken; the inner
/// one means the frame was unreadable (for example a newer peer sending a
/// kind we don't know), which callers should skip, not hang up on.
pub async fn read_frame(recv: &mut RecvStream) -> anyhow::Result<anyhow::Result<Frame>> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME, "frame too large: {len}");
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await?;
    Ok(decode(&body))
}

pub fn id_to_bytes(id: &str) -> Option<[u8; 16]> {
    uuid::Uuid::parse_str(id).ok().map(|u| *u.as_bytes())
}

pub fn id_from_bytes(b: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*b).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let frames = [
            Frame::Hello {
                version: PROTO_VERSION,
                name: "Roy 🦀".into(),
                share_last_seen: false,
            },
            Frame::Accept {
                name: String::new(),
                location: [7; 32],
                card: String::new(),
            },
            Frame::Msg {
                id: [7; 16],
                text: "hallo\nwereld".into(),
                sent_at: 1_792_000_000_000,
                group: Some("g:x".into()),
                extra: vec![1, 2],
                sig: vec![3],
            },
            Frame::GroupHave {
                group: "g:x".into(),
                ids: vec![[1; 16]],
            },
            Frame::GroupRelay {
                group: "g:x".into(),
                author: [5; 32],
                id: [1; 16],
                text: "hi".into(),
                sent_at: 5,
                extra: Vec::new(),
                sig: vec![6],
            },
            Frame::Change {
                id: [1; 16],
                target: [2; 16],
                change: b"Unsend".to_vec(),
                group: None,
            },
            Frame::ChangeAck { id: [1; 16] },
            Frame::Sibling {
                seq: 4,
                event: vec![1],
            },
            Frame::SiblingAck { seq: 4 },
            Frame::Receipt {
                kind: ReceiptKind::Read,
                ids: vec![[1; 16], [2; 16]],
            },
            Frame::Typing,
            Frame::Profile {
                avatar: Some(vec![0xff, 0xd8, 1, 2]),
            },
            Frame::FileOffer {
                id: [3; 16],
                sent_at: 5,
                name: "foto.jpg".into(),
                mime: "image/jpeg".into(),
                size: 123_456_789,
                sha256: [9; 32],
                caption: "kijk".into(),
                duration_ms: Some(4200),
                group: None,
                extra: Vec::new(),
            },
            Frame::FileWant {
                id: [3; 16],
                offset: 1 << 33,
            },
            Frame::FileDecline { id: [3; 16] },
            Frame::CallInvite {
                call: [4; 16],
                video: true,
                group: None,
            },
            Frame::GroupUpdate {
                state: GroupState {
                    id: "g:1".into(),
                    name: "Team".into(),
                    admin: "a".into(),
                    version: 3,
                    members: vec!["a".into(), "b".into()],
                    closed: false,
                    rules: GroupRules::default(),
                },
                sig: vec![1; 64],
                names: vec![("b".into(), "Bob".into())],
                secret: [9; 32],
            },
            Frame::GroupLeave {
                group: "g:1".into(),
            },
            Frame::CallAnswer {
                call: [4; 16],
                result: CallResult::Busy,
            },
            Frame::CallEnd { call: [4; 16] },
            Frame::CallVideo {
                call: [4; 16],
                on: false,
            },
        ];
        for f in frames {
            let bytes = encode(&f).unwrap();
            let len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
            assert_eq!(len, bytes.len() - 4);
            assert_eq!(decode(&bytes[4..]).unwrap(), f);
        }
    }

    #[test]
    fn garbage_does_not_decode_to_a_frame() {
        assert!(decode(&[0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn message_ids_convert() {
        let id = uuid::Uuid::now_v7().to_string();
        assert_eq!(id_from_bytes(&id_to_bytes(&id).unwrap()), id);
        assert!(id_to_bytes("not-a-uuid").is_none());
    }
}
