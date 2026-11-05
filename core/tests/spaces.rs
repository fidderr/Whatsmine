//! Spaces between real cores: a group of groups and rooms, each with its own way in.

use std::time::Duration;

use whatsmine_core::groups::ChildState;
use whatsmine_core::protocol::ChildKind;
use whatsmine_core::records::LocalRecords as MemoryLookup;
use whatsmine_core::{Core, CoreEvent, NetMode, Settings};

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..800 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never: {what}");
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

async fn befriend(a: &Core, b: &Core) {
    let mut events = b.subscribe();
    a.add_contact(&b.invite_link(), None).unwrap();
    let wait = async {
        loop {
            if let Ok(CoreEvent::RequestReceived { .. }) = events.recv().await {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(40), wait)
        .await
        .expect("a request");
    b.accept(&a.my_key()).unwrap();
    // Being online says little: people in the same space are connected already.
    until("accepted", || {
        a.contact(&b.my_key())
            .is_some_and(|c| c.state == whatsmine_core::ContactState::Accepted)
    })
    .await;
    until("online", || a.is_online(&b.my_key())).await;
}

struct Club {
    a: Core,
    b: Core,
    c: Core,
    space: String,
    _dirs: Vec<tempfile::TempDir>,
}

/// Alice runs a space with Bob and Carol in it.
async fn club() -> Club {
    let lookup = MemoryLookup::default();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = start(dirs[0].path(), &lookup, "Alice");
    let b = start(dirs[1].path(), &lookup, "Bob");
    let c = start(dirs[2].path(), &lookup, "Carol");
    befriend(&a, &b).await;
    befriend(&a, &c).await;
    let space = a
        .create_space("Climbing club", &[b.my_key(), c.my_key()])
        .unwrap()
        .id;
    for one in [&b, &c] {
        until("the space", || {
            one.group_info(&space).is_some_and(|g| g.rules.space)
        })
        .await;
    }
    Club {
        a,
        b,
        c,
        space,
        _dirs: dirs,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn children_show_for_everyone_and_each_has_its_own_way_in() {
    let t = club().await;
    // A group with Bob in it, and a room with Bob in it.
    let trips = t.a.create_group("Trips", &[t.b.my_key()]).unwrap().id;
    let evening = t.a.create_room("Evening chat", &[t.b.my_key()]).unwrap().id;
    t.a.space_add_child(&t.space, &trips).unwrap();
    t.a.space_add_child(&t.space, &evening).unwrap();
    until("Carol sees them", || {
        t.c.space_children(&t.space).len() == 2
    })
    .await;
    until("Bob sees them", || t.b.space_children(&t.space).len() == 2).await;

    let carol_sees = t.c.space_children(&t.space);
    let trips_row = carol_sees.iter().find(|c| c.child.id == trips).unwrap();
    assert_eq!(trips_row.child.name, "Trips");
    assert_eq!(trips_row.child.kind, ChildKind::Group);
    assert_eq!(trips_row.state, ChildState::Out, "she is not in it");
    let room_row = carol_sees.iter().find(|c| c.child.id == evening).unwrap();
    assert_eq!(room_row.child.kind, ChildKind::Room);
    until("Bob is in the group", || t.b.group_info(&trips).is_some()).await;
    until("and Bob's list says so", || {
        t.b.space_children(&t.space)
            .iter()
            .find(|c| c.child.id == trips)
            .is_some_and(|c| c.state == ChildState::In)
    })
    .await;

    // Opening it joins through its own link: this one lets anyone with the link in.
    t.c.space_open_child(&t.space, &trips).unwrap();
    until("Carol is in", || {
        t.c.space_children(&t.space)
            .iter()
            .find(|c| c.child.id == trips)
            .is_some_and(|c| c.state == ChildState::In)
    })
    .await;
    assert!(
        t.c.group_info(&trips)
            .unwrap()
            .members
            .iter()
            .any(|m| m.is_me)
    );
    // A message there reaches her.
    let hello = t.a.send(&trips, "Saturday?").unwrap();
    until("the message", || t.c.message(&hello.id).is_some()).await;

    // Taken out: gone from the list, and Carol stays in what she joined.
    t.a.space_remove_child(&t.space, &trips).unwrap();
    until("gone", || t.c.space_children(&t.space).len() == 1).await;
    assert!(t.c.group_info(&trips).is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_that_wants_to_approve_still_decides() {
    let t = club().await;
    let gear =
        t.a.create_group("Gear for sale", &[t.b.my_key()])
            .unwrap()
            .id;
    t.a.group_link(&gear).unwrap();
    t.a.group_link_approve(&gear, true).unwrap();
    t.a.space_add_child(&t.space, &gear).unwrap();
    until("Carol sees it", || t.c.space_children(&t.space).len() == 1).await;
    t.c.space_open_child(&t.space, &gear).unwrap();
    assert_eq!(
        t.c.space_children(&t.space)[0].state,
        ChildState::Asked,
        "she asked"
    );
    until("the admin hears", || {
        !t.a.group_link_state(&gear).asking.is_empty()
    })
    .await;
    assert!(t.c.group_info(&gear).is_none(), "not in yet");
    t.a.group_answer_join(&gear, &t.c.my_key(), true).unwrap();
    until("let in", || {
        t.c.space_children(&t.space)[0].state == ChildState::In
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_who_runs_it_changes_what_is_in_it() {
    let t = club().await;
    // Bob runs a group of his own, and is a plain member of the space.
    befriend(&t.b, &t.c).await;
    let bobs = t.b.create_group("Bob's", &[t.c.my_key()]).unwrap().id;
    assert!(
        t.b.space_add_child(&t.space, &bobs).is_err(),
        "not his space"
    );
    assert!(t.c.space_children(&t.space).is_empty());
    // Alice cannot add what she does not run: she has no link to it.
    assert!(t.a.space_add_child(&t.space, &bobs).is_err());
    // Made an admin, Bob can.
    t.a.group_set_admin(&t.space, &t.b.my_key(), true).unwrap();
    until("Bob is an admin", || {
        t.b.group_info(&t.space).is_some_and(|g| g.i_am_admin)
    })
    .await;
    t.b.space_add_child(&t.space, &bobs).unwrap();
    until("everyone sees it", || {
        t.a.space_children(&t.space).len() == 1
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_that_goes_somewhere_else_than_it_says_is_not_followed() {
    let t = club().await;
    let shown = t.a.create_group("Shown", &[t.b.my_key()]).unwrap().id;
    let other = t.a.create_group("Other", &[t.b.my_key()]).unwrap().id;
    let wrong = t.a.group_link(&other).unwrap();
    // An admin who writes one group's name with another group's link.
    t.a.group_set_rules(&t.space, |r| {
        r.children.push(whatsmine_core::protocol::SpaceChild {
            id: shown.clone(),
            name: "Shown".into(),
            kind: ChildKind::Group,
            link: wrong.clone(),
        });
    })
    .unwrap();
    until("Carol sees it", || t.c.space_children(&t.space).len() == 1).await;
    assert!(t.c.space_open_child(&t.space, &shown).is_err());
    assert!(t.c.group_info(&other).is_none());
    // A child that is not in the space cannot be opened through it.
    assert!(t.c.space_open_child(&t.space, &other).is_err());
    // And what is not a link either.
    t.a.group_set_rules(&t.space, |r| {
        r.children[0].link = "javascript:alert(1)".into();
    })
    .unwrap();
    until("the new link", || {
        t.c.space_children(&t.space)[0]
            .child
            .link
            .starts_with("javascript")
    })
    .await;
    assert!(t.c.space_open_child(&t.space, &shown).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn not_more_than_a_list_can_hold() {
    let t = club().await;
    let mut last = Ok(());
    let mut added = 0;
    for n in 0..whatsmine_core::protocol::MOST_CHILDREN + 1 {
        let id =
            t.a.create_group(&format!("G{n}"), &[t.b.my_key()])
                .unwrap()
                .id;
        last = t.a.space_add_child(&t.space, &id);
        if last.is_ok() {
            added += 1;
        }
    }
    assert!(last.is_err());
    assert_eq!(added, whatsmine_core::protocol::MOST_CHILDREN);
    until("it all arrives", || {
        t.b.space_children(&t.space).len() == whatsmine_core::protocol::MOST_CHILDREN
    })
    .await;
}
