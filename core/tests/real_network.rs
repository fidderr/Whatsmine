//! Two cores over the real BitTorrent DHT and the real internet: not run by default. It proves
//! the whole way in: a link, a sealed request, an accept, a location lookup, a connection.
//!
//!   cargo test -p whatsmine-core --test real_network -- --ignored --nocapture

use std::time::{Duration, Instant};

use whatsmine_core::{ContactState, Core, MessageStatus, NetMode, Settings};

async fn until(what: &str, limit: u64, mut cond: impl FnMut() -> bool) {
    let started = Instant::now();
    let fut = async {
        while !cond() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(limit), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    println!("{what}: {:.1} s", started.elapsed().as_secs_f32());
}

fn start(dir: &std::path::Path, name: &str) -> Core {
    let core = Core::start(dir, NetMode::Internet).unwrap();
    core.save_settings(&Settings {
        display_name: name.into(),
        onboarded: true,
        ..Settings::default()
    })
    .unwrap();
    // On screen: requests are looked for every minute at most.
    core.set_in_front(true);
    core
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the real internet"]
async fn add_and_talk_over_the_real_dht() {
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start(dir_a.path(), "Alice");
    let b = start(dir_b.path(), "Bob");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let link = b.invite_link();
    println!("link: {} characters", link.len());
    a.add_contact_with_greeting(&link, None, "Hi Bob").unwrap();
    until("Bob sees the request", 180, || !b.requests().is_empty()).await;
    assert_eq!(b.contact_keys(&a.my_key()).greeting, "Hi Bob");

    b.accept(&a.my_key()).unwrap();
    until("Alice hears the accept", 120, || {
        a.contact(&b.my_key())
            .is_some_and(|c| c.state == ContactState::Accepted)
    })
    .await;

    let m = a.send(&b.my_key(), "hello over the internet").unwrap();
    until("the first message arrives", 60, || {
        b.messages(&a.my_key(), None, 10)
            .iter()
            .any(|x| x.id == m.id)
    })
    .await;
    until("Alice sees it delivered", 60, || {
        a.message(&m.id)
            .is_some_and(|x| x.status >= MessageStatus::Delivered)
    })
    .await;
    let report = a.connection_report().await.unwrap();
    println!("{report:?}");

    // A reset, and they still talk.
    let started = Instant::now();
    b.reset_public_address();
    let again = a.send(&b.my_key(), "after the reset").unwrap();
    until("a message after a reset", 180, || {
        b.messages(&a.my_key(), None, 10)
            .iter()
            .any(|x| x.id == again.id)
    })
    .await;
    println!(
        "reset to delivered: {:.1} s",
        started.elapsed().as_secs_f32()
    );
    a.shutdown();
    b.shutdown();
}

/// A channel over the real DHT, and a long post from the author over the real internet. With
/// `only_relays` the follower's address stays hidden. Times are printed, to report.
async fn channel_run(only_relays: bool) {
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let author = start(da.path(), "Roy");
    let follower = start(db.path(), "Noor");
    let mut settings = follower.settings();
    settings.channel_relays_only = only_relays;
    follower.save_settings(&settings).unwrap();
    for core in [&author, &follower] {
        core.set_in_front(true);
    }
    let c = author.channel_create("A real channel").unwrap();
    let started = Instant::now();
    author
        .channel_write(&c.id, "Short, by itself", None)
        .unwrap();
    let long = "A longer story that does not fit in a record. ".repeat(40);
    author.channel_write(&c.id, &long, None).unwrap();
    // Someone looking from the outside: when can it be read at all?
    let outside = whatsmine_core::records::Records::join().unwrap();
    let (key, salt) = (c.key, whatsmine_core::channel::post_salt(1));
    let seen = Instant::now();
    loop {
        let head = outside.get_signed(&key, None).await.is_some();
        let post = outside.get_signed(&key, Some(&salt)).await.is_some();
        println!(
            "{} ms: newest {head}, post one {post}",
            seen.elapsed().as_millis()
        );
        if (head && post) || seen.elapsed() > Duration::from_secs(150) {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    let _ = follower.channel_follow(&c.link()).unwrap();
    let followed = Instant::now();
    until("the short post", 180, || {
        follower.channel_posts(&c.id).iter().any(|p| p.number == 1)
    })
    .await;
    println!(
        "short post {} ms after following, {} ms after writing",
        followed.elapsed().as_millis(),
        started.elapsed().as_millis()
    );
    until("the long post", 240, || {
        follower
            .channel_posts(&c.id)
            .iter()
            .any(|p| p.number == 2 && p.has_body)
    })
    .await;
    println!(
        "long post {} ms after following (only relays: {only_relays})",
        followed.elapsed().as_millis()
    );
    let got = follower.channel_posts(&c.id);
    assert_eq!(
        got.iter().find(|p| p.number == 2).unwrap().text,
        long.trim()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the real internet"]
async fn a_channel_over_the_real_dht() {
    channel_run(false).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "uses the real internet"]
async fn a_channel_only_through_relays() {
    channel_run(true).await;
}
