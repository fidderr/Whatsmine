//! Losing a device: a backup brings everything back, the words alone bring back who you are.

use std::time::Duration;

use whatsmine_core::records::LocalRecords;
use whatsmine_core::{ContactState, Core, NetMode};

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let fut = async {
        while !cond() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(60), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

async fn friends(net: &LocalRecords, a: &Core, b: &Core) {
    a.add_contact(&b.invite_link(), None).unwrap();
    until("the request", || !b.requests().is_empty()).await;
    b.accept(&a.my_key()).unwrap();
    until("being accepted", || {
        a.contact(&b.my_key())
            .is_some_and(|c| c.state == ContactState::Accepted)
    })
    .await;
    let _ = net;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_brings_back_everything_on_a_new_device() {
    let net = LocalRecords::default();
    let (dir_a, dir_b, dir_new) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let a = Core::start(dir_a.path(), NetMode::Local(net.clone())).unwrap();
    let b = Core::start(dir_b.path(), NetMode::Local(net.clone())).unwrap();
    friends(&net, &a, &b).await;
    let sent = a.send(&b.my_key(), "before the loss").unwrap();
    until("it to arrive", || b.message(&sent.id).is_some()).await;

    let words = a.recovery_words().join(" ");
    let file = a.make_backup().unwrap();
    let (ka, kb) = (a.my_key(), b.my_key());
    assert!(!file.windows(16).any(|w| w == b"before the loss\0"));
    a.shutdown();
    drop(a);

    // The new device starts as a stranger, and is told to come back.
    let new = Core::start(dir_new.path(), NetMode::Local(net.clone())).unwrap();
    assert_ne!(new.my_key(), ka);
    assert_eq!(
        new.restore_on_next_start("not the right words", Some(file.clone()))
            .unwrap_err()
            .to_string(),
        "wrong_words"
    );
    let other = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    assert_eq!(
        new.restore_on_next_start(other, Some(file.clone()))
            .unwrap_err()
            .to_string(),
        "wrong_backup"
    );
    new.restore_on_next_start(&words, Some(file)).unwrap();
    new.shutdown();
    drop(new);

    let back = Core::start(dir_new.path(), NetMode::Local(net.clone())).unwrap();
    assert_eq!(back.my_key(), ka);
    assert!(back.message(&sent.id).is_some());
    assert_eq!(
        back.contact(&kb).map(|c| c.state),
        Some(ContactState::Accepted)
    );
    // Nothing of what was to be restored stays behind.
    assert!(!dir_new.path().join("restore.pending").exists());

    // And the others talk to it as before.
    let reply = b.send(&ka, "welcome back").unwrap();
    until("the reply", || back.message(&reply.id).is_some()).await;
    let answer = back.send(&kb, "thanks").unwrap();
    until("the answer", || b.message(&answer.id).is_some()).await;
    back.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_words_bring_back_who_you_are_and_an_old_friend_answers() {
    let net = LocalRecords::default();
    let (dir_a, dir_b, dir_new) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let a = Core::start(dir_a.path(), NetMode::Local(net.clone())).unwrap();
    let b = Core::start(dir_b.path(), NetMode::Local(net.clone())).unwrap();
    friends(&net, &a, &b).await;
    let words = a.recovery_words().join(" ");
    let ka = a.my_key();
    a.shutdown();
    drop(a);

    let new = Core::start(dir_new.path(), NetMode::Local(net.clone())).unwrap();
    new.restore_on_next_start(&words, None).unwrap();
    new.shutdown();
    drop(new);
    let back = Core::start(dir_new.path(), NetMode::Local(net.clone())).unwrap();
    assert_eq!(back.my_key(), ka);
    assert!(back.chats().is_empty(), "the words hold nothing else");

    // Adding an old friend again: they know this person, and answer without asking.
    back.add_contact(&b.invite_link(), None).unwrap();
    until("the old friend to answer", || {
        back.contact(&b.my_key())
            .is_some_and(|c| c.state == ContactState::Accepted)
    })
    .await;
    assert!(b.requests().is_empty());
    back.shutdown();
    b.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn notes_to_self_stay_here_and_need_nobody() {
    let net = LocalRecords::default();
    let dir = tempfile::tempdir().unwrap();
    let a = Core::start(dir.path(), NetMode::Local(net)).unwrap();
    let notes = a.notes_chat("Notes to self");
    let m = a.send(&notes, "buy chalk").unwrap();
    assert_eq!(
        a.message(&m.id).unwrap().status,
        whatsmine_core::MessageStatus::Read
    );
    assert!(a.address_book().is_empty(), "you are not your own contact");
    assert_eq!(a.chats().len(), 1);
    a.shutdown();
}
