//! Drop-in rooms between real cores over loopback QUIC: a group whose call is just there, for whoever
//! wants to come in. Nobody rings.

use std::collections::HashSet;
use std::time::Duration;

use tokio::sync::broadcast::Receiver;
use whatsmine_core::calls::{CallEvent, MediaKind, ROOM_MOST, room_call_id};
use whatsmine_core::protocol::{Frame, id_to_bytes};
use whatsmine_core::records::LocalRecords as MemoryLookup;
use whatsmine_core::{Core, CoreEvent, NetMode, Settings};

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..1200 {
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
    until("online", || a.is_online(&b.my_key())).await;
}

struct Room {
    people: Vec<Core>,
    id: String,
    _dirs: Vec<tempfile::TempDir>,
    lookup: MemoryLookup,
}

async fn room_of(count: usize) -> Room {
    let lookup = MemoryLookup::default();
    let dirs: Vec<_> = (0..count).map(|_| tempfile::tempdir().unwrap()).collect();
    let people: Vec<Core> = dirs
        .iter()
        .enumerate()
        .map(|(n, dir)| start(dir.path(), &lookup, &format!("Person {n}")))
        .collect();
    for other in &people[1..] {
        befriend(&people[0], other).await;
    }
    let members: Vec<String> = people[1..].iter().map(Core::my_key).collect();
    let room = people[0].create_room("Evening chat", &members).unwrap();
    for one in &people {
        until("the room", || {
            one.group_info(&room.id).is_some_and(|g| g.rules.room)
        })
        .await;
    }
    Room {
        id: room.id,
        people,
        _dirs: dirs,
        lookup,
    }
}

async fn all_connected(people: &[Core]) {
    for a in people {
        for b in people {
            if a.my_key() != b.my_key() {
                until("they find each other", || a.is_online(&b.my_key())).await;
            }
        }
    }
}

fn nobody_rang(events: &mut Receiver<CoreEvent>) {
    while let Ok(event) = events.try_recv() {
        assert!(
            !matches!(event, CoreEvent::Call(CallEvent::Incoming { .. })),
            "a room does not ring"
        );
    }
}

