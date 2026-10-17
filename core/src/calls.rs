//! Voice and video calls: events, and the two media paths.
//!
//! Signalling (`CallInvite`, `CallAnswer`, `CallEnd`, `CallVideo`) rides the
//! normal control stream. Media never touches the database and is never
//! retried:
//!
//! * **audio**: one QUIC datagram per 20 ms Opus packet (unreliable, no
//!   head-of-line blocking);
//! * **video**: one short-lived uni stream per encoded frame, so a lost
//!   packet delays only its own frame, and a slow link drops frames instead
//!   of building a queue.
//!
//! The encoding itself (WebCodecs: Opus + VP8) happens in the UI layer; the
//! core only moves opaque packets between the UI and the peer.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use iroh::PublicKey;
use iroh::endpoint::{Connection, RecvStream};
use tokio::sync::mpsc;

use crate::identity::key_to_string;
use crate::protocol::{AUDIO_DATAGRAM_MAGIC, MAX_VIDEO_FRAME, VIDEO_STREAM_MAGIC, id_from_bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Audio,
    Video,
}

/// One packet of call media, in either direction.
#[derive(Debug, Clone)]
pub struct CallMedia {
    pub call_id: String,
    /// Who sent it (their key). Several people can send in a group call.
    pub from: String,
    pub kind: MediaKind,
    /// Capture time in microseconds, wrapping (u32 is plenty for jitter
    /// and ordering inside one call).
    pub ts: u32,
    pub key: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallEndReason {
    /// Either side hung up an answered call.
    Hangup,
    /// The caller gave up while it was ringing (seen by the callee as missed).
    Cancelled,
    Declined,
    Busy,
    /// Nobody answered in time.
    Missed,
    /// The connection to the other side dropped.
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEvent {
    /// `contact_id` is the conversation (a person, or a group); `from` is
    /// who is calling.
    Incoming {
        call_id: String,
        contact_id: String,
        from: String,
        video: bool,
    },
    /// We are in the call.
    Connected { call_id: String, contact_id: String },
    /// Who else is in the call right now (everyone but us).
    Participants { call_id: String, peers: Vec<String> },
    PeerVideo {
        call_id: String,
        from: String,
        on: bool,
    },
    /// A group call is going on that we could join (or it has ended).
    GroupCall {
        group_id: String,
        active: bool,
        video: bool,
    },
    Ended {
        call_id: String,
        contact_id: String,
        reason: CallEndReason,
        duration_ms: u64,
    },
}

/// The call, and who may send media for it, right now.
pub(crate) type ActiveCall = Arc<Mutex<Option<([u8; 16], HashSet<PublicKey>)>>>;

/// Where incoming media goes.
#[derive(Clone)]
pub(crate) struct MediaIn {
    pub active: ActiveCall,
    pub tx: mpsc::Sender<CallMedia>,
}

impl MediaIn {
    fn accepts(&self, call: &[u8; 16], peer: PublicKey) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|(id, members)| id == call && members.contains(&peer))
    }

    fn deliver(&self, media: CallMedia) {
        // A full queue means the UI is behind: dropping is the right thing.
        let _ = self.tx.try_send(media);
    }
}

/// Reads audio datagrams for as long as the connection lives.
pub(crate) fn spawn_datagram_reader(conn: Connection, peer: PublicKey, media: MediaIn) {
    tokio::spawn(async move {
        while let Ok(d) = conn.read_datagram().await {
            if d.len() < 1 + 16 + 4 || d[0] != AUDIO_DATAGRAM_MAGIC {
                continue;
            }
            let call: [u8; 16] = d[1..17].try_into().expect("16 bytes");
            if !media.accepts(&call, peer) {
                continue;
            }
            let ts = u32::from_be_bytes(d[17..21].try_into().expect("4 bytes"));
            media.deliver(CallMedia {
                call_id: id_from_bytes(&call),
                from: key_to_string(&peer),
                kind: MediaKind::Audio,
                ts,
                key: false,
                data: d[21..].to_vec(),
            });
        }
    });
}

/// Handles a video stream whose 28-byte header was already read.
pub(crate) async fn receive_video(
    mut recv: RecvStream,
    header: [u8; 28],
    peer: PublicKey,
    media: &MediaIn,
) {
    debug_assert_eq!(&header[..4], VIDEO_STREAM_MAGIC);
    let call: [u8; 16] = header[4..20].try_into().expect("16 bytes");
    if !media.accepts(&call, peer) {
        let _ = recv.stop(1u32.into());
        return;
    }
    let ts = u32::from_be_bytes(header[20..24].try_into().expect("4 bytes"));
    let flags = u32::from_be_bytes(header[24..28].try_into().expect("4 bytes"));
    let read =
        tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(MAX_VIDEO_FRAME)).await;
    let Ok(Ok(data)) = read else {
        let _ = recv.stop(1u32.into());
        return;
    };
    media.deliver(CallMedia {
        call_id: id_from_bytes(&call),
        from: key_to_string(&peer),
        kind: MediaKind::Video,
        ts,
        key: flags & 1 != 0,
        data,
    });
}

pub(crate) fn send_audio(conn: &Connection, call: [u8; 16], ts: u32, data: &[u8]) {
    let mut buf = Vec::with_capacity(21 + data.len());
    buf.push(AUDIO_DATAGRAM_MAGIC);
    buf.extend_from_slice(&call);
    buf.extend_from_slice(&ts.to_be_bytes());
    buf.extend_from_slice(data);
    if conn.max_datagram_size().is_some_and(|m| buf.len() <= m) {
        let _ = conn.send_datagram(Bytes::from(buf));
    }
}

/// Frames in flight before delta frames are dropped instead of queued.
const MAX_VIDEO_IN_FLIGHT: usize = 4;

pub(crate) fn send_video(
    conn: Connection,
    call: [u8; 16],
    ts: u32,
    key: bool,
    data: Vec<u8>,
    inflight: Arc<AtomicUsize>,
) {
    if data.len() > MAX_VIDEO_FRAME {
        return;
    }
    if inflight.load(Ordering::Relaxed) >= MAX_VIDEO_IN_FLIGHT && !key {
        return;
    }
    inflight.fetch_add(1, Ordering::Relaxed);
    tokio::spawn(async move {
        let result = async {
            let mut send = conn.open_uni().await?;
            let mut buf = Vec::with_capacity(28 + data.len());
            buf.extend_from_slice(VIDEO_STREAM_MAGIC);
            buf.extend_from_slice(&call);
            buf.extend_from_slice(&ts.to_be_bytes());
            buf.extend_from_slice(&u32::from(key).to_be_bytes());
            buf.extend_from_slice(&data);
            send.write_all(&buf).await?;
            send.finish()?;
            // Hold the slot until the peer has the whole frame (or gave up).
            let _ = tokio::time::timeout(Duration::from_secs(3), send.stopped()).await;
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = result {
            tracing::debug!("video frame not sent: {e:#}");
        }
        inflight.fetch_sub(1, Ordering::Relaxed);
    });
}
