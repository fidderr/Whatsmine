//! A phone and a PC, one person: everything that happens on one shows up on the other.

use std::time::Duration;

use whatsmine_core::records::LocalRecords;
use whatsmine_core::{ContactState, Core, MessageStatus, NetMode};

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

struct World {
    net: LocalRecords,
    phone: Core,
    pc: Core,
    friend: Core,
    _dirs: Vec<tempfile::TempDir>,
}

/// A phone with a friend, and a new device linked to it.
async fn world() -> World {
    let net = LocalRecords::default();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let start = |i: usize| Core::start(dirs[i].path(), NetMode::Local(net.clone())).unwrap();
    let (phone, friend) = (start(0), start(2));
    let mut settings = phone.settings();
    settings.display_name = "Roy".into();
    settings.onboarded = true;
    phone.save_settings(&settings).unwrap();
    phone.add_contact(&friend.invite_link(), None).unwrap();
    until("the request", || !friend.requests().is_empty()).await;
    friend.accept(&phone.my_key()).unwrap();
    until("being accepted", || {
        phone
            .contact(&friend.my_key())
            .is_some_and(|c| c.state == ContactState::Accepted)
    })
    .await;
    let m = phone.send(&friend.my_key(), "before linking").unwrap();
    until("the first message", || friend.message(&m.id).is_some()).await;

    let new = start(1);
    assert_ne!(new.my_key(), phone.my_key());
    let code = phone.start_link().await;
    new.join_device(&code).await.expect("joining");
    until("the phone to know", || phone.device_state().linked).await;
    new.shutdown();
    drop(new);
    let pc = start(1);
    assert_eq!(pc.my_key(), phone.my_key());
    assert!(pc.device_state().linked);
    assert_eq!(pc.device_state().slot, 1);
    assert_eq!(pc.settings().display_name, "Roy");
    assert!(pc.message(&m.id).is_some(), "the history came along");
    assert_eq!(
        pc.contact(&friend.my_key()).map(|c| c.state),
        Some(ContactState::Accepted)
    );
    World {
        net,
        phone,
        pc,
        friend,
        _dirs: dirs,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn both_devices_talk_and_stay_the_same() {
    let w = world().await;
    let kf = w.friend.my_key();
    let me = w.phone.my_key();

    // Sent from the PC: the friend gets it, the phone shows it.
    let a = w.pc.send(&kf, "from the pc").unwrap();
    until("the friend to get it", || w.friend.message(&a.id).is_some()).await;
    until("the phone to show it", || w.phone.message(&a.id).is_some()).await;
    assert!(w.phone.message(&a.id).unwrap().outgoing);

    // Sent from the phone: same, the other way around.
    let b = w.phone.send(&kf, "from the phone").unwrap();
    until("the friend to get that", || {
        w.friend.message(&b.id).is_some()
    })
    .await;
    until("the pc to show it", || w.pc.message(&b.id).is_some()).await;

    // From the friend: on both, whichever one it reached.
    let c = w.friend.send(&me, "hello you").unwrap();
    until("both to have it", || {
        w.phone.message(&c.id).is_some() && w.pc.message(&c.id).is_some()
    })
    .await;

    // Read on one is read on the other.
    let unread = |core: &Core| {
        core.chats()
            .iter()
            .find(|x| x.contact.id == kf)
            .map_or(0, |x| x.unread)
    };
    until("it to be unread on both", || {
        unread(&w.phone) == 1 && unread(&w.pc) == 1
    })
    .await;
    w.phone.mark_read(&kf);
    until("read on the pc too", || unread(&w.pc) == 0).await;

    // A reaction and a deletion follow.
    w.pc.react(&c.id, Some("👍"));
    until("the reaction on the phone", || {
        w.phone.message(&c.id).unwrap().extra.reactions.len() == 1
    })
    .await;
    until("the reaction at the friend", || {
        w.friend.message(&c.id).unwrap().extra.reactions.len() == 1
    })
    .await;
    w.phone.delete_message(&c.id);
    until("gone from the pc", || w.pc.message(&c.id).is_none()).await;
    // Delivered is delivered, on both.
    until("delivered on the phone", || {
        w.phone.message(&a.id).unwrap().status >= MessageStatus::Delivered
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_wait_for_the_other_device_and_new_people_show_up() {
    let mut w = world().await;
    let kf = w.friend.my_key();
    let dir = w._dirs[1].path().to_path_buf();
    w.pc.shutdown();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // While the PC is off.
    let m = w.phone.send(&kf, "while it was off").unwrap();
    until("the friend to get it", || w.friend.message(&m.id).is_some()).await;
    let dir_new = tempfile::tempdir().unwrap();
    let other = Core::start(dir_new.path(), NetMode::Local(w.net.clone())).unwrap();
    w.phone.add_contact(&other.invite_link(), None).unwrap();
    until("the second request", || !other.requests().is_empty()).await;
    other.accept(&w.phone.my_key()).unwrap();
    until("the second friend", || {
        w.phone
            .contact(&other.my_key())
            .is_some_and(|c| c.state == ContactState::Accepted)
    })
    .await;
    w.phone.rename(&kf, "Old friend");
    let mut prefs = w.phone.chat_prefs(&kf);
    prefs.notes = "A note".into();
    w.phone.set_chat_prefs(&kf, &prefs);

    // The PC comes back, and catches up.
    w.pc = Core::start(&dir, NetMode::Local(w.net.clone())).unwrap();
    until("the message on the pc", || w.pc.message(&m.id).is_some()).await;
    until("the new person on the pc", || {
        w.pc.contact(&other.my_key())
            .is_some_and(|c| c.state == ContactState::Accepted)
    })
    .await;
    until("the name and note on the pc", || {
        w.pc.contact(&kf).and_then(|c| c.nickname).as_deref() == Some("Old friend")
            && w.pc.chat_prefs(&kf).notes == "A note"
    })
    .await;
    // And can talk to the new person right away.
    let hi = w.pc.send(&other.my_key(), "hi from the pc").unwrap();
    until("the new person to get it", || {
        other.message(&hi.id).is_some()
    })
    .await;
    other.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_code_gets_nothing_and_a_device_can_be_removed() {
    let w = world().await;
    let dir = tempfile::tempdir().unwrap();
    let stranger = Core::start(dir.path(), NetMode::Local(w.net.clone())).unwrap();
    assert!(stranger.join_device("not a code").await.is_err());
    let made_up = whatsmine_core::sibling::link_code(&[7; 32]);
    assert!(stranger.join_device(&made_up).await.is_err());
    // Already two: no third.
    assert!(w.pc.device_state().linked);

    w.phone.remove_other_device();
    until("the pc to be wiped", || w.pc.chats().is_empty()).await;
    until("the phone to be alone again", || {
        !w.phone.device_state().linked
    })
    .await;
    // The phone goes on.
    let m = w.phone.send(&w.friend.my_key(), "just me now").unwrap();
    until("the friend to get it", || w.friend.message(&m.id).is_some()).await;
    stranger.shutdown();
}
