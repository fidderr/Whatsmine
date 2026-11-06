//! Status between real cores over loopback QUIC.

use std::time::Duration;

use tokio::sync::broadcast;
use whatsmine_core::protocol::{Frame, id_to_bytes};
use whatsmine_core::records::LocalRecords as MemoryLookup;
use whatsmine_core::status::{Audience, Backdrop, LASTS, MOST_FROM_ONE, Post};
use whatsmine_core::{Core, CoreEvent, NetMode, Settings, now_ms};

const WAIT: Duration = Duration::from_secs(30);

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

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..600 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never: {what}");
}

async fn befriend(a: &Core, b: &Core) {
    let mut events: broadcast::Receiver<CoreEvent> = b.subscribe();
    a.add_contact(&b.invite_link(), None).unwrap();
    let wait = async {
        loop {
            if let Ok(CoreEvent::RequestReceived { .. }) = events.recv().await {
                return;
            }
        }
    };
    tokio::time::timeout(WAIT, wait).await.expect("a request");
    b.accept(&a.my_key()).unwrap();
    until("online", || {
        a.is_online(&b.my_key()) && b.is_online(&a.my_key())
    })
    .await;
}

fn from<'a>(posts: &'a [Post], who: &Core) -> Vec<&'a Post> {
    posts.iter().filter(|p| p.author == who.my_key()).collect()
}

