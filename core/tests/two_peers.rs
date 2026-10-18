//! Two real cores talking over loopback QUIC: the full contact and message
//! lifecycle, including going offline and catching up.

use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use tokio::sync::broadcast::Receiver;
use whatsmine_core::{ContactState, Core, CoreEvent, MessageStatus, NetMode, Settings};

const WAIT: Duration = Duration::from_secs(40);

async fn wait_for(
    rx: &mut Receiver<CoreEvent>,
    what: &str,
    mut pred: impl FnMut(&CoreEvent) -> bool,
) -> CoreEvent {
    let fut = async {
        loop {
            match rx.recv().await {
                Ok(ev) if pred(&ev) => return ev,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
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

fn status_of(core: &Core, contact: &str, message_id: &str) -> MessageStatus {
    core.messages(contact, None, 100)
        .into_iter()
        .find(|m| m.id == message_id)
        .expect("message stored")
        .status
}

#[tokio::test(flavor = "multi_thread")]
async fn full_lifecycle() {
    let lookup = MemoryLookup::new();
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let a = start(dir_a.path(), &lookup, "Alice");
    let b = start(dir_b.path(), &lookup, "Bob");
    let (ka, kb) = (a.my_key(), b.my_key());
    let mut ra = a.subscribe();
    let mut rb = b.subscribe();

    // Request -> accept.
    a.add_contact(&kb, Some("bobby")).unwrap();
    wait_for(&mut rb, "request", |e| {
        matches!(e, CoreEvent::RequestReceived { contact_id, name } if *contact_id == ka && name == "Alice")
    })
    .await;
    assert_eq!(b.requests().len(), 1);
    b.accept(&ka).unwrap();
    wait_for(
        &mut ra,
        "accepted",
        |e| matches!(e, CoreEvent::Accepted { contact_id } if *contact_id == kb),
    )
    .await;
    let bob = a.contact(&kb).unwrap();
    assert_eq!(bob.state, ContactState::Accepted);
    assert_eq!(bob.their_name, "Bob");
    assert_eq!(bob.display_name(), "bobby");
    wait_for(
        &mut ra,
        "bob online",
        |e| matches!(e, CoreEvent::Presence { contact_id, online: true, .. } if *contact_id == kb),
    )
    .await;
    assert!(a.is_online(&kb));

    // Message -> delivered -> read.
    let m1 = a.send(&kb, "hoi Bob").unwrap();
    wait_for(&mut rb, "message", |e| {
        matches!(e, CoreEvent::MessageAdded(m) if m.id == m1.id && !m.outgoing && m.body == "hoi Bob")
    })
    .await;
    wait_for(&mut ra, "delivered", |e| {
        matches!(e, CoreEvent::MessageStatus { message_id, status: MessageStatus::Delivered, .. } if *message_id == m1.id)
    })
    .await;
    assert_eq!(b.chats()[0].unread, 1);
    b.mark_read(&ka);
    wait_for(&mut ra, "read", |e| {
        matches!(e, CoreEvent::MessageStatus { message_id, status: MessageStatus::Read, .. } if *message_id == m1.id)
    })
    .await;
    assert_eq!(b.chats()[0].unread, 0);

    // Typing.
    b.typing(&ka);
    wait_for(
        &mut ra,
        "typing",
        |e| matches!(e, CoreEvent::Typing { contact_id } if *contact_id == kb),
    )
    .await;

    // Bob goes offline: Alice sees last seen and queues messages.
    b.shutdown();
    drop(b);
    wait_for(&mut ra, "bob offline", |e| {
        matches!(e, CoreEvent::Presence { contact_id, online: false, last_seen: Some(_) } if *contact_id == kb)
    })
    .await;
    let m2 = a.send(&kb, "ben je er?").unwrap();
    let m3 = a.send(&kb, "hallo??").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(status_of(&a, &kb, &m2.id) < MessageStatus::Delivered);

    // Bob comes back with the same identity: the outbox flushes, in order.
    let b = Core::start(dir_b.path(), NetMode::Local(lookup.clone())).unwrap();
    assert_eq!(b.my_key(), kb, "identity survives restart");
    let mut rb = b.subscribe();
    for want in [&m2.id, &m3.id] {
        wait_for(&mut ra, "queued delivered", |e| {
            matches!(e, CoreEvent::MessageStatus { message_id, status: MessageStatus::Delivered, .. } if message_id == want)
        })
        .await;
    }
    let history = b.messages(&ka, None, 100);
    assert_eq!(
        history.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
        ["hoi Bob", "ben je er?", "hallo??"],
        "history survives restart, no duplicates, in order"
    );

    // Reply direction works after the restart too.
    let r = b.send(&ka, "ja!").unwrap();
    wait_for(
        &mut ra,
        "reply",
        |e| matches!(e, CoreEvent::MessageAdded(m) if m.id == r.id),
    )
    .await;

    // Blocking: Bob blocks Alice, her next message never arrives.
    b.block(&ka).unwrap();
    wait_for(
        &mut ra,
        "bob gone after block",
        |e| matches!(e, CoreEvent::Presence { contact_id, online: false, .. } if *contact_id == kb),
    )
    .await;
    let blocked = a.send(&kb, "mag ik nog?").unwrap();
    tokio::time::sleep(Duration::from_secs(8)).await;
    assert!(status_of(&a, &kb, &blocked.id) < MessageStatus::Delivered);
    assert!(
        b.messages(&ka, None, 100)
            .iter()
            .all(|m| m.id != blocked.id)
    );
    while let Ok(ev) = rb.try_recv() {
        assert!(
            !matches!(&ev, CoreEvent::MessageAdded(m) if m.id == blocked.id),
            "blocked message leaked"
        );
    }

    a.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn mutual_add_is_consent_and_strangers_cannot_message() {
    let lookup = MemoryLookup::new();
    let (d1, d2, d3) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let a = start(d1.path(), &lookup, "A");
    let b = start(d2.path(), &lookup, "B");
    let stranger = start(d3.path(), &lookup, "S");
    let mut ra = a.subscribe();
    let mut rb = b.subscribe();

    // Both add each other: accepted without anyone pressing accept.
    a.add_contact(&b.my_key(), None).unwrap();
    b.add_contact(&a.my_key(), None).unwrap();
    wait_for(&mut ra, "a accepted", |e| {
        matches!(e, CoreEvent::Accepted { .. })
    })
    .await;
    wait_for(&mut rb, "b accepted", |e| {
        matches!(e, CoreEvent::Accepted { .. })
    })
    .await;

    // A stranger can send a request, but messages are refused until accepted.
    stranger.add_contact(&a.my_key(), None).unwrap();
    wait_for(&mut ra, "stranger request", |e| {
        matches!(e, CoreEvent::RequestReceived { .. })
    })
    .await;
    let spam = stranger.send(&a.my_key(), "buy now").unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(a.messages(&stranger.my_key(), None, 10).is_empty());
    assert!(status_of(&stranger, &a.my_key(), &spam.id) < MessageStatus::Delivered);

    // Once accepted, the queued message is delivered.
    a.accept(&stranger.my_key()).unwrap();
    wait_for(
        &mut ra,
        "spam delivered after accept",
        |e| matches!(e, CoreEvent::MessageAdded(m) if m.id == spam.id),
    )
    .await;

    for c in [a, b, stranger] {
        c.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_chat_keeps_the_contact_and_a_new_message_brings_it_back() {
    let lookup = MemoryLookup::new();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(dir_a.path(), &lookup, "Alice");
    let b = start(dir_b.path(), &lookup, "Bob");
    let (ka, kb) = (a.my_key(), b.my_key());
    let (mut ra, mut rb) = (a.subscribe(), b.subscribe());
    a.add_contact(&kb, None).unwrap();
    b.add_contact(&ka, None).unwrap();
    wait_for(
        &mut ra,
        "bob online",
        |e| matches!(e, CoreEvent::Presence { contact_id, online: true, .. } if *contact_id == kb),
    )
    .await;

    a.send(&kb, "one").unwrap();
    wait_for(&mut rb, "first message", |e| matches!(e, CoreEvent::MessageAdded(_))).await;
    assert_eq!(b.chats().len(), 1);

    // Deleting the chat empties it and takes it off the list, but Alice stays
    // in the address book.
    b.set_pinned(&ka, true);
    b.hide_chat(&ka);
    assert!(b.chats().is_empty());
    assert!(b.messages(&ka, None, 10).is_empty());
    let book = b.address_book();
    assert_eq!(book.len(), 1);
    assert_eq!(book[0].id, ka);
    assert!(book[0].pinned, "hiding does not undo the pin");

    // Her next message opens the chat again, with only that message in it.
    a.send(&kb, "two").unwrap();
    wait_for(&mut rb, "second message", |e| matches!(e, CoreEvent::MessageAdded(_))).await;
    let chats = b.chats();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].last_message.as_ref().unwrap().body, "two");
    assert_eq!(b.messages(&ka, None, 10).len(), 1);

    // Writing to a deleted chat also brings it back.
    b.hide_chat(&ka);
    assert!(b.chats().is_empty());
    b.send(&ka, "three").unwrap();
    assert_eq!(b.chats().len(), 1);

    // Mute and block round trip.
    b.set_muted(&ka, true);
    assert!(b.contact(&ka).unwrap().muted);
    b.block(&ka).unwrap();
    assert_eq!(b.blocked().len(), 1);
    assert!(b.chats().is_empty() && b.address_book().is_empty());
    b.unblock(&ka).unwrap();
    assert!(b.blocked().is_empty());
}
