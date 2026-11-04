//! Who you are is not where you are. Real cores over loopback QUIC, with a DHT in the process.

use std::time::Duration;

use whatsmine_core::model::Trust;
use whatsmine_core::records::LocalRecords;
use whatsmine_core::{ContactState, Core, MessageStatus, NetMode, Settings};

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

struct Person {
    core: Core,
    _dir: tempfile::TempDir,
}

fn person(net: &LocalRecords, name: &str) -> Person {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::start(dir.path(), NetMode::Local(net.clone())).unwrap();
    core.save_settings(&Settings {
        display_name: name.into(),
        onboarded: true,
        ..Settings::default()
    })
    .unwrap();
    Person { core, _dir: dir }
}

async fn befriend(a: &Core, b: &Core) {
    a.add_contact(&b.invite_link(), None).unwrap();
    until("the request", || {
        b.contact(&a.my_key())
            .is_some_and(|c| c.state == ContactState::PendingIn)
    })
    .await;
    b.accept(&a.my_key()).unwrap();
    until("being accepted", || {
        a.contact(&b.my_key())
            .is_some_and(|c| c.state == ContactState::Accepted)
    })
    .await;
    until("being connected", || {
        a.is_online(&b.my_key()) && b.is_online(&a.my_key())
    })
    .await;
}