/// A marker: whatever was sent before it has been dealt with once this arrives.
async fn after_all(a: &Core, b: &Core) {
    let marker = a.send(&b.my_key(), "after it").unwrap();
    until("what came after", || b.message(&marker.id).is_some()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_post_reaches_who_it_is_for_and_lasts_a_day() {
    let lookup = MemoryLookup::default();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        start(da.path(), &lookup, "Roy"),
        start(db.path(), &lookup, "Noor"),
    );
    befriend(&a, &b).await;
    let id = a.status_post("At the lake", Backdrop::Dusk, None).unwrap();
    until("the post", || !from(&b.status_posts(), &a).is_empty()).await;
    let posts = b.status_posts();
    let post = from(&posts, &a)[0];
    assert_eq!(post.id, id);
    assert_eq!(post.text, "At the lake");
    assert_eq!(post.backdrop, Backdrop::Dusk);
    assert!(!post.seen);
    assert!(post.until <= now_ms() + LASTS);
    // Our own list has it too, and nobody saw it yet.
    let mine = a.status_posts();
    assert_eq!(from(&mine, &a).len(), 1);
    assert!(a.status_seen_by(&id).is_empty());

    // They look: the one who posted hears it.
    b.status_seen(&id);
    until("seen", || a.status_seen_by(&id) == [b.my_key()]).await;
    assert!(from(&b.status_posts(), &a)[0].seen);

    // Taken back: gone on both sides, for good.
    a.status_delete(&id);
    until("gone there", || from(&b.status_posts(), &a).is_empty()).await;
    assert!(from(&a.status_posts(), &a).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn it_waits_for_someone_who_is_away_and_gets_there_when_both_are_around() {
    let lookup = MemoryLookup::default();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(da.path(), &lookup, "Roy");
    let b = start(db.path(), &lookup, "Noor");
    befriend(&a, &b).await;
    let b_key = b.my_key();
    b.shutdown();
    until("away", || !a.is_online(&b_key)).await;
    let id = a
        .status_post("While you were out", Backdrop::Sea, None)
        .unwrap();
    assert!(a.status_owed_to(&b_key) > 0, "still to deliver");
    // Back again.
    let b = start(db.path(), &lookup, "Noor");
    b.set_in_front(true);
    a.set_in_front(true);
    until("the post", || b.status_posts().iter().any(|p| p.id == id)).await;
    // Delivered, and not owed any more.
    until("settled", || a.status_owed_to(&b_key) == 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_who_it_is_for_gets_it() {
    let lookup = MemoryLookup::default();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = start(dirs[0].path(), &lookup, "Roy");
    let b = start(dirs[1].path(), &lookup, "Noor");
    let c = start(dirs[2].path(), &lookup, "Sam");
    befriend(&a, &b).await;
    befriend(&a, &c).await;
    let mut settings = a.settings();
    settings.status_audience = Audience::Only(vec![b.my_key()]);
    a.save_settings(&settings).unwrap();
    let only = a.status_post("For Noor", Backdrop::Sea, None).unwrap();
    until("Noor has it", || {
        b.status_posts().iter().any(|p| p.id == only)
    })
    .await;
    after_all(&a, &c).await;
    assert!(from(&c.status_posts(), &a).is_empty(), "Sam does not");

    let mut settings = a.settings();
    settings.status_audience = Audience::Except(vec![b.my_key()]);
    a.save_settings(&settings).unwrap();
    let except = a.status_post("Not for Noor", Backdrop::Sea, None).unwrap();
    until("Sam has it", || {
        c.status_posts().iter().any(|p| p.id == except)
    })
    .await;
    after_all(&a, &b).await;
    assert!(!b.status_posts().iter().any(|p| p.id == except));
}

#[tokio::test(flavor = "multi_thread")]
async fn it_goes_when_the_time_is_up_on_both_sides() {
    let lookup = MemoryLookup::default();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        start(da.path(), &lookup, "Roy"),
        start(db.path(), &lookup, "Noor"),
    );
    befriend(&a, &b).await;
    let id = a
        .status_post_lasting("Short", Backdrop::Sea, None, 1500)
        .unwrap();
    until("the post", || b.status_posts().iter().any(|p| p.id == id)).await;
    until("gone there", || {
        !b.status_posts().iter().any(|p| p.id == id)
    })
    .await;
    until("gone here", || !a.status_posts().iter().any(|p| p.id == id)).await;
    assert_eq!(a.status_owed_to(&b.my_key()), 0, "nothing left to deliver");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_photo_arrives_whole_and_goes_with_the_post() {
    let lookup = MemoryLookup::default();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        start(da.path(), &lookup, "Roy"),
        start(db.path(), &lookup, "Noor"),
    );
    befriend(&a, &b).await;
    // Colours that do not compress, so it takes more than one part.
    let path = da.path().join("photo.png");
    let mut seed = 77u32;
    let image = image::RgbImage::from_fn(900, 900, |_, _| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        image::Rgb([(seed >> 24) as u8, (seed >> 16) as u8, (seed >> 8) as u8])
    });
    image.save(&path).unwrap();
    // Long enough for the photo to get there on a slow machine first: at 6 s, a CI runner had the
    // post gone, photo and all, before the photo arrived.
    let id = a
        .status_post_lasting("With a photo", Backdrop::Sea, Some(&path), 20_000)
        .unwrap();
    let sent = a.status_photo(&id).expect("the photo here");
    assert!(sent.len() > 64 * 1024, "more than one part: {}", sent.len());
    until("the photo there", || b.status_photo(&id).is_some()).await;
    assert_eq!(b.status_photo(&id).unwrap(), sent);
    until("settled", || a.status_owed_to(&b.my_key()) == 0).await;
    // Whatever was in it is gone with it.
    until("gone", || {
        b.status_photo(&id).is_none() && a.status_photo(&id).is_none()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_is_a_normal_message_that_says_what_it_is_about() {
    let lookup = MemoryLookup::default();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        start(da.path(), &lookup, "Roy"),
        start(db.path(), &lookup, "Noor"),
    );
    befriend(&a, &b).await;
    let id = a.status_post("At the lake", Backdrop::Sea, None).unwrap();
    until("the post", || b.status_posts().iter().any(|p| p.id == id)).await;
    let reply = b.status_reply(&id, "Coming!").unwrap();
    until("the reply", || a.message(&reply.id).is_some()).await;
    let got = a.message(&reply.id).unwrap();
    assert_eq!(got.body, "Coming!");
    assert!(matches!(
        got.extra.rich,
        Some(whatsmine_core::rich::Rich::StatusReply { ref quote, .. }) if quote == "At the lake"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn turned_off_read_receipts_keep_quiet() {
    let lookup = MemoryLookup::default();
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (a, b) = (
        start(da.path(), &lookup, "Roy"),
        start(db.path(), &lookup, "Noor"),
    );
    befriend(&a, &b).await;
    let mut settings = b.settings();
    settings.read_receipts = false;
    b.save_settings(&settings).unwrap();
    let id = a.status_post("At the lake", Backdrop::Sea, None).unwrap();
    until("the post", || b.status_posts().iter().any(|p| p.id == id)).await;
    b.status_seen(&id);
    after_all(&b, &a).await;
    assert!(a.status_seen_by(&id).is_empty());
}

/// What someone who does not follow the rules can send.
#[tokio::test(flavor = "multi_thread")]
async fn made_up_posts_get_nowhere() {
    let lookup = MemoryLookup::default();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = start(dirs[0].path(), &lookup, "Roy");
    let b = start(dirs[1].path(), &lookup, "Noor");
    let c = start(dirs[2].path(), &lookup, "Sam");
    befriend(&a, &b).await;
    befriend(&a, &c).await;
    befriend(&b, &c).await;
    let new_id = || id_to_bytes(&uuid::Uuid::now_v7().to_string()).unwrap();
    let b_key = b.my_key();

    // Too old already, and a photo with more parts than anyone may send.
    let old = new_id();
    a.send_as_is(
        &b_key,
        Frame::StatusPost {
            id: old,
            at: now_ms() - LASTS - 1000,
            lasts: LASTS,
            text: "old".into(),
            backdrop: 0,
            parts: 0,
        },
    );
    a.send_as_is(
        &b_key,
        Frame::StatusPost {
            id: new_id(),
            at: now_ms(),
            lasts: LASTS,
            text: "huge".into(),
            backdrop: 0,
            parts: u16::MAX,
        },
    );
    // Claims to last forever, with a backdrop that is not on the list.
    let forever = new_id();
    a.send_as_is(
        &b_key,
        Frame::StatusPost {
            id: forever,
            at: now_ms() + 100 * LASTS,
            lasts: i64::MAX,
            text: "forever".into(),
            backdrop: 200,
            parts: 0,
        },
    );
    after_all(&a, &b).await;
    let posts = b.status_posts();
    let mine = from(&posts, &a);
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(mine[0].text, "forever");
    assert_eq!(mine[0].backdrop, Backdrop::Sea);
    assert!(mine[0].until <= now_ms() + LASTS, "a day at most");
    let real = mine[0].id.clone();

    // Someone else's post: Sam cannot take it away, add a photo to it, or claim to have seen ours.
    c.send_as_is(&b_key, Frame::StatusGone { id: forever });
    c.send_as_is(
        &b_key,
        Frame::StatusPhoto {
            id: forever,
            part: 0,
            bytes: vec![1, 2, 3],
        },
    );
    after_all(&c, &b).await;
    assert!(b.status_posts().iter().any(|p| p.id == real), "still there");
    assert!(b.status_photo(&real).is_none());
    let mut settings = a.settings();
    settings.status_audience = Audience::Only(vec![b.my_key()]);
    a.save_settings(&settings).unwrap();
    let ours = a.status_post("For Noor", Backdrop::Sea, None).unwrap();
    until("Noor has it", || {
        b.status_posts().iter().any(|p| p.id == ours)
    })
    .await;
    c.send_as_is(
        &a.my_key(),
        Frame::StatusSeen {
            id: id_to_bytes(&ours).unwrap(),
        },
    );
    after_all(&c, &a).await;
    assert!(a.status_seen_by(&ours).is_empty(), "it did not go to Sam");

    // Flooding only pushes out their own oldest.
    for n in 0..MOST_FROM_ONE + 5 {
        a.send_as_is(
            &b_key,
            Frame::StatusPost {
                id: new_id(),
                at: now_ms(),
                lasts: LASTS,
                text: format!("{n}"),
                backdrop: 0,
                parts: 0,
            },
        );
    }
    after_all(&a, &b).await;
    assert_eq!(from(&b.status_posts(), &a).len(), MOST_FROM_ONE);

    // A chat that is hidden: nothing here shows that person exists.
    let mut prefs = b.chat_prefs(&a.my_key());
    prefs.locked = true;
    b.set_chat_prefs(&a.my_key(), &prefs);
    assert!(from(&b.status_posts(), &a).is_empty());
    prefs.locked = false;
    b.set_chat_prefs(&a.my_key(), &prefs);
    assert_eq!(from(&b.status_posts(), &a).len(), MOST_FROM_ONE);

    // Someone who is no contact any more: nothing of theirs stays, and nothing new comes in.
    b.block(&a.my_key()).unwrap();
    assert!(from(&b.status_posts(), &a).is_empty());
}