fn in_room(core: &Core, id: &str) -> HashSet<String> {
    core.group_call_people(id).into_iter().collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn coming_in_rings_nobody_and_everyone_can_see_who_is_in() {
    let room = room_of(3).await;
    all_connected(&room.people).await;
    let [a, b, c] = &room.people[..] else {
        panic!()
    };
    let (mut eb, mut ec) = (b.subscribe(), c.subscribe());
    a.call_join(&room.id);
    until("a is in", || a.call_snapshot().is_some()).await;
    // Others see it, and are not in it.
    until("b sees a", || in_room(b, &room.id).contains(&a.my_key())).await;
    until("c sees a", || in_room(c, &room.id).contains(&a.my_key())).await;
    assert!(b.call_snapshot().is_none(), "nobody is pulled in");
    nobody_rang(&mut eb);
    nobody_rang(&mut ec);

    // Alone in a room is fine: it stays on.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(a.call_snapshot().is_some());

    // Someone comes in: they find each other, and the same call.
    b.call_join(&room.id);
    until("a has b", || {
        a.call_snapshot()
            .is_some_and(|s| s.peers.contains(&b.my_key()))
    })
    .await;
    until("b has a", || {
        b.call_snapshot()
            .is_some_and(|s| s.peers.contains(&a.my_key()))
    })
    .await;
    assert_eq!(
        a.call_snapshot().unwrap().call_id,
        b.call_snapshot().unwrap().call_id
    );
    until("c sees both", || in_room(c, &room.id).len() == 2).await;

    // The one who was there first goes, and the room goes on.
    a.call_end();
    until("a is out", || a.call_snapshot().is_none()).await;
    until("b is on its own", || {
        b.call_snapshot().is_some_and(|s| s.peers.is_empty())
    })
    .await;
    assert!(b.call_snapshot().is_some(), "the room is still on");
    until("c sees one", || {
        in_room(c, &room.id) == HashSet::from([b.my_key()])
    })
    .await;

    // The last one goes: over, for everyone, and nothing in the list of calls.
    b.call_end();
    until("empty", || in_room(c, &room.id).is_empty()).await;
    until("no call shown", || c.group_call(&room.id).is_none()).await;
    assert!(a.call_records().is_empty() && b.call_records().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn two_coming_in_at_once_end_up_in_the_same_room() {
    let room = room_of(2).await;
    all_connected(&room.people).await;
    let [a, b] = &room.people[..] else { panic!() };
    a.call_join(&room.id);
    b.call_join(&room.id);
    until("a has b", || {
        a.call_snapshot()
            .is_some_and(|s| s.peers.contains(&b.my_key()))
    })
    .await;
    until("b has a", || {
        b.call_snapshot()
            .is_some_and(|s| s.peers.contains(&a.my_key()))
    })
    .await;
    let expected = whatsmine_core::protocol::id_from_bytes(&room_call_id(&room.id));
    assert_eq!(a.call_snapshot().unwrap().call_id, expected);
    assert_eq!(b.call_snapshot().unwrap().call_id, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn someone_who_comes_online_hears_the_room_is_on() {
    let room = room_of(2).await;
    let [a, b] = &room.people[..] else { panic!() };
    let (b_key, b_dir) = (b.my_key(), room._dirs[1].path().to_path_buf());
    until("online", || a.is_online(&b_key)).await;
    b.shutdown();
    until("away", || !a.is_online(&b_key)).await;
    a.call_join(&room.id);
    until("a is in", || a.call_snapshot().is_some()).await;
    let b = start(&b_dir, &room.lookup, "Person 1");
    b.set_in_front(true);
    a.set_in_front(true);
    until("b sees a", || in_room(&b, &room.id).contains(&a.my_key())).await;
    assert!(b.call_snapshot().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hand_goes_up_and_down_and_whoever_comes_later_sees_it() {
    let room = room_of(3).await;
    all_connected(&room.people).await;
    let [a, b, c] = &room.people[..] else {
        panic!()
    };
    let mut eb = b.subscribe();
    let mut ec = c.subscribe();
    a.call_join(&room.id);
    b.call_join(&room.id);
    until("a has b", || {
        a.call_snapshot()
            .is_some_and(|s| s.peers.contains(&b.my_key()))
    })
    .await;
    a.call_hand(true);
    wait_for_hand(&mut eb, &a.my_key(), true).await;
    c.call_join(&room.id);
    wait_for_hand(&mut ec, &a.my_key(), true).await;
    a.call_hand(false);
    wait_for_hand(&mut eb, &a.my_key(), false).await;
}

async fn wait_for_hand(events: &mut Receiver<CoreEvent>, who: &str, up: bool) {
    let wait = async {
        loop {
            if let Ok(CoreEvent::Call(CallEvent::PeerHand { from, up: u, .. })) =
                events.recv().await
                && from == who
                && u == up
            {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("the hand");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_made_up_call_or_someone_outside_gets_nowhere() {
    let room = room_of(2).await;
    all_connected(&room.people).await;
    let [a, b] = &room.people[..] else { panic!() };
    // A call with another id than the room's own: nobody is told of it, or rung.
    let mut eb = b.subscribe();
    a.send_as_is(
        &b.my_key(),
        Frame::CallInvite {
            call: id_to_bytes(&uuid::Uuid::now_v7().to_string()).unwrap(),
            video: false,
            group: Some(room.id.clone()),
        },
    );
    let marker = a.send(&b.my_key(), "after it").unwrap();
    until("what came after", || b.message(&marker.id).is_some()).await;
    nobody_rang(&mut eb);
    assert!(in_room(b, &room.id).is_empty());
    assert!(b.group_call(&room.id).is_none());
}

/// Everyone connects to everyone, so this is the number the app promises: all of them hear all of
/// them. Measured here on one PC: a real connection is reported apart.
#[tokio::test(flavor = "multi_thread")]
async fn the_most_that_may_talk_all_hear_each_other_and_one_more_stays_out() {
    let room = room_of(ROOM_MOST + 1).await;
    let people = &room.people[..ROOM_MOST];
    let late = &room.people[ROOM_MOST];
    all_connected(&room.people).await;
    let mut media: Vec<_> = people.iter().map(Core::call_media).collect();
    for one in people {
        one.call_join(&room.id);
    }
    for one in people {
        until("everyone in", || {
            one.call_snapshot()
                .is_some_and(|s| s.peers.len() == ROOM_MOST - 1)
        })
        .await;
    }
    let keys: Vec<String> = people.iter().map(Core::my_key).collect();
    let mut heard: Vec<HashSet<String>> = vec![HashSet::new(); ROOM_MOST];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        for one in people {
            one.call_audio(0, vec![7; 60]);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        for (n, rx) in media.iter_mut().enumerate() {
            while let Ok(m) = rx.try_recv() {
                if m.kind == MediaKind::Audio {
                    heard[n].insert(m.from);
                }
            }
        }
        if heard.iter().all(|h| h.len() == ROOM_MOST - 1) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "not everyone was heard: {:?}",
            heard.iter().map(HashSet::len).collect::<Vec<_>>()
        );
    }
    for (n, key) in keys.iter().enumerate() {
        assert!(!heard[n].contains(key), "nobody hears themselves");
    }

    // Full: someone else stays out, and hears why.
    until("the late one knows", || {
        in_room(late, &room.id).len() == ROOM_MOST
    })
    .await;
    let mut events = late.subscribe();
    late.call_join(&room.id);
    let wait = async {
        loop {
            if let Ok(CoreEvent::Call(CallEvent::RoomFull { .. })) = events.recv().await {
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .expect("told it is full");
    assert!(late.call_snapshot().is_none());
}
