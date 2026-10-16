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
    },
    Receipt {
        kind: ReceiptKind,
        ids: Vec<[u8; 16]>,
    },
    Typing,
}

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

pub async fn read_frame(recv: &mut RecvStream) -> anyhow::Result<Frame> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len).await?;
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME, "frame too large: {len}");
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body).await?;
    decode(&body)
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
            },
            Frame::Receipt {
                kind: ReceiptKind::Read,
                ids: vec![[1; 16], [2; 16]],
            },
            Frame::Typing,
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
