//! Calls between two real cores over loopback QUIC: ring, answer, audio and
//! video flowing both ways, hang up, decline, busy, and a lost connection.

use std::time::Duration;

use tokio::sync::broadcast::Receiver;
use whatsmine_core::calls::{CallEndReason, CallEvent, MediaKind};
use whatsmine_core::records::LocalRecords as MemoryLookup;
use whatsmine_core::{Core, CoreEvent, NetMode, Settings};

const WAIT: Duration = Duration::from_secs(40);

async fn call_event(
    rx: &mut Receiver<CoreEvent>,
    what: &str,
    mut pred: impl FnMut(&CallEvent) -> bool,
) -> CallEvent {
    let fut = async {
        loop {
            match rx.recv().await {
                Ok(CoreEvent::Call(ev)) if pred(&ev) => return ev,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("event channel closed while waiting for {what}: {e}"),
            }
        }
    };
    tokio::time::timeout(WAIT, fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn start(dir: &std::path::Path, lookup: &MemoryLookup, name: &str) -> Core {
    let core = Core::start(dir, NetMode::Local(lookup.clone())).unwrap();
    core.save_settings(&Settings {
        display_name: name.into(),
        onboarded: true,
        ..Settings::default()
    })
    .unwrap();
    core
}

async fn online_pair(
    lookup: &MemoryLookup,
) -> (
    Core,
    Core,
    String,
    String,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(da.path(), lookup, "Alice");
    let b = start(db.path(), lookup, "Bob");
    let (ka, kb) = (a.my_key(), b.my_key());
    let mut ra = a.subscribe();
    let mut rb = b.subscribe();
    a.add_contact(&b.invite_link(), None).unwrap();
    loop {
        if let Ok(CoreEvent::RequestReceived { .. }) = tokio::time::timeout(WAIT, rb.recv())
            .await
            .expect("request")
        {
            break;
        }
    }
    b.accept(&ka).unwrap();
    for _ in 0..400 {
        if a.is_online(&kb) && b.is_online(&ka) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(a.is_online(&kb) && b.is_online(&ka), "both online");
    let _ = &mut ra;
    (a, b, ka, kb, da, db)
}

#[tokio::test(flavor = "multi_thread")]
async fn call_lifecycle_with_media() {
    let lookup = MemoryLookup::default();
    let (a, b, ka, kb, _da, _db) = online_pair(&lookup).await;
    let (mut ra, mut rb) = (a.subscribe(), b.subscribe());
    let mut media_a = a.call_media();
    let mut media_b = b.call_media();

    // Ring -> answer.
    let call = a.call_invite(&kb, true).unwrap();
    let inc = call_event(&mut rb, "incoming", |e| {
        matches!(e, CallEvent::Incoming { .. })
    })
    .await;
    assert_eq!(
        inc,
        CallEvent::Incoming {
            call_id: call.clone(),
            contact_id: ka.clone(),
            from: ka.clone(),
            video: true
        }
    );
    b.call_answer(true);
    call_event(&mut ra, "caller connected", |e| {
        matches!(e, CallEvent::Connected { .. })
    })
    .await;
    call_event(&mut rb, "callee connected", |e| {
        matches!(e, CallEvent::Connected { .. })
    })
    .await;

    // Audio datagrams both ways, video frames both ways.
    for i in 0..10u32 {
        a.call_audio(i * 20_000, vec![i as u8; 80]);
        b.call_audio(i * 20_000, vec![0x80 | i as u8; 60]);
    }
    a.call_video(1000, true, vec![7; 20_000]);
    b.call_video(2000, false, vec![9; 5_000]);
    let (mut audio_at_b, mut video_at_b, mut audio_at_a, mut video_at_a) = (0, 0, 0, 0);
    let collect = async {
        while audio_at_b < 5 || audio_at_a < 5 || video_at_b < 1 || video_at_a < 1 {
            tokio::select! {
                Some(m) = media_b.recv() => match m.kind {
                    MediaKind::Screen => panic!("no screen here"),
                    MediaKind::Audio => { assert_eq!(m.call_id, call); assert_eq!(m.data.len(), 80); audio_at_b += 1 }
                    MediaKind::Video => { assert!(m.key); assert_eq!(m.ts, 1000); assert_eq!(m.data, vec![7; 20_000]); video_at_b += 1 }
                },
                Some(m) = media_a.recv() => match m.kind {
                    MediaKind::Screen => panic!("no screen here"),
                    MediaKind::Audio => { assert_eq!(m.data.len(), 60); audio_at_a += 1 }
                    MediaKind::Video => { assert!(!m.key); assert_eq!(m.data, vec![9; 5_000]); video_at_a += 1 }
                },
            }
        }
    };
    tokio::time::timeout(WAIT, collect)
        .await
        .expect("media arrives");

    // Camera toggle is announced.
    b.call_video_on(false);
    call_event(&mut ra, "peer video off", |e| {
        matches!(e, CallEvent::PeerVideo { on: false, .. })
    })
    .await;

    // Hang up from the callee.
    b.call_end();
    let ended = call_event(&mut ra, "caller sees end", |e| {
        matches!(e, CallEvent::Ended { .. })
    })
    .await;
    assert!(matches!(
        ended,
        CallEvent::Ended {
            reason: CallEndReason::Hangup,
            ..
        }
    ));
    call_event(&mut rb, "callee sees end", |e| {
        matches!(e, CallEvent::Ended { .. })
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_screen_arrives_in_order_and_a_viewer_can_ask_for_a_whole_picture() {
    use whatsmine_core::calls::ScreenCodec;

    let lookup = MemoryLookup::default();
    let (a, b, _ka, _kb, _da, _db) = online_pair(&lookup).await;
    let (mut ra, mut rb) = (a.subscribe(), b.subscribe());
    a.call_invite(&b.my_key(), false).unwrap();
    call_event(&mut rb, "ringing", |e| {
        matches!(e, CallEvent::Incoming { .. })
    })
    .await;
    b.call_answer(true);
    call_event(&mut ra, "connected", |e| {
        matches!(e, CallEvent::Connected { .. })
    })
    .await;
    let mut viewer = b.call_media();

    // Nothing goes out before it is switched on.
    a.call_screen_frame(1, true, ScreenCodec::Vp9, vec![1; 100]);

    a.call_screen_on(true, 1920, 1080, false);
    let shown = call_event(&mut rb, "the screen", |e| {
        matches!(e, CallEvent::PeerScreen { on: true, .. })
    })
    .await;
    assert!(matches!(
        shown,
        CallEvent::PeerScreen {
            width: 1920,
            height: 1080,
            ..
        }
    ));
    until("the sharer to be known", || {
        b.call_snapshot()
            .is_some_and(|c| c.sharer.as_deref() == Some(a.my_key().as_str()))
    })
    .await;

    // A frame of a screen can be big, and they come in order.
    let big = vec![7u8; 400_000];
    a.call_screen_frame(10, true, ScreenCodec::Vp9, big.clone());
    for n in 0u8..5 {
        a.call_screen_frame(11 + u32::from(n), false, ScreenCodec::H264, vec![n; 2_000]);
    }
    let mut got = Vec::new();
    let read = async {
        while got.len() < 6 {
            let m = viewer.recv().await.expect("the media");
            if m.kind == MediaKind::Screen {
                got.push(m);
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), read)
        .await
        .expect("the frames");
    assert_eq!(got[0].data, big);
    assert!(got[0].key);
    assert_eq!(got[0].codec, ScreenCodec::Vp9);
    assert_eq!(
        got[0].number, 0,
        "the one from before it was switched on does not count"
    );
    for (i, m) in got.iter().enumerate().skip(1) {
        assert_eq!(m.number, i as u32);
        assert!(!m.key);
        assert_eq!(m.codec, ScreenCodec::H264);
    }

    // Lost its place: asks for a whole picture.
    b.call_screen_key();
    call_event(&mut ra, "the question for a whole picture", |e| {
        matches!(e, CallEvent::ScreenKeyWanted { .. })
    })
    .await;

    // Stopped, and whoever watched knows.
    a.call_screen_on(false, 0, 0, false);
    call_event(&mut rb, "the end of the screen", |e| {
        matches!(e, CallEvent::PeerScreen { on: false, .. })
    })
    .await;
    until("nobody sharing", || {
        b.call_snapshot().is_some_and(|c| c.sharer.is_none())
    })
    .await;
    a.call_end();
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_outside_the_call_shows_a_screen() {
    use whatsmine_core::protocol::{Frame, id_to_bytes};

    let lookup = MemoryLookup::default();
    let (a, b, _ka, _kb, _da, _db) = online_pair(&lookup).await;
    let mut rb = b.subscribe();
    a.send_as_is(
        &b.my_key(),
        Frame::CallScreen {
            call: id_to_bytes(&uuid::Uuid::now_v7().to_string()).unwrap(),
            on: true,
            width: 1,
            height: 1,
            can_control: false,
        },
    );
    let marker = a.send(&b.my_key(), "after it").unwrap();
    until("what came after", || b.message(&marker.id).is_some()).await;
    while let Ok(event) = rb.try_recv() {
        assert!(
            !matches!(
                event,
                whatsmine_core::CoreEvent::Call(CallEvent::PeerScreen { .. })
            ),
            "no call, no screen"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn decline_busy_cancel_and_lost() {
    let lookup = MemoryLookup::default();
    let (a, b, _ka, kb, _da, _db) = online_pair(&lookup).await;
    let (mut ra, mut rb) = (a.subscribe(), b.subscribe());

    // Declined.
    a.call_invite(&kb, false).unwrap();
    call_event(&mut rb, "incoming", |e| {
        matches!(e, CallEvent::Incoming { video: false, .. })
    })
    .await;
    b.call_answer(false);
    let ev = call_event(&mut ra, "declined", |e| {
        matches!(e, CallEvent::Ended { .. })
    })
    .await;
    assert!(matches!(
        ev,
        CallEvent::Ended {
            reason: CallEndReason::Declined,
            ..
        }
    ));

    // Cancelled while ringing: the callee sees a missed call.
    a.call_invite(&kb, false).unwrap();
    call_event(&mut rb, "incoming 2", |e| {
        matches!(e, CallEvent::Incoming { .. })
    })
    .await;
    a.call_end();
    let ev = call_event(&mut rb, "missed", |e| matches!(e, CallEvent::Ended { .. })).await;
    assert!(matches!(
        ev,
        CallEvent::Ended {
            reason: CallEndReason::Missed,
            ..
        }
    ));
    let ev = call_event(&mut ra, "cancelled", |e| {
        matches!(e, CallEvent::Ended { .. })
    })
    .await;
    assert!(matches!(
        ev,
        CallEvent::Ended {
            reason: CallEndReason::Cancelled,
            ..
        }
    ));

    // Busy: Bob is ringing, so his own attempt to call out is refused at once.
    a.call_invite(&kb, false).unwrap();
    call_event(&mut rb, "incoming 3", |e| {
        matches!(e, CallEvent::Incoming { .. })
    })
    .await;
    b.call_invite(&_ka, false).unwrap();
    let ev = call_event(&mut rb, "busy", |e| {
        matches!(
            e,
            CallEvent::Ended {
                reason: CallEndReason::Busy,
                ..
            }
        )
    })
    .await;
    assert!(matches!(ev, CallEvent::Ended { .. }));

    // Lost: the answered call ends when the connection goes away.
    b.call_answer(true);
    call_event(&mut ra, "connected", |e| {
        matches!(e, CallEvent::Connected { .. })
    })
    .await;
    a.shutdown();
    let ev = call_event(&mut rb, "lost", |e| matches!(e, CallEvent::Ended { .. })).await;
    assert!(matches!(
        ev,
        CallEvent::Ended {
            reason: CallEndReason::Lost,
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_call_state_can_be_read_by_a_screen_that_opens_late() {
    use whatsmine_core::calls::CallPhase;
    let lookup = MemoryLookup::default();
    let (a, b, ka, kb, _da, _db) = online_pair(&lookup).await;
    assert!(a.call_snapshot().is_none() && b.call_snapshot().is_none());

    a.call_invite(&kb, true).unwrap();
    until("ringing both sides", || {
        a.call_snapshot()
            .is_some_and(|s| s.phase == CallPhase::RingingOut)
            && b.call_snapshot()
                .is_some_and(|s| s.phase == CallPhase::RingingIn)
    })
    .await;
    let incoming = b.call_snapshot().unwrap();
    assert_eq!(
        (incoming.contact_id.as_str(), incoming.from.as_str()),
        (ka.as_str(), ka.as_str())
    );
    assert!(incoming.video && incoming.connected_since.is_none());

    b.call_answer(true);
    until("both active", || {
        a.call_snapshot()
            .is_some_and(|s| s.phase == CallPhase::Active)
            && b.call_snapshot()
                .is_some_and(|s| s.phase == CallPhase::Active)
    })
    .await;
    let active = a.call_snapshot().unwrap();
    assert!(active.connected_since.is_some());
    assert_eq!(active.peers, vec![kb.clone()]);

    // A new media listener takes over; the old one is cut off.
    let mut old = a.call_media();
    let mut new = a.call_media();
    b.call_audio(1, vec![5; 30]);
    let got = tokio::time::timeout(WAIT, new.recv())
        .await
        .expect("audio")
        .unwrap();
    assert_eq!(got.data, vec![5; 30]);
    assert!(
        old.recv().await.is_none(),
        "the earlier listener was replaced"
    );

    a.call_end();
    until("both idle", || {
        a.call_snapshot().is_none() && b.call_snapshot().is_none()
    })
    .await;
}

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let fut = async {
        while !cond() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(WAIT, fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}
