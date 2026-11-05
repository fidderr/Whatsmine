//! Group chats and group calls between three real cores over loopback QUIC.
//! Alice administers the group; Bob and Carol are her contacts but not each
//! other's, so they only ever talk because they share the group.

use std::collections::HashSet;
use std::time::Duration;

use whatsmine_core::calls::{CallEvent, MediaKind};
use whatsmine_core::records::LocalRecords as MemoryLookup;
use whatsmine_core::{
    ContactKind, ContactState, Core, CoreEvent, MessageStatus, NetMode, Settings, TransferState,
};

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let fut = async {
        while !cond() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(40), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
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

/// `a` asks `b`; `b` accepts; both online.
async fn befriend(a: &Core, b: &Core) {
    a.add_contact(&b.invite_link(), None).unwrap();
    until("request", || !b.requests().is_empty()).await;
    b.accept(&a.my_key()).unwrap();
    until("online", || {
        a.is_online(&b.my_key()) && b.is_online(&a.my_key())
    })
    .await;
}

struct Trio {
    a: Core,
    b: Core,
    c: Core,
    group: String,
    dirs: Vec<tempfile::TempDir>,
    lookup: MemoryLookup,
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_call_is_over_for_everyone_when_the_last_one_is_left_alone() {
    let t = trio().await;
    let (mut eb, mut ec) = (t.b.subscribe(), t.c.subscribe());
    t.a.call_invite(&t.group, false).unwrap();
    for rx in [&mut eb, &mut ec] {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv())
                .await
                .expect("incoming")
                .unwrap();
            if matches!(ev, CoreEvent::Call(CallEvent::Incoming { .. })) {
                break;
            }
        }
    }
    t.b.call_answer(true);
    t.c.call_answer(true);
    until("all three in the call", || {
        [&t.a, &t.b, &t.c]
            .iter()
            .all(|core| core.call_snapshot().is_some_and(|c| c.peers.len() == 2))
    })
    .await;

    // Two leave. Carol does nothing: her call ends by itself, and nobody sees a call any more.
    t.a.call_end();
    t.b.call_end();
    until("the call to end for Carol", || {
        t.c.call_snapshot().is_none()
    })
    .await;
    until("no call left for anyone", || {
        [&t.a, &t.b, &t.c]
            .iter()
            .all(|core| core.group_call(&t.group).is_none())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn what_only_admins_may_change_stays_theirs_and_everyone_can_react() {
    let t = trio().await;
    t.a.group_set_rules(&t.group, |r| {
        r.only_admins_send = true;
        r.only_admins_edit = true;
    })
    .unwrap();
    until("the rules", || {
        t.b.group_info(&t.group)
            .is_some_and(|g| g.rules.only_admins_send)
    })
    .await;
    let m = t.a.send(&t.group, "Saturday at ten").unwrap();
    until("the post", || t.b.message(&m.id).is_some()).await;

    // A member reacts, also where only admins write.
    t.b.react(&m.id, Some("👍"));
    until("the reaction", || {
        t.a.message(&m.id)
            .is_some_and(|m| !m.extra.reactions.is_empty())
    })
    .await;

    // The disappearing time is not for a member to change: the app does not try.
    let mut prefs = t.b.chat_prefs(&t.group);
    prefs.disappear_ms = Some(60_000);
    t.b.set_chat_prefs(&t.group, &prefs);
    assert_eq!(t.b.chat_prefs(&t.group).disappear_ms, None);
    assert_eq!(t.b.changes_owed_to(&t.a.my_key()), 0);

    // Whoever changes the app is refused, and told so: it is not sent again and again.
    t.b.tell_chat_as_is(
        &t.group,
        "",
        &whatsmine_core::rich::MessageChange::ChatTimer(Some(60_000)),
    );
    assert_eq!(t.b.changes_owed_to(&t.a.my_key()), 1);
    until("the refusal to be confirmed", || {
        t.b.changes_owed_to(&t.a.my_key()) == 0
    })
    .await;
    assert_eq!(t.a.chat_prefs(&t.group).disappear_ms, None);
}

async fn trio() -> Trio {
    let lookup = MemoryLookup::default();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = start(dirs[0].path(), &lookup, "Alice");
    let b = start(dirs[1].path(), &lookup, "Bob");
    let c = start(dirs[2].path(), &lookup, "Carol");
    befriend(&a, &b).await;
    befriend(&a, &c).await;
    let g = a
        .create_group("Weekend", &[b.my_key(), c.my_key()])
        .unwrap();
    assert_eq!(g.kind, ContactKind::Group);
    // Everyone ends up with the group and a link to everyone else.
    for core in [&b, &c] {
        until("member gets the group", || core.group_info(&g.id).is_some()).await;
    }
    until("bob and carol find each other", || {
        b.is_online(&c.my_key()) && c.is_online(&b.my_key())
    })
    .await;
    Trio {
        group: g.id,
        a,
        b,
        c,
        dirs,
        lookup,
    }
}

fn unread(core: &Core, group: &str) -> u32 {
    core.chats()
        .iter()
        .find(|c| c.contact.id == group)
        .unwrap()
        .unread
}

#[tokio::test(flavor = "multi_thread")]
async fn messages_reach_everyone_and_deliver_once_all_have_them() {
    let t = trio().await;
    let info = t.b.group_info(&t.group).unwrap();
    assert_eq!(info.name, "Weekend");
    assert_eq!(info.members.len(), 3);
    assert!(!info.i_am_admin && info.active);
    // Carol is a group peer for Bob, not a contact.
    let carol_for_bob = t.b.contact(&t.c.my_key()).unwrap();
    assert_eq!(carol_for_bob.state, ContactState::GroupPeer);
    assert_eq!(carol_for_bob.their_name, "Carol");
    assert!(
        !t.b.chats().iter().any(|c| c.contact.id == t.c.my_key()),
        "a group peer has no chat of her own"
    );
    assert!(t.b.chats().iter().any(|c| c.contact.id == t.group));

    // Admin writes.
    let m = t.a.send(&t.group, "hello everyone").unwrap();
    for core in [&t.b, &t.c] {
        until("message arrives", || {
            core.messages(&t.group, None, 10)
                .iter()
                .any(|x| x.id == m.id)
        })
        .await;
    }
    let got = t.c.messages(&t.group, None, 10).pop().unwrap();
    assert_eq!(got.sender.as_deref(), Some(t.a.my_key().as_str()));
    assert!(!got.outgoing && got.attachment.is_none());
    until("delivered to all", || {
        t.a.message(&m.id).unwrap().status == MessageStatus::Delivered
    })
    .await;

    // A member writes; it goes to the admin AND to the other member.
    let m2 = t.b.send(&t.group, "from bob").unwrap();
    for core in [&t.a, &t.c] {
        until("bob's message arrives", || {
            core.messages(&t.group, None, 10)
                .iter()
                .any(|x| x.id == m2.id)
        })
        .await;
    }
    let seen = t.c.message(&m2.id).unwrap();
    assert_eq!(seen.sender.as_deref(), Some(t.b.my_key().as_str()));
    assert_eq!(unread(&t.c, &t.group), 2);
    t.c.mark_read(&t.group);
    assert_eq!(unread(&t.c, &t.group), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_offline_member_catches_up_and_the_message_stays_pending_until_then() {
    let mut t = trio().await;
    let (kc, dir_c) = (t.c.my_key(), t.dirs[2].path().to_path_buf());
    t.c.shutdown();
    until("carol offline for alice", || !t.a.is_online(&kc)).await;
    let m = t.a.send(&t.group, "while you were out").unwrap();
    until("bob has it", || t.b.message(&m.id).is_some()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        t.a.message(&m.id).unwrap().status < MessageStatus::Delivered,
        "not delivered while Carol is away"
    );
    t.c = start(&dir_c, &t.lookup, "Carol");
    until("carol catches up", || t.c.message(&m.id).is_some()).await;
    until("now delivered", || {
        t.a.message(&m.id).unwrap().status == MessageStatus::Delivered
    })
    .await;
    // Her history survived the restart, with the group and its members.
    assert_eq!(t.c.group_info(&t.group).unwrap().members.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn files_go_to_every_member() {
    let t = trio().await;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("notes.txt");
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &data).unwrap();
    let m =
        t.b.send_file(
            &t.group,
            &src,
            "notes.txt",
            "text/plain",
            "for the group",
            None,
        )
        .unwrap();
    for core in [&t.a, &t.c] {
        until("file arrives", || {
            core.message(&m.id)
                .and_then(|x| x.attachment)
                .is_some_and(|a| a.state == TransferState::Done && a.path.is_some())
        })
        .await;
        let a = core.message(&m.id).unwrap().attachment.unwrap();
        assert_eq!(std::fs::read(a.path.unwrap()).unwrap(), data);
    }
    until("sender sees it done", || {
        t.b.message(&m.id)
            .and_then(|x| x.attachment)
            .is_some_and(|a| a.state == TransferState::Done)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_admin_changes_members_and_removed_members_are_cut_off() {
    let t = trio().await;
    // A member cannot add, remove or rename.
    assert!(t.b.group_add(&t.group, &t.c.my_key()).is_err());
    assert!(t.b.group_remove(&t.group, &t.c.my_key()).is_err());
    assert!(t.b.group_rename(&t.group, "Mine now").is_err());

    t.a.group_rename(&t.group, "Weekend trip").unwrap();
    until("rename arrives", || {
        t.b.group_info(&t.group)
            .is_some_and(|g| g.name == "Weekend trip")
            && t.c
                .group_info(&t.group)
                .is_some_and(|g| g.name == "Weekend trip")
    })
    .await;

    // Remove Carol.
    t.a.group_remove(&t.group, &t.c.my_key()).unwrap();
    until("carol is out", || {
        t.c.group_info(&t.group).is_some_and(|g| !g.active)
    })
    .await;
    until("bob sees two members", || {
        t.b.group_info(&t.group)
            .is_some_and(|g| g.members.len() == 2)
    })
    .await;
    assert!(
        t.c.send(&t.group, "still here?").is_err(),
        "removed members cannot send"
    );
    let m = t.a.send(&t.group, "private now").unwrap();
    until("bob gets it", || t.b.message(&m.id).is_some()).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        t.c.message(&m.id).is_none(),
        "Carol must not get new messages"
    );
    until("delivered to the remaining member", || {
        t.a.message(&m.id).unwrap().status == MessageStatus::Delivered
    })
    .await;

    // Add her back.
    t.a.group_add(&t.group, &t.c.my_key()).unwrap();
    until("carol back in", || {
        t.c.group_info(&t.group)
            .is_some_and(|g| g.active && g.members.len() == 3)
    })
    .await;
    let m2 = t.b.send(&t.group, "welcome back").unwrap();
    until("carol gets new messages", || t.c.message(&m2.id).is_some()).await;
    assert!(
        t.c.message(&m.id).is_none(),
        "history from her absence stays hidden"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn leaving_and_closing() {
    let t = trio().await;
    t.b.leave_group(&t.group).unwrap();
    assert!(!t.b.group_info(&t.group).unwrap().active);
    assert!(t.b.send(&t.group, "gone").is_err());
    until("admin drops bob", || {
        t.a.group_info(&t.group)
            .is_some_and(|g| g.members.len() == 2)
    })
    .await;
    until("carol sees two members", || {
        t.c.group_info(&t.group)
            .is_some_and(|g| g.members.len() == 2)
    })
    .await;

    // Ending it is the owner's to do, for everyone.
    t.a.group_close(&t.group).unwrap();
    until("carol sees it closed", || {
        t.c.group_info(&t.group).is_some_and(|g| !g.active)
    })
    .await;
    // History stays until deleted; deleting removes the group and its peers.
    assert!(t.c.chats().iter().any(|c| c.contact.id == t.group));
    t.c.delete_contact(&t.group).unwrap();
    assert!(t.c.contact(&t.group).is_none());
    assert!(
        t.c.contact(&t.b.my_key()).is_none(),
        "group peers are cleaned up"
    );
    assert!(t.c.contact(&t.a.my_key()).is_some(), "contacts stay");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stranger_cannot_add_you_to_a_group() {
    let lookup = MemoryLookup::default();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = start(dirs[0].path(), &lookup, "Alice");
    let b = start(dirs[1].path(), &lookup, "Bob");
    let s = start(dirs[2].path(), &lookup, "Stranger");
    befriend(&a, &b).await;
    // The stranger is Bob's contact only; Alice has never met them.
    befriend(&b, &s).await;
    let g = s.create_group("Spam", &[b.my_key()]).unwrap();
    until("bob gets the group from his contact", || {
        b.group_info(&g.id).is_some()
    })
    .await;
    // Only the admin's own contacts can be added, so Alice cannot be dragged in.
    assert!(s.group_add(&g.id, &a.my_key()).is_err());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(a.group_info(&g.id).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn group_call_with_three_people_join_and_leave() {
    let t = trio().await;
    let (mut eb, mut ec) = (t.b.subscribe(), t.c.subscribe());
    let mut ma = t.a.call_media();
    let mut mb = t.b.call_media();
    let mut mc = t.c.call_media();

    t.a.call_invite(&t.group, false).unwrap();
    for rx in [&mut eb, &mut ec] {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv())
                .await
                .expect("incoming")
                .unwrap();
            if let CoreEvent::Call(CallEvent::Incoming {
                contact_id, from, ..
            }) = ev
            {
                assert_eq!((contact_id, from), (t.group.clone(), t.a.my_key()));
                break;
            }
        }
    }
    assert_eq!(t.b.group_call(&t.group), Some(false));
    t.b.call_answer(true);
    t.c.call_answer(true);

    // Each of the three sends audio; each of the other two must hear it,
    // tagged with who it is from.
    let (ka, kb, kc) = (t.a.my_key(), t.b.my_key(), t.c.my_key());
    let mut heard: HashSet<(char, String)> = HashSet::new();
    let expect = [
        ('a', kb.clone()),
        ('a', kc.clone()),
        ('b', ka.clone()),
        ('b', kc.clone()),
        ('c', ka.clone()),
        ('c', kb.clone()),
    ];
    let drive = async {
        let mut n = 0u32;
        loop {
            n += 1;
            t.a.call_audio(n * 20_000, vec![1; 40]);
            t.b.call_audio(n * 20_000, vec![2; 40]);
            t.c.call_audio(n * 20_000, vec![3; 40]);
            tokio::time::sleep(Duration::from_millis(20)).await;
            for (tag, rx) in [('a', &mut ma), ('b', &mut mb), ('c', &mut mc)] {
                while let Ok(m) = rx.try_recv() {
                    if m.kind == MediaKind::Audio {
                        heard.insert((tag, m.from));
                    }
                }
            }
            if expect.iter().all(|e| heard.contains(e)) {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), drive)
        .await
        .expect("everyone hears everyone");

    // A video frame from Carol reaches both others.
    t.c.call_video(5, true, vec![9; 3000]);
    let mut got = HashSet::new();
    let video = async {
        while got.len() < 2 {
            tokio::select! {
                Some(m) = ma.recv() => if m.kind == MediaKind::Video { assert_eq!(m.from, kc); got.insert('a'); },
                Some(m) = mb.recv() => if m.kind == MediaKind::Video { assert_eq!(m.from, kc); got.insert('b'); },
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), video)
        .await
        .expect("video fans out");

    // Bob leaves: the call goes on for Alice and Carol, and Bob can rejoin.
    t.b.call_end();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(t.a.group_call(&t.group).is_some(), "still going");
    assert!(t.b.group_call(&t.group).is_some(), "joinable for Bob");
    t.b.call_join(&t.group);
    let wait = async {
        loop {
            if let Ok(CoreEvent::Call(CallEvent::Connected { .. })) = eb.recv().await {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), wait)
        .await
        .expect("bob rejoined");

    // Everyone leaves; the call is over and nobody can join it any more.
    t.a.call_end();
    t.b.call_end();
    t.c.call_end();
    until("no call left", || {
        t.a.group_call(&t.group).is_none()
            && t.b.group_call(&t.group).is_none()
            && t.c.group_call(&t.group).is_none()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_leaving_does_not_end_the_group() {
    let t = trio().await;
    let (kb, kc) = (t.b.my_key(), t.c.my_key());
    t.a.group_set_admin(&t.group, &kc, true).unwrap();
    until("carol to be an admin", || {
        t.c.group_info(&t.group).is_some_and(|g| g.i_am_admin)
    })
    .await;
    t.a.leave_group(&t.group).unwrap();
    until("carol to be the owner", || {
        t.c.group_info(&t.group).is_some_and(|g| g.i_am_owner)
    })
    .await;
    until("bob to see two members", || {
        t.b.group_info(&t.group)
            .is_some_and(|g| g.active && g.members.len() == 2)
    })
    .await;
    assert!(!t.a.group_info(&t.group).unwrap().active);
    let m = t.b.send(&t.group, "still here").unwrap();
    until("the group to go on", || t.c.message(&m.id).is_some()).await;

    // The new owner runs it, an old state is not taken back.
    t.c.group_rename(&t.group, "Ours").unwrap();
    until("the new name", || {
        t.b.group_info(&t.group).is_some_and(|g| g.name == "Ours")
    })
    .await;
    assert!(t.b.group_remove(&t.group, &kc).is_err());
    t.c.group_remove(&t.group, &kb).unwrap();
    until("bob to be out", || {
        t.b.group_info(&t.group).is_some_and(|g| !g.active)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn what_a_group_allows_holds() {
    let t = trio().await;
    // Announcements: only admins write, and what others send anyway does not count.
    t.a.group_set_rules(&t.group, |r| r.only_admins_send = true)
        .unwrap();
    until("the rule to reach bob", || {
        t.b.group_info(&t.group)
            .is_some_and(|g| g.rules.only_admins_send)
    })
    .await;
    assert_eq!(
        t.b.send(&t.group, "may I?").unwrap_err().to_string(),
        "only_admins"
    );
    let m = t.a.send(&t.group, "announcement").unwrap();
    until("the announcement", || t.b.message(&m.id).is_some()).await;

    // Slow mode: one message per wait.
    t.a.group_set_rules(&t.group, |r| {
        r.only_admins_send = false;
        r.slow_ms = 60_000;
    })
    .unwrap();
    until("the slow rule to reach bob", || {
        t.b.group_info(&t.group)
            .is_some_and(|g| g.rules.slow_ms == 60_000)
    })
    .await;
    t.b.send(&t.group, "one").unwrap();
    assert_eq!(
        t.b.send(&t.group, "two").unwrap_err().to_string(),
        "slow_mode"
    );

    // Anyone may rename once the group says so.
    assert!(t.b.group_rename(&t.group, "Mine").is_err());
    t.a.group_set_rules(&t.group, |r| r.only_admins_edit = false)
        .unwrap();
    until("the edit rule to reach bob", || {
        t.b.group_info(&t.group)
            .is_some_and(|g| !g.rules.only_admins_edit)
    })
    .await;
    t.b.group_rename(&t.group, "Ours").unwrap();
    until("the name at alice", || {
        t.a.group_info(&t.group).is_some_and(|g| g.name == "Ours")
    })
    .await;
}

/// Bob writes while Carol is away and is gone by the time she is back: Alice passes it on.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_comes_from_any_member_when_its_writer_is_gone() {
    let mut t = trio().await;
    let (kb, kc) = (t.b.my_key(), t.c.my_key());
    t.c.shutdown();
    let m = t.b.send(&t.group, "for when you are back").unwrap();
    until("alice to have it", || t.a.message(&m.id).is_some()).await;
    t.b.shutdown();
    t.c = start(t.dirs[2].path(), &t.lookup, "Carol");
    assert_eq!(t.c.my_key(), kc);
    until("carol to have it, from alice", || {
        t.c.message(&m.id).is_some()
    })
    .await;
    let got = t.c.message(&m.id).unwrap();
    assert_eq!(got.body, "for when you are back");
    assert_eq!(got.sender.as_deref(), Some(kb.as_str()));
}

/// Where only admins write, any member can still pass on what an admin wrote.
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_member_passes_on_what_only_admins_can_write() {
    let mut t = trio().await;
    t.a.group_set_rules(&t.group, |r| r.only_admins_send = true)
        .unwrap();
    until("the rules", || {
        [&t.b, &t.c].iter().all(|core| {
            core.group_info(&t.group)
                .is_some_and(|g| g.rules.only_admins_send)
        })
    })
    .await;
    let kc = t.c.my_key();
    t.c.shutdown();
    let m = t.a.send(&t.group, "Saturday at ten").unwrap();
    until("bob to have it", || t.b.message(&m.id).is_some()).await;
    t.a.shutdown();
    t.c = start(t.dirs[2].path(), &t.lookup, "Carol");
    assert_eq!(t.c.my_key(), kc);
    until("carol to have it, from bob", || {
        t.c.message(&m.id).is_some()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn joining_with_a_link_goes_through_who_runs_it() {
    let t = trio().await;
    let dir = tempfile::tempdir().unwrap();
    let dave = start(dir.path(), &t.lookup, "Dave");
    let link = t.a.group_link(&t.group).unwrap();
    assert!(whatsmine_core::identity::parse_group_link(&link).is_some());

    // A made-up token gets nowhere.
    let mut wrong = whatsmine_core::identity::parse_group_link(&link).unwrap();
    wrong.token = [9; 16];
    dave.join_by_link(&wrong.link(), "").unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(dave.group_info(&t.group).is_none());

    // The real link, with approval on: Alice decides.
    t.a.group_link_approve(&t.group, true).unwrap();
    dave.join_by_link(&link, "Hi, it is Dave").unwrap();
    until("alice to see who asks", || {
        !t.a.group_link_state(&t.group).asking.is_empty()
    })
    .await;
    let asker = t.a.group_link_state(&t.group).asking[0].clone();
    assert_eq!(asker.id, dave.my_key());
    assert_eq!(asker.greeting, "Hi, it is Dave");
    assert!(dave.group_info(&t.group).is_none());
    t.a.group_answer_join(&t.group, &asker.id, true).unwrap();
    until("dave to be in", || {
        dave.group_info(&t.group)
            .is_some_and(|g| g.active && g.members.len() == 4)
    })
    .await;
    until("bob to see dave", || {
        t.b.group_info(&t.group)
            .is_some_and(|g| g.members.len() == 4)
    })
    .await;
    let m = dave.send(&t.group, "hello all").unwrap();
    until("the others to get it", || {
        t.b.message(&m.id).is_some() && t.c.message(&m.id).is_some()
    })
    .await;
    // Alice is no contact of Dave's because of this.
    assert_ne!(
        dave.contact(&t.a.my_key()).map(|c| c.state),
        Some(ContactState::Accepted)
    );
    dave.shutdown();
}
