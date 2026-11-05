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
use crate::protocol::{
    AUDIO_DATAGRAM_MAGIC, MAX_VIDEO_FRAME, SCREEN_STREAM_MAGIC, VIDEO_STREAM_MAGIC, id_from_bytes,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Audio,
    Video,
    /// A shown screen.
    Screen,
}

/// How a shown screen is encoded. The sender knows what it can do, every viewer can read these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenCodec {
    #[default]
    Vp8,
    Vp9,
    H264,
}

impl ScreenCodec {
    fn bits(self) -> u8 {
        match self {
            Self::Vp8 => 0,
            Self::Vp9 => 1,
            Self::H264 => 2,
        }
    }

    pub fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            1 => Self::Vp9,
            2 => Self::H264,
            _ => Self::Vp8,
        }
    }

    /// As the page's decoder asks for it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Vp8 => "vp8",
            Self::Vp9 => "vp09.00.10.08",
            Self::H264 => "avc1.42E01F",
        }
    }
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
    /// For a shown screen: which frame this is, so a missing one shows.
    pub number: u32,
    pub codec: ScreenCodec,
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
    /// Someone started or stopped showing their screen.
    PeerScreen {
        call_id: String,
        from: String,
        on: bool,
        width: u32,
        height: u32,
        /// They could be controlled, if they agree.
        can_control: bool,
    },
    /// In a room: someone raised their hand, or took it down.
    PeerHand {
        call_id: String,
        from: String,
        up: bool,
    },
    /// This room has as many in it as it can take.
    RoomFull { group_id: String },
    /// Someone asks to control this PC, and all the rules are met: time to ask the owner.
    ControlAsked { call_id: String, from: String },
    /// Someone asked, and the rules say no. The owner hears why.
    ControlAskedAnyway {
        call_id: String,
        from: String,
        why: crate::control::Refusal,
    },
    /// We asked to control someone's screen, and they will not have it.
    ControlRefused {
        call_id: String,
        from: String,
        why: crate::control::Refusal,
    },
    /// Someone is in control. `driving`: that is us. `until`: when it ends, for the owner.
    ControlOn {
        call_id: String,
        who: String,
        driving: bool,
        scope: crate::control::Scope,
        until: Option<crate::model::Millis>,
    },
    /// It is over, or the question is. Whatever anyone held down is to be let go now.
    ControlOff {
        call_id: String,
        who: String,
        driving: bool,
        why: crate::control::Ended,
    },
    /// Someone watching our screen needs a whole picture.
    ScreenKeyWanted { call_id: String },
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

/// How many can be in a room. Everyone connects to everyone, so this is what the tests prove works.
pub const ROOM_MOST: usize = 8;

/// A room has one call, whoever came in first: two people coming in at once end up in the same one.
pub fn room_call_id(group: &str) -> [u8; 16] {
    let hash = blake3::derive_key("whatsmine room call v1", group.as_bytes());
    let mut id = [0u8; 16];
    id.copy_from_slice(&hash[..16]);
    id
}

/// Where incoming media goes. The receiving end can be replaced at any time
/// (the screen that plays the media comes and goes; the call does not).
pub(crate) type MediaSink = Arc<Mutex<Option<mpsc::Sender<CallMedia>>>>;

#[derive(Clone)]
pub(crate) struct MediaIn {
    pub active: ActiveCall,
    pub sink: MediaSink,
    /// The one session of input we take right now, and from whom.
    pub control: ControlGate,
}

pub(crate) type ControlGate = Arc<Mutex<Option<([u8; 16], PublicKey)>>>;
/// Where input that passed every rule goes.
pub(crate) type ControlSink = Arc<Mutex<Option<mpsc::Sender<Vec<crate::control::Input>>>>>;

/// Where a call stands, for a screen that was not there when it began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase {
    /// We are calling and nobody has answered yet.
    RingingOut,
    /// Somebody is calling us.
    RingingIn,
    /// We are in the call.
    Active,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSnapshot {
    pub call_id: String,
    /// The conversation: a person's key, or a group id.
    pub contact_id: String,
    /// Who rang us (our own key if we are the caller).
    pub from: String,
    pub video: bool,
    pub phase: CallPhase,
    /// When we joined (milliseconds since the epoch), once active.
    pub connected_since: Option<i64>,
    /// Everyone else who is in the call.
    pub peers: Vec<String>,
    /// Who is showing their screen.
    pub sharer: Option<String>,
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
        // No listener (the screen is closed) drops it too.
        if let Some(tx) = self.sink.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            let _ = tx.try_send(media);
        }
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
                number: 0,
                codec: ScreenCodec::default(),
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
        number: 0,
        codec: ScreenCodec::default(),
    });
}

/// How many frames of a screen may wait for one viewer. What does not fit is skipped: the viewer
/// sees the number jump and asks for a whole picture, which brings it up to date.
const SCREEN_WAITING: usize = 8;

/// What a frame of a screen looks like on its stream.
pub(crate) fn screen_frame(
    number: u32,
    ts: u32,
    key: bool,
    codec: ScreenCodec,
    data: &[u8],
) -> Bytes {
    let mut out = Vec::with_capacity(13 + data.len());
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&number.to_be_bytes());
    out.extend_from_slice(&ts.to_be_bytes());
    out.push(u8::from(key) | (codec.bits() << 1));
    out.extend_from_slice(data);
    Bytes::from(out)
}

/// Opens the stream of a shown screen to one viewer. It ends when the sender is dropped.
pub(crate) fn open_screen_stream(conn: Connection, call: [u8; 16]) -> mpsc::Sender<Bytes> {
    let (tx, mut rx) = mpsc::channel::<Bytes>(SCREEN_WAITING);
    tokio::spawn(async move {
        let result = async {
            let mut send = conn.open_uni().await?;
            let mut header = Vec::with_capacity(28);
            header.extend_from_slice(SCREEN_STREAM_MAGIC);
            header.extend_from_slice(&call);
            header.extend_from_slice(&[0; 8]);
            send.write_all(&header).await?;
            while let Some(frame) = rx.recv().await {
                send.write_all(&frame).await?;
            }
            send.finish()?;
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = result {
            tracing::debug!("the stream of a screen ended: {e:#}");
        }
    });
    tx
}

/// Reads a shown screen, frame by frame, for as long as this person is in this call.
pub(crate) async fn receive_screen(
    mut recv: RecvStream,
    header: [u8; 28],
    peer: PublicKey,
    media: &MediaIn,
) {
    debug_assert_eq!(&header[..4], SCREEN_STREAM_MAGIC);
    let call: [u8; 16] = header[4..20].try_into().expect("16 bytes");
    loop {
        if !media.accepts(&call, peer) {
            let _ = recv.stop(1u32.into());
            return;
        }
        let mut head = [0u8; 13];
        if recv.read_exact(&mut head).await.is_err() {
            return;
        }
        let len = u32::from_be_bytes(head[..4].try_into().expect("4 bytes")) as usize;
        if len > MAX_VIDEO_FRAME {
            let _ = recv.stop(1u32.into());
            return;
        }
        let mut data = vec![0u8; len];
        if recv.read_exact(&mut data).await.is_err() {
            return;
        }
        media.deliver(CallMedia {
            call_id: id_from_bytes(&call),
            from: key_to_string(&peer),
            kind: MediaKind::Screen,
            ts: u32::from_be_bytes(head[8..12].try_into().expect("4 bytes")),
            key: head[12] & 1 != 0,
            data,
            number: u32::from_be_bytes(head[4..8].try_into().expect("4 bytes")),
            codec: ScreenCodec::from_bits(head[12] >> 1),
        });
    }
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
