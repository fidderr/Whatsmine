//! Two real cores over loopback QUIC: file transfer, size limit, avatars.

use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use tokio::sync::broadcast::Receiver;
use whatsmine_core::{Core, CoreEvent, MessageStatus, NetMode, Settings, TransferState, files};

const WAIT: Duration = Duration::from_secs(60);

async fn wait_for(
    rx: &mut Receiver<CoreEvent>,
    what: &str,
    mut pred: impl FnMut(&CoreEvent) -> bool,
) -> CoreEvent {
    let fut = async {
        loop {
            match rx.recv().await {
                Ok(ev) if pred(&ev) => return ev,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => panic!("event channel closed while waiting for {what}: {e}"),
            }
        }
    };
    tokio::time::timeout(WAIT, fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Alice and Bob, already contacts and connected.
async fn pair(
    lookup: &MemoryLookup,
    dirs: [&std::path::Path; 2],
    bob_settings: impl FnOnce(&mut Settings),
) -> (Core, Core, Receiver<CoreEvent>, Receiver<CoreEvent>) {
    let start = |dir: &std::path::Path, name: &str, tweak: Box<dyn FnOnce(&mut Settings)>| {
        let core = Core::start(dir, NetMode::Local(lookup.clone())).unwrap();
        let mut s = Settings {
            display_name: name.into(),
            onboarded: true,
            ..Settings::default()
        };
        tweak(&mut s);
        core.save_settings(&s).unwrap();
        core
    };
    let a = start(dirs[0], "Alice", Box::new(|_| {}));
    let b = start(dirs[1], "Bob", Box::new(bob_settings));
    let (mut ra, mut rb) = (a.subscribe(), b.subscribe());
    a.add_contact(&b.my_key(), None).unwrap();
    wait_for(&mut rb, "request", |e| {
        matches!(e, CoreEvent::RequestReceived { .. })
    })
    .await;
    b.accept(&a.my_key()).unwrap();
    wait_for(&mut ra, "online", |e| {
        matches!(e, CoreEvent::Presence { online: true, .. })
    })
    .await;
    (a, b, ra, rb)
}

fn write_pattern(path: &std::path::Path, len: usize) {
    let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
    std::fs::write(path, data).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn file_arrives_complete_and_verified() {
    let lookup = MemoryLookup::new();
    let (da, db_, src) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (a, b, mut ra, mut rb) = pair(&lookup, [da.path(), db_.path()], |_| {}).await;
    let (ka, kb) = (a.my_key(), b.my_key());

    let file = src.path().join("holiday.JPG");
    write_pattern(&file, 5 * 1024 * 1024 + 123);
    let sent = a
        .send_file(&kb, &file, "../../holiday.JPG", "image/jpeg", "kijk!", None)
        .unwrap();
    // The original may disappear right away: we hold our own copy.
    std::fs::remove_file(&file).unwrap();
    assert_eq!(sent.attachment.as_ref().unwrap().name, "holiday.JPG");

    let ev = wait_for(
        &mut rb,
        "file offer",
        |e| matches!(e, CoreEvent::MessageAdded(m) if m.id == sent.id),
    )
    .await;
    let CoreEvent::MessageAdded(offered) = ev else {
        unreachable!()
    };
    assert_eq!(offered.body, "kijk!");

    wait_for(&mut rb, "file complete", |e| {
        matches!(e, CoreEvent::MessageUpdated(m)
            if m.id == sent.id && m.attachment.as_ref().is_some_and(|a| a.state == TransferState::Done))
    })
    .await;
    wait_for(&mut ra, "sender sees delivered", |e| {
        matches!(e, CoreEvent::MessageStatus { message_id, status: MessageStatus::Delivered, .. } if *message_id == sent.id)
    })
    .await;

    let got = b.message(&sent.id).unwrap().attachment.unwrap();
    let path = std::path::PathBuf::from(got.path.expect("stored"));
    assert!(path.starts_with(b.media_dir()), "stored in our own folder");
    assert_eq!(path.extension().unwrap(), "jpg");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), got.size);
    assert_eq!(files::to_hex(&files::hash_file(&path).unwrap()), got.sha256);
    assert!(
        !files::partial_path(&b.media_dir(), &sent.id).exists(),
        "no partial file left"
    );
    let mine = a.message(&sent.id).unwrap().attachment.unwrap();
    assert_eq!(mine.state, TransferState::Done);
    assert_eq!(a.chats()[0].contact.id, kb);
    let _ = ka;

    // Deleting the message deletes the file.
    b.delete_message(&sent.id);
    assert!(!path.exists());

    a.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn offers_over_the_receivers_limit_are_declined() {
    let lookup = MemoryLookup::new();
    let (da, db_, src) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (a, b, mut ra, _rb) = pair(&lookup, [da.path(), db_.path()], |s| s.max_file_mb = 1).await;
    let kb = b.my_key();

    let big = src.path().join("big.bin");
    write_pattern(&big, 2 * 1024 * 1024);
    let sent = a
        .send_file(&kb, &big, "big.bin", "application/octet-stream", "", None)
        .unwrap();
    wait_for(&mut ra, "declined", |e| {
        matches!(e, CoreEvent::MessageUpdated(m)
            if m.id == sent.id && m.attachment.as_ref().is_some_and(|a| a.state == TransferState::Declined))
    })
    .await;
    let theirs = b.message(&sent.id).unwrap().attachment.unwrap();
    assert_eq!(theirs.state, TransferState::Declined);
    assert!(theirs.path.is_none(), "nothing was stored");
    assert!(!files::partial_path(&b.media_dir(), &sent.id).exists());

    // A small file still goes through.
    let small = src.path().join("small.txt");
    write_pattern(&small, 1000);
    let ok = a
        .send_file(&kb, &small, "small.txt", "text/plain", "", None)
        .unwrap();
    wait_for(&mut ra, "small delivered", |e| {
        matches!(e, CoreEvent::MessageStatus { message_id, status: MessageStatus::Delivered, .. } if *message_id == ok.id)
    })
    .await;

    a.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_file_is_sent_when_the_peer_comes_back() {
    let lookup = MemoryLookup::new();
    let (da, db_, src) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (a, b, mut ra, _rb) = pair(&lookup, [da.path(), db_.path()], |_| {}).await;
    let kb = b.my_key();
    b.shutdown();
    drop(b);
    wait_for(&mut ra, "bob offline", |e| {
        matches!(e, CoreEvent::Presence { online: false, .. })
    })
    .await;

    let f = src.path().join("later.bin");
    write_pattern(&f, 300_000);
    let sent = a
        .send_file(&kb, &f, "later.bin", "application/octet-stream", "", None)
        .unwrap();

    let b = Core::start(db_.path(), NetMode::Local(lookup.clone())).unwrap();
    let mut rb = b.subscribe();
    wait_for(&mut rb, "file after restart", |e| {
        matches!(e, CoreEvent::MessageUpdated(m)
            if m.id == sent.id && m.attachment.as_ref().is_some_and(|a| a.state == TransferState::Done))
    })
    .await;
    wait_for(&mut ra, "delivered", |e| {
        matches!(e, CoreEvent::MessageStatus { message_id, status: MessageStatus::Delivered, .. } if *message_id == sent.id)
    })
    .await;

    a.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn profile_pictures_reach_contacts_and_can_be_removed() {
    let lookup = MemoryLookup::new();
    let (da, db_, src) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let (a, b, _ra, mut rb) = pair(&lookup, [da.path(), db_.path()], |_| {}).await;
    let ka = a.my_key();

    let pic = src.path().join("me.png");
    image::RgbImage::from_fn(640, 480, |x, y| image::Rgb([x as u8, y as u8, 7]))
        .save(&pic)
        .unwrap();
    a.set_avatar(&pic).unwrap();
    assert!(a.settings().avatar.is_some());

    let wait_avatar = |want: bool| {
        let b = &b;
        let ka = ka.clone();
        async move {
            for _ in 0..100 {
                if b.contact(&ka).unwrap().avatar.is_some() == want {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("avatar present={want} never happened");
        }
    };
    wait_avatar(true).await;
    let path = b.contact(&ka).unwrap().avatar.unwrap();
    let img = image::open(&path).unwrap();
    assert_eq!((img.width(), img.height()), (256, 256));

    a.clear_avatar().unwrap();
    wait_avatar(false).await;
    assert!(!std::path::Path::new(&path).exists());
    let _ = &mut rb;

    a.shutdown();
    b.shutdown();
}
