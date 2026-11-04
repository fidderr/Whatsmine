//! The wire protocol between two WhatsMine devices.
//!
//! One QUIC bi-directional stream per connection carries length-prefixed
//! `postcard` frames: a 4-byte big-endian length, then the encoded [`Frame`].
//! The transport (iroh/QUIC with TLS) already encrypts and authenticates
//! both ends, so frames carry no keys or signatures of their own.

use iroh::endpoint::{RecvStream, SendStream};
use serde::{Deserialize, Serialize};

/// ALPN for this protocol. Bump the suffix on incompatible changes.
pub const ALPN: &[u8] = b"whatsmine/1";

pub const PROTO_VERSION: u16 = 1;

/// Upper bound for one frame, to stop a peer from making us allocate wildly.
pub const MAX_FRAME: usize = 256 * 1024;

/// Upper bound for one message body, in bytes.
pub const MAX_TEXT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Frame {
    /// Always the first frame on a connection, in both directions.
    Hello {
        version: u16,
        name: String,
        /// Whether the sender agrees to have their last seen shown.
        share_last_seen: bool,
    },
    /// "Please add me as a contact."
    Request {
        name: String,
    },
    /// "I accept you as a contact." Sent on every connection while accepted,
    /// so a peer that missed it catches up.
    Accept {
        name: String,
    },
    Msg {
        id: [u8; 16],
        text: String,
        sent_at: i64,
        /// Set for a group message: the group's id.
        group: Option<String>,
    },
    Receipt {
        kind: ReceiptKind,
        ids: Vec<[u8; 16]>,
    },
    Typing,
    /// Not sent any more: QUIC has its own heartbeat. A peer that stays silent longer than a time-out was
    /// treated as gone, so "online" stops showing within seconds when an app
    /// is killed without closing its connection.
    Ping,
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
    },
    /// "I am leaving this group."
    GroupLeave {
        group: String,
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
    /// The admin ended the group.
    pub closed: bool,
}

impl GroupState {
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut canon = self.clone();
        canon.members.sort();
        let mut out = b"whatsmine-group-v1".to_vec();
        out.extend(postcard::to_stdvec(&canon).unwrap_or_default());
        out
    }
}

/// Most members one group may have (everyone connects to everyone).
pub const MAX_GROUP_MEMBERS: usize = 16;

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
            Frame::Request { name: "x".into() },
            Frame::Accept {
                name: String::new(),
            },
            Frame::Msg {
                id: [7; 16],
                text: "hallo\nwereld".into(),
                sent_at: 1_792_000_000_000,
                group: Some("g:x".into()),
            },
            Frame::Receipt {
                kind: ReceiptKind::Read,
                ids: vec![[1; 16], [2; 16]],
            },
            Frame::Typing,
            Frame::Ping,
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
                },
                sig: vec![1; 64],
                names: vec![("b".into(), "Bob".into())],
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
