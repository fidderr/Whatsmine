//! Channels: one to many, by a private link, with nobody running anything.
//!
//! A channel has a signing key, kept by whoever runs it, and a read key, which is in the link. So
//! whoever has the link can read, and only who runs it can write: followers never learn anyone's
//! person key, and cannot fake a post.
//!
//! Short posts live in the DHT, each in a record signed by the channel. Followers read them at their
//! normal checks: no connection, and no internet address shown to anyone. Followers also write those
//! signed records again, which they can do without being able to change them, so a post stays around
//! while the author is away.
//!
//! Longer posts and photos come from other followers, see `engine/channel.rs`. That does take a
//! connection, and it shows an internet address to the few followers at the other end, which is said
//! once before someone follows, with a switch to go only through relays.
//!
//! Honest limits: anyone with the link can read, and removing someone takes a new link. If nobody
//! who has a long post is around, only its short part is there. The numbers are estimates, and
//! anyone with the link can disturb them.

use iroh::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use crate::model::Millis;

/// What fits in the text of one record, in bytes. A record holds at most `records::MAX_RECORD`, and
/// the rest of it goes to the sealing, the signature and the post's own fields.
pub const MAX_SHORT: usize = 700;
/// The most text in a post, in characters. What does not fit in a record comes from other followers.
pub const MAX_TEXT: usize = 8_000;
/// The most a post's body can be: its text and photo, sealed.
pub const MAX_BODY: usize = 3 * 1024 * 1024;
pub const MAX_NAME: usize = 64;
/// How many slots there are to leave an address in, per day.
pub const SLOTS: u8 = 8;
pub const KEEP_DAYS: u32 = 30;
/// The reactions on a post.
pub const EMOJI: [&str; 5] = ["👍", "❤️", "😂", "😮", "😢"];

const POST_CONTEXT: &[u8] = b"whatsmine-channel-post-v1";
const BODY_CONTEXT: &[u8] = b"whatsmine-channel-body-v1";
const NOTE_CONTEXT: &[u8] = b"whatsmine-channel-note-v1";
const SIGN_DOMAIN: &[u8] = b"whatsmine-channel-sign-v1";

pub const CHANNEL_BASE: &str = "https://fidderr.github.io/whatsmine/channel#";

/// A channel as this device knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Its public key, as text.
    pub id: String,
    pub key: [u8; 32],
    pub read: [u8; 32],
    pub name: String,
    /// Only for who runs it.
    pub secret: Option<[u8; 32]>,
    /// Who we are to this channel, and to nothing else: goes into the counts.
    pub follower: [u8; 16],
    /// The newest post we know of.
    pub last: u64,
    /// About how many follow it.
    pub followers: u32,
    pub keep_days: u32,
    pub muted: bool,
    pub at: Millis,
    /// The newest record as it sits in the DHT, to write it again.
    pub head: Option<Vec<u8>>,
    /// Nothing at or below this number is looked for any more: nobody has it.
    #[serde(default)]
    pub floor: u64,
    /// How many looks in a row found a post missing.
    #[serde(default)]
    pub misses: u8,
    /// The followers we know of, for which week.
    #[serde(default)]
    pub sketch: Sketch,
    #[serde(default)]
    pub sketch_week: u64,
}

/// A post as it is here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPost {
    pub channel: String,
    pub number: u64,
    pub at: Millis,
    /// All of it when we have it, or else the first part.
    pub text: String,
    pub more: Option<More>,
    /// We have all of it.
    pub has_body: bool,
    /// For each of [`EMOJI`], about how many.
    pub reactions: Vec<u32>,
    /// Which we chose ourselves.
    pub mine: Vec<u8>,
    /// What we know of them, so a read that failed loses nobody's.
    #[serde(default)]
    pub sketch: Reactions,
}

impl ChannelPost {
    pub fn has_photo(&self) -> bool {
        self.more.as_ref().is_some_and(|m| m.photo)
    }
}

impl Channel {
    pub fn runs_it(&self) -> bool {
        self.secret.is_some()
    }

    pub fn link(&self) -> String {
        ChannelLink {
            key: self.key,
            read: self.read,
            name: self.name.clone(),
        }
        .link()
    }
}

/// What is in the link: enough to read, and to know what is real.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelLink {
    pub key: [u8; 32],
    pub read: [u8; 32],
    pub name: String,
}

impl ChannelLink {
    pub fn link(&self) -> String {
        let bytes = postcard::to_stdvec(self).expect("a channel link serializes");
        format!(
            "{CHANNEL_BASE}{}",
            data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
        )
    }

