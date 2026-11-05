//! Channels between real cores over loopback, with the DHT played by something in this process.

use std::time::Duration;

use whatsmine_core::channel;
use whatsmine_core::records::{LocalRecords as MemoryLookup, Records, SignedRecord};
use whatsmine_core::{Core, NetMode, Settings};

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
    core.set_in_front(true);
    core
}

struct World {
    lookup: MemoryLookup,
    dirs: Vec<tempfile::TempDir>,
}

impl World {
    fn new() -> Self {
        Self {
            lookup: MemoryLookup::default(),
            dirs: Vec::new(),
        }
    }

    fn person(&mut self, name: &str) -> Core {
        let dir = tempfile::tempdir().unwrap();
        let core = start(dir.path(), &self.lookup, name);
        self.dirs.push(dir);
        core
    }
}

fn texts(core: &Core, id: &str) -> Vec<String> {
    core.channel_posts(id).into_iter().map(|p| p.text).collect()
}

/// A picture that does not compress.
fn a_photo(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("photo.png");
    let mut seed = 99u32;
    let image = image::RgbImage::from_fn(600, 600, |_, _| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        image::Rgb([(seed >> 24) as u8, (seed >> 16) as u8, (seed >> 8) as u8])
    });
    image.save(&path).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_post_reaches_followers_without_any_connection_between_them() {
    let mut w = World::new();
    let (author, follower) = (w.person("Roy"), w.person("Noor"));
    let c = author.channel_create("Hill climbing news").unwrap();
    assert!(c.runs_it());
    let number = author
        .channel_write(&c.id, "Saturday at ten", None)
        .unwrap();
    assert_eq!(number, 1);

    let followed = follower.channel_follow(&c.link()).unwrap();
    assert_eq!(followed.id, c.id);
    assert_eq!(followed.name, "Hill climbing news");
    assert!(!followed.runs_it());
    until("the post", || {
        texts(&follower, &c.id) == ["Saturday at ten"]
    })
    .await;
    // They are strangers: nothing connected them, and neither knows the other's key.
    assert!(follower.contact(&author.my_key()).is_none());
    assert!(author.contact(&follower.my_key()).is_none());
    assert!(!follower.is_online(&author.my_key()));

    // More later, and what was missed in between arrives in order.
    author.channel_write(&c.id, "Second", None).unwrap();
    author.channel_write(&c.id, "Third", None).unwrap();
    until("all of them", || {
        texts(&follower, &c.id) == ["Saturday at ten", "Second", "Third"]
    })
    .await;

    // Followers cannot write: there is nothing to sign with.
    assert!(follower.channel_write(&c.id, "Mine now", None).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn following_again_or_a_bad_link_changes_nothing() {
    let mut w = World::new();
    let (author, follower) = (w.person("Roy"), w.person("Noor"));
    let c = author.channel_create("News").unwrap();
    author.channel_write(&c.id, "One", None).unwrap();
    assert!(author.channel_follow(&c.link()).is_err(), "it is ours");
    assert!(follower.channel_follow("https://x/channel#aaaa").is_err());
    let f = follower.channel_follow(&c.link()).unwrap();
    assert_eq!(
        follower.channel_follow(&c.link()).unwrap().follower,
        f.follower
    );
    assert_eq!(follower.channels().len(), 1);
    until("the post", || texts(&follower, &c.id).len() == 1).await;
    // Leaving: gone from here, and not followed any more.
    follower.channel_leave(&c.id);
    assert!(follower.channels().is_empty());
    assert!(follower.channel_posts(&c.id).is_empty());
    author.channel_write(&c.id, "Two", None).unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(follower.channel_posts(&c.id).is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn what_is_not_from_the_channel_is_never_shown() {
    let mut w = World::new();
    let (author, follower) = (w.person("Roy"), w.person("Noor"));
    let c = author.channel_create("News").unwrap();
    let f = follower.channel_follow(&c.link()).unwrap();
    author.channel_write(&c.id, "Real", None).unwrap();
    until("the post", || texts(&follower, &c.id) == ["Real"]).await;

    // Someone with the link makes up a post. They have the read key, and any signing key but this one.
    let records = Records::Local(w.lookup.clone());
    let fake = channel::Post {
        number: 2,
        at: 1,
        text: "Send money here".into(),
        more: None,
    };
    let sealed = channel::seal_post(&[7; 32], &f.read, &fake).unwrap();
    // Under the channel's own place in the DHT, as if it were real: signed with a wrong key.
    let wrong = SignedRecord {
        key: f.key,
        value: sealed,
        seq: 2,
        signature: [0; 64],
        salt: None,
    };
    records.put_signed(&wrong).await;
    let mut salted = wrong.clone();
    salted.salt = Some(channel::post_salt(2).to_vec());
    records.put_signed(&salted).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(texts(&follower, &c.id), ["Real"]);

    // And when the real one comes, that is what counts.
    author.channel_write(&c.id, "Also real", None).unwrap();
    until("the next", || texts(&follower, &c.id).len() == 2).await;
    assert_eq!(texts(&follower, &c.id), ["Real", "Also real"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn followers_keep_it_alive_when_the_author_is_gone() {
    let mut w = World::new();
    let (author, one) = (w.person("Roy"), w.person("Noor"));
    let c = author.channel_create("News").unwrap();
    author
        .channel_write(&c.id, "Before going away", None)
        .unwrap();
    one.channel_follow(&c.link()).unwrap();
    until("the first follower", || texts(&one, &c.id).len() == 1).await;
    author.shutdown();
    // The DHT forgets after a while, unless someone writes it again. The author cannot.
    w.lookup.forget_everything();
    let later = w.person("Sam");
    later.channel_follow(&c.link()).unwrap();
    until("someone who came later", || {
        texts(&later, &c.id) == ["Before going away"]
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_switch_off_a_follower_passes_nothing_on() {
    let mut w = World::new();
    let (author, one) = (w.person("Roy"), w.person("Noor"));
    let mut settings = one.settings();
    settings.channel_pass_on = false;
    one.save_settings(&settings).unwrap();
    let c = author.channel_create("News").unwrap();
    author
        .channel_write(&c.id, "Before going away", None)
        .unwrap();
    one.channel_follow(&c.link()).unwrap();
    until("the follower", || texts(&one, &c.id).len() == 1).await;
    author.shutdown();
    w.lookup.forget_everything();
    let later = w.person("Sam");
    later.channel_follow(&c.link()).unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(texts(&later, &c.id).is_empty());
    assert_eq!(
        texts(&one, &c.id),
        ["Before going away"],
        "still has it itself"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_post_comes_from_the_author_and_then_from_other_followers() {
    let mut w = World::new();
    let (author, one) = (w.person("Roy"), w.person("Noor"));
    let photo_dir = tempfile::tempdir().unwrap();
    let photo = a_photo(photo_dir.path());
    let c = author.channel_create("News").unwrap();
    let long = "The route to the north face opens again. ".repeat(60);
    let number = author.channel_write(&c.id, &long, Some(&photo)).unwrap();
    let sent = author.channel_photo(&c.id, number).expect("the photo");
    assert!(sent.len() > 1000);
    let posts = author.channel_posts(&c.id);
    assert_eq!(posts[0].text, long.trim(), "the author has all of it");
    assert!(posts[0].has_body);

    one.channel_follow(&c.link()).unwrap();
    // The short part comes first, without any connection.
    until("the short part", || !texts(&one, &c.id).is_empty()).await;
    until("the rest", || {
        one.channel_posts(&c.id).first().is_some_and(|p| p.has_body)
    })
    .await;
    assert_eq!(one.channel_posts(&c.id)[0].text, long.trim());
    assert_eq!(one.channel_photo(&c.id, number).unwrap(), sent);

    // The author goes. Someone who comes later gets all of it from a follower.
    author.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let later = w.person("Sam");
    later.channel_follow(&c.link()).unwrap();
    until("from another follower", || {
        later
            .channel_posts(&c.id)
            .first()
            .is_some_and(|p| p.has_body)
    })
    .await;
    assert_eq!(later.channel_photo(&c.id, number).unwrap(), sent);

    // None of this used anyone's own keys, and every time a new one.
    let keys = one.channel_swarm_keys();
    assert!(!keys.is_empty());
    let mine = one.my_key();
    for (id, key) in &keys {
        assert_eq!(*id, c.id);
        let text =
            whatsmine_core::identity::key_to_string(&iroh::PublicKey::from_bytes(key).unwrap());
        assert_ne!(text, mine, "not who someone is");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_is_there_for_a_long_post_then_its_short_part_still_is() {
    let mut w = World::new();
    let (author, one) = (w.person("Roy"), w.person("Noor"));
    let mut settings = author.settings();
    settings.channel_pass_on = false;
    author.save_settings(&settings).unwrap();
    let c = author.channel_create("News").unwrap();
    let long = "x".repeat(channel::MAX_SHORT + 500);
    author.channel_write(&c.id, &long, None).unwrap();
    // It went out before the author left.
    let records = Records::Local(w.lookup.clone());
    let mut there = false;
    for _ in 0..200 {
        there = records
            .get_signed(&c.key, Some(&channel::post_salt(1)))
            .await
            .is_some();
        if there {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(there, "never written");
    author.shutdown();
    tokio::time::sleep(Duration::from_millis(200)).await;
    one.channel_follow(&c.link()).unwrap();
    until("the short part", || !texts(&one, &c.id).is_empty()).await;
    let posts = one.channel_posts(&c.id);
    assert!(posts[0].text.len() <= channel::MAX_SHORT);
    assert!(!posts[0].has_body);
    assert!(posts[0].more.is_some(), "and it says there is more");
}

#[tokio::test(flavor = "multi_thread")]
async fn each_channel_and_each_time_has_a_key_of_its_own() {
    let mut w = World::new();
    let (author, one) = (w.person("Roy"), w.person("Noor"));
    let a = author.channel_create("A").unwrap();
    let b = author.channel_create("B").unwrap();
    author
        .channel_write(&a.id, &"a".repeat(channel::MAX_SHORT + 1), None)
        .unwrap();
    author
        .channel_write(&b.id, &"b".repeat(channel::MAX_SHORT + 1), None)
        .unwrap();
    one.channel_follow(&a.link()).unwrap();
    one.channel_follow(&b.link()).unwrap();
    until("both", || {
        one.channel_posts(&a.id).first().is_some_and(|p| p.has_body)
            && one.channel_posts(&b.id).first().is_some_and(|p| p.has_body)
    })
    .await;
    let keys = one.channel_swarm_keys();
    let distinct: std::collections::HashSet<_> = keys.iter().map(|(_, k)| *k).collect();
    assert_eq!(distinct.len(), keys.len(), "never the same key twice");
    assert!(keys.iter().any(|(id, _)| *id == a.id));
    assert!(keys.iter().any(|(id, _)| *id == b.id));
}

#[tokio::test(flavor = "multi_thread")]
async fn counts_are_about_right_and_reactions_add_up() {
    let mut w = World::new();
    let author = w.person("Roy");
    let followers: Vec<Core> = (0..5).map(|n| w.person(&format!("F{n}"))).collect();
    let c = author.channel_create("News").unwrap();
    let number = author.channel_write(&c.id, "Who is there?", None).unwrap();
    for f in &followers {
        f.channel_follow(&c.link()).unwrap();
    }
    for f in &followers {
        until("the post", || texts(f, &c.id).len() == 1).await;
    }
    until("about five", || {
        author
            .channel(&c.id)
            .is_some_and(|c| (3..=8).contains(&c.followers))
    })
    .await;
    for f in &followers[..3] {
        f.channel_react(&c.id, number, 1);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    until("the hearts", || {
        author.channel_posts(&c.id)[0]
            .reactions
            .get(1)
            .is_some_and(|n| (2..=4).contains(n))
    })
    .await;
    assert!(followers[0].channel_posts(&c.id)[0].mine.contains(&1));
    // Who reacted is known to nobody: there is nothing in there that says who.
    assert_eq!(author.channel_posts(&c.id)[0].reactions[0], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn someone_new_starts_with_the_newest_even_if_the_old_ones_are_long_gone() {
    let mut w = World::new();
    let author = w.person("Roy");
    let c = author.channel_create("News").unwrap();
    for n in 0..30 {
        author
            .channel_write(&c.id, &format!("Post {n}"), None)
            .unwrap();
    }
    // The DHT has forgotten, and only the newest are written again.
    w.lookup.forget_everything();
    let later = w.person("Sam");
    later.channel_follow(&c.link()).unwrap();
    until("the newest", || {
        later
            .channel_posts(&c.id)
            .iter()
            .any(|p| p.text == "Post 29")
    })
    .await;
    // It knows where it is, whatever it cannot get any more.
    until("up to date", || later.channel(&c.id).unwrap().last == 30).await;
    author.channel_write(&c.id, "The next one", None).unwrap();
    until("the next", || {
        later.channel_posts(&c.id).iter().any(|p| p.number == 31)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_that_failed_loses_nobodys_reaction() {
    let mut w = World::new();
    let author = w.person("Roy");
    let (a, b) = (w.person("Noor"), w.person("Sam"));
    let c = author.channel_create("News").unwrap();
    let number = author.channel_write(&c.id, "Who is there?", None).unwrap();
    a.channel_follow(&c.link()).unwrap();
    b.channel_follow(&c.link()).unwrap();
    until("Noor has it", || !a.channel_posts(&c.id).is_empty()).await;
    until("Sam has it", || !b.channel_posts(&c.id).is_empty()).await;
    a.channel_react(&c.id, number, 0);
    until("Sam sees it", || {
        b.channel_posts(&c.id)[0].reactions[0] == 1
    })
    .await;
    // All of it gone from the DHT, as when a read fails or it was written over.
    w.lookup.forget_everything();
    b.channel_react(&c.id, number, 0);
    until("both count", || {
        author.channel_posts(&c.id)[0].reactions[0] == 2
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_posts_at_once_get_a_number_each() {
    let mut w = World::new();
    let author = w.person("Roy");
    let c = author.channel_create("News").unwrap();
    let mut writers = Vec::new();
    for n in 0..8 {
        let (core, id) = (author.clone(), c.id.clone());
        writers.push(std::thread::spawn(move || {
            core.channel_write(&id, &format!("Post {n}"), None).unwrap()
        }));
    }
    let mut numbers: Vec<u64> = writers.into_iter().map(|t| t.join().unwrap()).collect();
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=8).collect::<Vec<u64>>());
    assert_eq!(author.channel_posts(&c.id).len(), 8);
}

/// As many followers as this proves, and the number the app may claim.
#[tokio::test(flavor = "multi_thread")]
async fn twenty_followers_all_get_it_with_the_author_away_for_the_rest() {
    const FOLLOWERS: usize = 20;
    let mut w = World::new();
    let author = w.person("Roy");
    let c = author.channel_create("News").unwrap();
    let short = author.channel_write(&c.id, "Short", None).unwrap();
    let long = "y".repeat(channel::MAX_SHORT * 3);
    let long_number = author.channel_write(&c.id, &long, None).unwrap();
    let first = w.person("First");
    first.channel_follow(&c.link()).unwrap();
    until("the first has it all", || {
        first
            .channel_posts(&c.id)
            .iter()
            .any(|p| p.number == long_number && p.has_body)
    })
    .await;
    author.shutdown();
    let followers: Vec<Core> = (0..FOLLOWERS - 1)
        .map(|n| w.person(&format!("F{n}")))
        .collect();
    let started = std::time::Instant::now();
    for f in &followers {
        f.channel_follow(&c.link()).unwrap();
    }
    for f in &followers {
        until("all of it", || {
            let posts = f.channel_posts(&c.id);
            posts.iter().any(|p| p.number == short)
                && posts.iter().any(|p| p.number == long_number && p.has_body)
        })
        .await;
    }
    println!(
        "{FOLLOWERS} followers had everything {} ms after following, on one PC",
        started.elapsed().as_millis()
    );
}