async fn message_gets_through(from: &Core, to: &Core, text: &str) {
    let m = from.send(&to.my_key(), text).unwrap();
    until("the message to arrive", || {
        to.messages(&from.my_key(), None, 50)
            .iter()
            .any(|x| x.id == m.id && x.body == text)
    })
    .await;
    until("the receipt", || {
        from.message(&m.id)
            .is_some_and(|x| x.status >= MessageStatus::Delivered)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn messages_flow_both_ways_after_an_invite() {
    let net = LocalRecords::default();
    let (a, b) = (person(&net, "Alice"), person(&net, "Bob"));
    befriend(&a.core, &b.core).await;
    message_gets_through(&a.core, &b.core, "hello").await;
    message_gets_through(&b.core, &a.core, "hi there").await;
    let c = a.core.contact(&b.core.my_key()).unwrap();
    assert_eq!(c.their_name, "Bob");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_greeting_comes_along_and_a_stranger_is_only_a_request() {
    let net = LocalRecords::default();
    let (a, b) = (person(&net, "Alice"), person(&net, "Bob"));
    a.core
        .add_contact_with_greeting(&b.core.invite_link(), None, "Hi, it's Alice from climbing")
        .unwrap();
    until("the request", || !b.core.requests().is_empty()).await;
    let keys = b.core.contact_keys(&a.core.my_key());
    assert_eq!(keys.greeting, "Hi, it's Alice from climbing");
    assert_eq!(keys.trust, Trust::Link);
    // Nothing connected: a request costs the phone nothing and shows it no address.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!b.core.is_online(&a.core.my_key()));
    assert!(b.core.connection_report().await.unwrap().peers.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_in_person_code_adds_both_without_asking_and_works_once() {
    let net = LocalRecords::default();
    let (a, b, c) = (
        person(&net, "Alice"),
        person(&net, "Bob"),
        person(&net, "Carol"),
    );
    let code = a.core.in_person_code();
    b.core.add_contact(&code, None).unwrap();
    until("both to be added", || {
        a.core
            .contact(&b.core.my_key())
            .is_some_and(|x| x.state == ContactState::Accepted)
            && b.core
                .contact(&a.core.my_key())
                .is_some_and(|x| x.state == ContactState::Accepted)
    })
    .await;
    assert_eq!(
        a.core.contact_keys(&b.core.my_key()).trust,
        Trust::MetInPerson
    );
    message_gets_through(&b.core, &a.core, "that was easy").await;

    // Someone who saw the code later gets nowhere.
    c.core.add_contact(&code, None).unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(a.core.requests().is_empty());
    assert!(a.core.contact(&c.core.my_key()).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_link_stops_and_the_others_go_on() {
    let net = LocalRecords::default();
    let (a, b, c) = (
        person(&net, "Alice"),
        person(&net, "Bob"),
        person(&net, "Carol"),
    );
    let club = a.core.new_invite("Climbing club", None, None);
    let id = a
        .core
        .invites()
        .into_iter()
        .find(|i| i.label == "Climbing club")
        .unwrap()
        .id;
    a.core.revoke_invite(&id);
    b.core.add_contact(&club, None).unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(a.core.requests().is_empty(), "the revoked link still works");

    c.core.add_contact(&a.core.invite_link(), None).unwrap();
    until("the main link to work", || !a.core.requests().is_empty()).await;
}

/// The reason all of this exists: bombarded through a leaked link, one button, and everyone you
/// know keeps talking to you and notices nothing.
#[tokio::test(flavor = "multi_thread")]
async fn after_a_reset_the_old_link_is_dead_and_contacts_notice_nothing() {
    let net = LocalRecords::default();
    let (a, b) = (person(&net, "Alice"), person(&net, "Bob"));
    befriend(&a.core, &b.core).await;
    message_gets_through(&a.core, &b.core, "before").await;
    let leaked = a.core.invite_link();

    // A flood through the leaked link.
    let mut flood = Vec::new();
    for n in 0..8 {
        let s = person(&net, &format!("Stranger {n}"));
        s.core.add_contact(&leaked, None).unwrap();
        flood.push(s);
    }
    until("the flood to show", || a.core.requests().len() >= 3).await;
    assert!(
        a.core.connection_report().await.unwrap().peers.len() == 1,
        "strangers never connect"
    );

    let fresh = a.core.reset_public_address();
    assert!(a.core.requests().is_empty(), "waiting requests are gone");
    assert_ne!(fresh.link(), leaked);

    // Contacts keep talking, both ways, without doing anything.
    until("Bob to be reconnected", || {
        a.core.is_online(&b.core.my_key()) && b.core.is_online(&a.core.my_key())
    })
    .await;
    message_gets_through(&b.core, &a.core, "still here?").await;
    message_gets_through(&a.core, &b.core, "still here").await;

    // The leaked link is dead, also for someone who tries again.
    let late = person(&net, "Late");
    late.core.add_contact(&leaked, None).unwrap();
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(a.core.requests().is_empty(), "the old link still works");

    // The new one works.
    let new = person(&net, "New friend");
    new.core.add_contact(&fresh.link(), None).unwrap();
    until("the new link to work", || !a.core.requests().is_empty()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn whoever_is_blocked_cannot_follow_and_contacts_can() {
    let net = LocalRecords::default();
    let (a, b, c) = (
        person(&net, "Alice"),
        person(&net, "Bob"),
        person(&net, "Carol"),
    );
    befriend(&a.core, &b.core).await;
    befriend(&a.core, &c.core).await;
    let old = c.core.contact_keys(&a.core.my_key()).location;
    assert!(old.is_some());

    a.core.block(&c.core.my_key()).unwrap();
    until("Bob to hear where Alice is now", || {
        b.core.contact_keys(&a.core.my_key()).location != old && b.core.is_online(&a.core.my_key())
    })
    .await;
    message_gets_through(&b.core, &a.core, "hello again").await;

    // Carol still has the old location, and it leads nowhere.
    assert_eq!(c.core.contact_keys(&a.core.my_key()).location, old);
    let m = c.core.send(&a.core.my_key(), "let me in").unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(a.core.messages(&c.core.my_key(), None, 10).is_empty());
    assert!(c.core.message(&m.id).unwrap().status < MessageStatus::Delivered);
    assert!(!a.core.is_online(&c.core.my_key()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_contact_found_again_after_both_restart() {
    let net = LocalRecords::default();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let start =
        |dir: &tempfile::TempDir| Core::start(dir.path(), NetMode::Local(net.clone())).unwrap();
    let (a, b) = (start(&dir_a), start(&dir_b));
    befriend(&a, &b).await;
    message_gets_through(&a, &b, "one").await;
    let (ka, kb) = (a.my_key(), b.my_key());
    a.shutdown();
    b.shutdown();
    drop((a, b));

    let (a, b) = (start(&dir_a), start(&dir_b));
    assert_eq!(
        (a.my_key(), b.my_key()),
        (ka.clone(), kb.clone()),
        "the person stays"
    );
    until("them finding each other", || {
        a.is_online(&kb) && b.is_online(&ka)
    })
    .await;
    message_gets_through(&a, &b, "two").await;
    message_gets_through(&b, &a, "three").await;
}

/// Copying the folder gives nobody the chats or the identity.
#[tokio::test(flavor = "multi_thread")]
async fn nothing_readable_lies_on_disk() {
    let net = LocalRecords::default();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = Core::start(dir_a.path(), NetMode::Local(net.clone())).unwrap();
    let b = Core::start(dir_b.path(), NetMode::Local(net.clone())).unwrap();
    a.save_settings(&Settings {
        display_name: "Alicetheuniquename".into(),
        onboarded: true,
        ..Settings::default()
    })
    .unwrap();
    befriend(&a, &b).await;
    message_gets_through(&a, &b, "averyuniquesecretmessage").await;
    a.set_draft(&b.my_key(), "averyuniquesecretdraft");
    a.shutdown();
    b.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;

    for dir in [dir_a.path(), dir_b.path()] {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            if !entry.path().is_file() {
                continue;
            }
            let bytes = std::fs::read(entry.path()).unwrap();
            for secret in [&b"averyuniquesecret"[..], b"Alicetheuniquename"] {
                assert!(
                    !bytes.windows(secret.len()).any(|w| w == secret),
                    "{} has it in the clear",
                    entry.path().display()
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_recovery_words_are_twelve_and_stay_the_same() {
    let net = LocalRecords::default();
    let a = person(&net, "Alice");
    let words = a.core.recovery_words();
    assert_eq!(words.len(), 12);
    assert_eq!(a.core.recovery_words(), words);
}