    pub fn id(&self) -> String {
        data_encoding::BASE32_NOPAD
            .encode(&self.key)
            .to_ascii_lowercase()
    }
}

/// Finds a channel link in whatever was pasted, scanned or clicked.
pub fn parse_link(text: &str) -> Option<ChannelLink> {
    for token in text.split_whitespace() {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric());
        let Some(rest) = token
            .strip_prefix("whatsmine://channel/")
            .or_else(|| token.split_once("/channel#").map(|(_, rest)| rest))
        else {
            continue;
        };
        let upper = rest.to_ascii_uppercase();
        if let Ok(bytes) = data_encoding::BASE32_NOPAD.decode(upper.as_bytes())
            && let Ok(mut link) = postcard::from_bytes::<ChannelLink>(&bytes)
            && PublicKey::from_bytes(&link.key).is_ok()
        {
            link.name = crate::engine::clean_name(&link.name);
            return Some(link);
        }
    }
    None
}

/// A new channel: who runs it holds both keys.
pub fn new_channel(name: &str, now: Millis) -> Channel {
    let secret: [u8; 32] = crate::vault::random();
    let key = *SecretKey::from_bytes(&secret).public().as_bytes();
    let link = ChannelLink {
        key,
        read: crate::vault::random(),
        name: crate::engine::clean_name(name),
    };
    Channel {
        id: link.id(),
        key,
        read: link.read,
        name: link.name,
        secret: Some(secret),
        follower: crate::vault::random(),
        last: 0,
        followers: 0,
        keep_days: KEEP_DAYS,
        muted: false,
        at: now,
        head: None,
        floor: 0,
        misses: 0,
        sketch: Sketch::default(),
        sketch_week: 0,
    }
}

/// A channel someone else runs.
pub fn from_link(link: &ChannelLink, now: Millis) -> Channel {
    Channel {
        id: link.id(),
        key: link.key,
        read: link.read,
        name: link.name.clone(),
        secret: None,
        follower: crate::vault::random(),
        last: 0,
        followers: 0,
        keep_days: KEEP_DAYS,
        muted: false,
        at: now,
        head: None,
        floor: 0,
        misses: 0,
        sketch: Sketch::default(),
        sketch_week: 0,
    }
}

/// More of a post than fits in a record: to get from someone who has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct More {
    /// Of the body as it travels, so anyone can check it without being able to read it.
    pub hash: [u8; 32],
    pub size: u32,
    pub photo: bool,
}

/// A post as it sits in a record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    pub number: u64,
    pub at: Millis,
    /// All of it, or the first part.
    pub text: String,
    pub more: Option<More>,
}

/// The rest of a post.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub text: String,
    pub photo: Option<Vec<u8>>,
}

