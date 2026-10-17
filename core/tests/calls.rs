//! Calls between two real cores over loopback QUIC: ring, answer, audio and
//! video flowing both ways, hang up, decline, busy, and a lost connection.

use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use tokio::sync::broadcast::Receiver;
use whatsmine_core::calls::{CallEndReason, CallEvent, MediaKind};
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
    a.add_contact(&kb, None).unwrap();
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
    let lookup = MemoryLookup::new();
    let (a, b, ka, kb, _da, _db) = online_pair(&lookup).await;
    let (mut ra, mut rb) = (a.subscribe(), b.subscribe());
    let mut media_a = a.take_call_media().expect("media receiver");
    let mut media_b = b.take_call_media().expect("media receiver");
    assert!(a.take_call_media().is_none(), "handed out once");

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
                    MediaKind::Audio => { assert_eq!(m.call_id, call); assert_eq!(m.data.len(), 80); audio_at_b += 1 }
                    MediaKind::Video => { assert!(m.key); assert_eq!(m.ts, 1000); assert_eq!(m.data, vec![7; 20_000]); video_at_b += 1 }
                },
                Some(m) = media_a.recv() => match m.kind {
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
async fn decline_busy_cancel_and_lost() {
    let lookup = MemoryLookup::new();
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