/// As much of this text as fits in a record, not cutting a character in two.
pub fn short_part(text: &str) -> &str {
    if text.len() <= MAX_SHORT {
        return text;
    }
    let mut end = MAX_SHORT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn signed_bytes(key: &[u8; 32], sealed: &[u8]) -> Vec<u8> {
    let mut out = SIGN_DOMAIN.to_vec();
    out.extend_from_slice(key);
    out.extend_from_slice(sealed);
    out
}

/// A post as it goes into a record: sealed with the read key, and signed by the channel.
pub fn seal_post(secret: &[u8; 32], read: &[u8; 32], post: &Post) -> Option<Vec<u8>> {
    let signer = SecretKey::from_bytes(secret);
    let plain = postcard::to_stdvec(post).ok()?;
    let mut sealed = crate::vault::seal_with(read, &plain, POST_CONTEXT);
    let signature = signer.sign(&signed_bytes(signer.public().as_bytes(), &sealed));
    sealed.extend_from_slice(&signature.to_bytes());
    Some(sealed)
}

/// A post out of a record. Nothing for whatever the channel did not sign, however it got there.
pub fn open_post(key: &[u8; 32], read: &[u8; 32], record: &[u8]) -> Option<Post> {
    let split = record.len().checked_sub(64)?;
    let (sealed, signature) = record.split_at(split);
    let public = PublicKey::from_bytes(key).ok()?;
    let signature = iroh::Signature::from_bytes(signature.try_into().ok()?);
    public.verify(&signed_bytes(key, sealed), &signature).ok()?;
    let plain = crate::vault::open_with(read, sealed, POST_CONTEXT)?;
    let post: Post = postcard::from_bytes(&plain).ok()?;
    (post.text.chars().count() <= MAX_TEXT).then_some(post)
}

pub fn seal_body(read: &[u8; 32], body: &Body) -> Option<Vec<u8>> {
    let plain = postcard::to_stdvec(body).ok()?;
    Some(crate::vault::seal_with(read, &plain, BODY_CONTEXT))
}

pub fn open_body(read: &[u8; 32], sealed: &[u8]) -> Option<Body> {
    let plain = crate::vault::open_with(read, sealed, BODY_CONTEXT)?;
    postcard::from_bytes(&plain).ok()
}

/// Is this what the post said it would be?
pub fn body_is(more: &More, sealed: &[u8]) -> bool {
    sealed.len() <= MAX_BODY && sealed.len() == more.size as usize && hash_of(sealed) == more.hash
}

pub fn hash_of(sealed: &[u8]) -> [u8; 32] {
    *blake3::hash(sealed).as_bytes()
}

/// Something only followers can read: an address, or the counts.
pub fn seal_note(read: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    crate::vault::seal_with(read, plain, NOTE_CONTEXT)
}

pub fn open_note(read: &[u8; 32], sealed: &[u8]) -> Option<Vec<u8>> {
    crate::vault::open_with(read, sealed, NOTE_CONTEXT)
}

/// The salt of a post's own record. The newest post also sits under no salt at all.
pub fn post_salt(number: u64) -> [u8; 9] {
    let mut salt = [0u8; 9];
    salt[0] = b'p';
    salt[1..].copy_from_slice(&number.to_be_bytes());
    salt
}

/// Where a follower leaves an address, for other followers.
pub fn address_seed(read: &[u8; 32], day: u64, slot: u8) -> [u8; 32] {
    crate::records::seed("channel address", read, day, slot)
}

/// Where the follower count of this week sits.
pub fn followers_seed(read: &[u8; 32], week: u64) -> [u8; 32] {
    crate::records::seed("channel followers", read, week, 0)
}

/// Where the reactions on a post sit.
pub fn reactions_seed(read: &[u8; 32], number: u64) -> [u8; 32] {
    crate::records::seed("channel reactions", read, number, 0)
}

/// Shows a follower is one, to another. Not a secret handshake against anyone with the link, only
/// against whoever finds an address and tries it.
pub fn hello(read: &[u8; 32], from: &[u8; 32], to: &[u8; 32]) -> [u8; 32] {
    let mut input = Vec::with_capacity(64);
    input.extend_from_slice(from);
    input.extend_from_slice(to);
    *blake3::keyed_hash(read, &input).as_bytes()
}

// ---- counting without knowing who ----

/// How many different ones there are, about, in very little room. Two can be merged whatever the
/// order and however often, so whoever writes last loses nothing of what they read before.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sketch {
    registers: Vec<u8>,
}

const REGISTERS: usize = 32;

impl Sketch {
    fn ensure(&mut self) {
        if self.registers.len() != REGISTERS {
            self.registers = vec![0; REGISTERS];
        }
    }

    pub fn add(&mut self, id: &[u8]) {
        self.ensure();
        let hash = blake3::hash(id);
        let bytes = hash.as_bytes();
        let register = usize::from(bytes[0]) % REGISTERS;
        let mut word = [0u8; 8];
        word.copy_from_slice(&bytes[1..9]);
        let rank = (u64::from_be_bytes(word).leading_zeros() + 1).min(64) as u8;
        self.registers[register] = self.registers[register].max(rank);
    }

    /// Whether this changed anything.
    pub fn merge(&mut self, other: &Self) -> bool {
        self.ensure();
        let mut changed = false;
        if other.registers.len() == REGISTERS {
            for (mine, theirs) in self.registers.iter_mut().zip(&other.registers) {
                // A rank above 64 cannot come from `add`. Someone is making things up.
                if *theirs <= 64 && *theirs > *mine {
                    *mine = *theirs;
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn estimate(&self) -> u32 {
        if self.registers.len() != REGISTERS {
            return 0;
        }
        let m = REGISTERS as f64;
        let alpha = 0.697;
        let sum: f64 = self
            .registers
            .iter()
            .map(|r| 2f64.powi(-i32::from(*r)))
            .sum();
        let raw = alpha * m * m / sum;
        let zeros = self.registers.iter().filter(|r| **r == 0).count();
        let value = if raw <= 2.5 * m && zeros > 0 {
            m * (m / zeros as f64).ln()
        } else {
            raw
        };
        value.round() as u32
    }
}

/// A number as it can honestly be said: exact when small, rounded when it is an estimate anyway.
pub fn about(count: u32) -> u32 {
    match count {
        0..=10 => count,
        11..=99 => (count + 5) / 10 * 10,
        100..=999 => (count + 50) / 100 * 100,
        _ => (count + 500) / 1000 * 1000,
    }
}

/// The reactions on a post: for each of [`EMOJI`] how many, about.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Reactions {
    pub each: Vec<Sketch>,
}

impl Reactions {
    pub fn add(&mut self, emoji: usize, follower: &[u8; 16]) {
        if emoji >= EMOJI.len() {
            return;
        }
        self.each.resize_with(EMOJI.len(), Sketch::default);
        self.each[emoji].add(follower);
    }

    pub fn merge(&mut self, other: &Self) -> bool {
        self.each.resize_with(EMOJI.len(), Sketch::default);
        let mut changed = false;
        for (mine, theirs) in self.each.iter_mut().zip(&other.each) {
            changed |= mine.merge(theirs);
        }
        changed
    }

    pub fn counts(&self) -> Vec<u32> {
        (0..EMOJI.len())
            .map(|n| self.each.get(n).map_or(0, Sketch::estimate))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> Channel {
        new_channel("Hill climbing news", 1)
    }

    fn post(number: u64, text: &str) -> Post {
        Post {
            number,
            at: 5,
            text: text.to_owned(),
            more: None,
        }
    }

    #[test]
    fn a_link_holds_what_it_takes_to_read_and_nothing_to_write() {
        let c = channel();
        let link = c.link();
        assert!(link.starts_with(CHANNEL_BASE));
        let parsed = parse_link(&format!("Follow this: {link} !")).unwrap();
        assert_eq!(parsed.key, c.key);
        assert_eq!(parsed.read, c.read);
        assert_eq!(parsed.name, c.name);
        assert_eq!(parsed.id(), c.id);
        let followed = from_link(&parsed, 2);
        assert!(!followed.runs_it());
        assert_ne!(followed.follower, c.follower, "each their own");
        // The signing key is nowhere in it.
        let secret = c.secret.unwrap();
        let bytes = data_encoding::BASE32_NOPAD
            .decode(link[CHANNEL_BASE.len()..].to_ascii_uppercase().as_bytes())
            .unwrap();
        assert!(!bytes.windows(32).any(|w| w == secret));
        // Whatever else: no channel.
        for bad in [
            "",
            "hello",
            "https://x/channel#",
            "whatsmine://channel/AAAA",
            "https://x/channel#zzzz",
        ] {
            assert_eq!(parse_link(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_the_channel_can_write_and_only_followers_can_read() {
        let c = channel();
        let secret = c.secret.unwrap();
        let sealed = seal_post(&secret, &c.read, &post(1, "Saturday")).unwrap();
        assert_eq!(
            open_post(&c.key, &c.read, &sealed).unwrap().text,
            "Saturday"
        );
        // Without the read key: nothing.
        assert_eq!(open_post(&c.key, &[9; 32], &sealed), None);
        // A follower has the read key, and can seal, but does not have the signing key.
        let fake = seal_post(&[7; 32], &c.read, &post(2, "Send money")).unwrap();
        assert_eq!(open_post(&c.key, &c.read, &fake), None);
        // Changed on the way: nothing.
        let mut bent = sealed.clone();
        bent[3] ^= 1;
        assert_eq!(open_post(&c.key, &c.read, &bent), None);
        let mut bent = sealed.clone();
        *bent.last_mut().unwrap() ^= 1;
        assert_eq!(open_post(&c.key, &c.read, &bent), None);
        // Another channel's post does not pass for this one's.
        let other = channel();
        let theirs = seal_post(&other.secret.unwrap(), &c.read, &post(3, "x")).unwrap();
        assert_eq!(open_post(&c.key, &c.read, &theirs), None);
        // Whatever bytes: no panic, and nothing opens.
        for len in 0..200 {
            assert_eq!(open_post(&c.key, &c.read, &vec![7; len]), None);
        }
    }

    #[test]
    fn the_longest_short_post_fits_in_a_record() {
        let c = channel();
        let secret = c.secret.unwrap();
        let text = "x".repeat(MAX_SHORT);
        let longest = Post {
            number: u64::MAX,
            at: i64::MAX,
            text,
            more: Some(More {
                hash: [1; 32],
                size: u32::MAX,
                photo: true,
            }),
        };
        let sealed = seal_post(&secret, &c.read, &longest).unwrap();
        assert!(
            sealed.len() <= crate::records::MAX_RECORD,
            "{} does not fit",
            sealed.len()
        );
    }

    #[test]
    fn a_short_part_does_not_cut_a_character() {
        assert_eq!(short_part("hello"), "hello");
        let long = "é".repeat(MAX_SHORT);
        let short = short_part(&long);
        assert!(short.len() <= MAX_SHORT);
        assert!(short.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_body_is_what_the_post_said_or_it_is_not_used() {
        let c = channel();
        let body = Body {
            text: "A long story".into(),
            photo: Some(vec![1, 2, 3]),
        };
        let sealed = seal_body(&c.read, &body).unwrap();
        let more = More {
            hash: hash_of(&sealed),
            size: sealed.len() as u32,
            photo: true,
        };
        assert!(body_is(&more, &sealed));
        assert_eq!(open_body(&c.read, &sealed), Some(body));
        let mut bent = sealed.clone();
        bent[0] ^= 1;
        assert!(!body_is(&more, &bent));
        assert!(!body_is(&more, &sealed[1..]));
        assert_eq!(open_body(&[9; 32], &sealed), None);
    }

    #[test]
    fn records_of_each_channel_and_post_are_apart() {
        let (a, b) = (channel(), channel());
        assert_ne!(post_salt(1), post_salt(2));
        assert_ne!(address_seed(&a.read, 1, 0), address_seed(&b.read, 1, 0));
        assert_ne!(address_seed(&a.read, 1, 0), address_seed(&a.read, 2, 0));
        assert_ne!(address_seed(&a.read, 1, 0), address_seed(&a.read, 1, 1));
        assert_ne!(followers_seed(&a.read, 1), reactions_seed(&a.read, 1));
        assert_ne!(
            hello(&a.read, &[1; 32], &[2; 32]),
            hello(&b.read, &[1; 32], &[2; 32])
        );
        assert_ne!(
            hello(&a.read, &[1; 32], &[2; 32]),
            hello(&a.read, &[2; 32], &[1; 32])
        );
    }

    #[test]
    fn a_sketch_counts_about_right_and_merging_loses_nothing() {
        for truth in [1u32, 5, 10, 30, 80, 300, 1000, 5000] {
            let mut sketch = Sketch::default();
            for n in 0..truth {
                sketch.add(&n.to_be_bytes());
                // The same one again counts once.
                sketch.add(&n.to_be_bytes());
            }
            let estimate = f64::from(sketch.estimate());
            let wrong = (estimate - f64::from(truth)).abs() / f64::from(truth);
            assert!(wrong < 0.6, "{truth} came out as {estimate}");
        }
        // Whatever the order, and whoever has seen what.
        let (mut a, mut b, mut all) = (Sketch::default(), Sketch::default(), Sketch::default());
        for n in 0..200u32 {
            let bytes = n.to_be_bytes();
            all.add(&bytes);
            if n % 2 == 0 {
                a.add(&bytes)
            } else {
                b.add(&bytes)
            }
        }
        let mut ab = a.clone();
        assert!(ab.merge(&b));
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab, ba);
        assert_eq!(ab, all);
        assert!(!ab.merge(&a), "nothing new");
    }

    #[test]
    fn a_sketch_from_someone_else_cannot_do_harm() {
        let mut mine = Sketch::default();
        mine.add(b"me");
        let before = mine.clone();
        // Another size, or ranks that cannot be.
        assert!(!mine.merge(&Sketch {
            registers: vec![9; 1000]
        }));
        assert!(!mine.merge(&Sketch {
            registers: vec![255; REGISTERS]
        }));
        assert_eq!(mine, before);
        assert_eq!(
            Sketch {
                registers: vec![1, 2]
            }
            .estimate(),
            0
        );
        let mut reactions = Reactions::default();
        reactions.add(99, &[1; 16]);
        assert_eq!(reactions.counts(), vec![0; EMOJI.len()]);
        reactions.merge(&Reactions {
            each: vec![Sketch::default(); 1000],
        });
        assert_eq!(reactions.counts().len(), EMOJI.len());
    }

    #[test]
    fn numbers_are_said_as_exactly_as_they_are_known() {
        assert_eq!(about(0), 0);
        assert_eq!(about(7), 7);
        assert_eq!(about(83), 80);
        assert_eq!(about(87), 90);
        assert_eq!(about(340), 300);
        assert_eq!(about(1600), 2000);
    }

    #[test]
    fn reactions_merge_and_count_per_emoji() {
        let (mut a, mut b) = (Reactions::default(), Reactions::default());
        a.add(0, &[1; 16]);
        b.add(0, &[2; 16]);
        b.add(1, &[2; 16]);
        assert!(a.merge(&b));
        let counts = a.counts();
        assert_eq!(counts[0], 2);
        assert_eq!(counts[1], 1);
        assert_eq!(counts[2], 0);
    }
}
