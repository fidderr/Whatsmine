//! Channels on the network: see `crate::channel` for what they are.
//!
//! Reading takes no connection: at each check, the records of a channel are read from the DHT. Long
//! posts and photos do take one. For that a follower brings up an endpoint of its own for a while,
//! with a key it uses for nothing else and never again, and leaves its address sealed in the DHT for
//! other followers. They ask for what they miss, check it against the hash in the signed post, and
//! go. Nothing stays connected, and nothing ties this to who someone is or to their other channels.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, SecretKey};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

use super::{Engine, Internal, NetMode, emit};
use crate::channel::{self, Channel, More, Post, Reactions, Sketch};
use crate::db::Database;
use crate::location::LocationRecord;
use crate::model::{CoreEvent, Millis, now_ms};
use crate::power::{self, Plan};
use crate::records::{Records, SignedRecord};

/// Which channel used which key, for a test to see they are not anyone's.
pub(super) type KeysUsed = Arc<std::sync::Mutex<Vec<(String, [u8; 32])>>>;

const ALPN: &[u8] = b"whatsmine/channel/1";
const LOOK_IN_USE: Duration = Duration::from_secs(60);
const LOOK_AWAY: Duration = Duration::from_secs(15 * 60);
/// Tests have no time for that.
const LOOK_LOCAL: Duration = Duration::from_millis(150);
/// How many missed posts to read in one go.
const MOST_PER_LOOK: u64 = 16;
/// How many of the newest posts a follower writes again.
const KEEP_ALIVE: usize = 3;
/// How long an endpoint stays up for others to come and get things.
const SERVE_FOR: Duration = Duration::from_secs(10 * 60);
const SERVE_FOR_LOCAL: Duration = Duration::from_secs(20);
/// How many others to try for what is missing.
const MOST_TRIED: usize = 4;
const CONNECT_LIMIT: Duration = Duration::from_secs(8);
const STREAM_LIMIT: Duration = Duration::from_secs(60);
/// What one connection may ask for.
const MOST_WANTS: usize = 32;
const WEEK: Millis = 7 * 24 * 3600 * 1000;
/// How far back a follower looks.
const WINDOW: u64 = 64;
/// How many looks a missing post gets.
const GIVE_UP_AFTER: u8 = 3;

#[derive(Default)]
pub(super) struct ChannelSide {
    looking: bool,
    /// When to see whether there is anything to do.
    check_at: Option<Instant>,
    /// When something is due: nothing wakes a phone for someone who follows no channels.
    next_look: Option<Instant>,
    /// Channels with an endpoint up right now.
    swarms: HashSet<String>,
    /// When to try again for what nobody could give, and how many times we did.
    want_after: HashMap<String, (Instant, u32)>,
    /// Ends whatever is up.
    stop: Arc<Notify>,
}

/// What a look at one channel found.
pub(super) struct Found {
    id: String,
    head: Option<SignedRecord>,
    /// The newest number there is.
    newest: u64,
    posts: Vec<(Post, SignedRecord)>,
    /// How many of those looked for were not there.
    missed: usize,
    sketch: Option<(u64, Sketch)>,
    reactions: Vec<(u64, Reactions)>,
}

impl Engine {
    fn may_pass_on(&self, plan: &Plan) -> bool {
        if !self.settings().channel_pass_on {
            return false;
        }
        if matches!(self.mode, NetMode::Local(_)) {
            return true;
        }
        let device = &self.conditions().device;
        if device.power_save || device.low_battery {
            return false;
        }
        // Nothing is held on a phone in a pocket on mobile data.
        self.in_front
            || device.charging
            || (plan.endpoint_up && device.network == power::Network::Wifi)
    }

    pub(super) fn channels_due(&mut self, plan: &Plan, now: Instant) {
        if self.channel.looking || self.channel.check_at.is_some_and(|at| at > now) {
            return;
        }
        let Some(records) = self.records.clone() else {
            return;
        };
        let every = if matches!(self.mode, NetMode::Local(_)) {
            LOOK_LOCAL
        } else if plan.level == power::Level::InUse {
            LOOK_IN_USE
        } else {
            LOOK_AWAY
        };
        self.channel.check_at = Some(now + every);
        let channels = self.db.channels();
        if channels.is_empty() {
            self.channel.next_look = None;
            return;
        }
        self.channel.next_look = self.channel.check_at;
        self.channel.looking = true;
        let pass_on = self.may_pass_on(plan);
        let db = self.db.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut found = Vec::with_capacity(channels.len());
            for c in channels {
                found.push(look_at(&records, &db, &c, pass_on).await);
            }
            let _ = tx.send(Internal::ChannelsFound(found, pass_on));
        });
    }

    /// The DHT is there now.
    pub(super) fn channels_can_go(&mut self) {
        self.channel.check_at = Some(Instant::now());
    }

    pub(super) fn next_channel_look(&self) -> Option<Instant> {
        self.channel.next_look
    }

    pub(super) fn on_channels_found(&mut self, found: Vec<Found>, pass_on: bool) {
        self.channel.looking = false;
        let mut changed = false;
        for f in found {
            let Some(mut c) = self.db.channel(&f.id) else {
                continue;
            };
            if let Some(head) = f.head
                && head.seq as u64 >= c.last
            {
                c.head = Some(head.to_bytes());
            }
            let cutoff = now_ms() - i64::from(c.keep_days) * 24 * 3600 * 1000;
            for (post, record) in f.posts {
                if post.at < cutoff {
                    continue;
                }
                let text = post.text.clone();
                if self.db.add_channel_post(&c.id, &post, &record) {
                    changed = true;
                    if !c.muted && !c.runs_it() {
                        emit(
                            &self.events,
                            CoreEvent::ChannelPost {
                                channel: c.id.clone(),
                                name: c.name.clone(),
                                text,
                            },
                        );
                    }
                }
            }
            c.last = c.last.max(f.newest);
            // Gone from the DHT, and nobody who has them writes them again: not for ever.
            if f.missed > 0 {
                c.misses = c.misses.saturating_add(1);
                if c.misses >= GIVE_UP_AFTER {
                    c.floor = c.last;
                    c.misses = 0;
                }
            } else {
                c.misses = 0;
            }
            if let Some((week, sketch)) = f.sketch {
                let estimate = sketch.estimate();
                if estimate != c.followers {
                    c.followers = estimate;
                    changed = true;
                }
                c.sketch = sketch;
                c.sketch_week = week;
            }
            self.db.update_channel(&c);
            for (number, reactions) in f.reactions {
                self.db.set_channel_sketch(&c.id, number, &reactions);
                changed = true;
            }
            self.db
                .expire_channel_posts(&c.id, now_ms() - i64::from(c.keep_days) * 24 * 3600 * 1000);
            self.channel_swarm_if_needed(&c, pass_on);
        }
        if changed {
            emit(&self.events, CoreEvent::ChannelsChanged);
        }
    }

    /// An endpoint for this channel, if something is missing or others could use what we have.
    fn channel_swarm_if_needed(&mut self, c: &Channel, pass_on: bool) {
        if self.channel.swarms.contains(&c.id) {
            return;
        }
        let serve = pass_on && self.db.channel_has_bodies(&c.id);
        let now = Instant::now();
        // Something nobody can give right now is not worth an endpoint every minute.
        let wants = if self
            .channel
            .want_after
            .get(&c.id)
            .is_none_or(|(at, _)| *at <= now)
        {
            self.db.channel_wants(&c.id)
        } else {
            Vec::new()
        };
        if wants.is_empty() && !serve {
            return;
        }
        if !wants.is_empty() {
            let tries = self.channel.want_after.get(&c.id).map_or(0, |(_, n)| *n);
            let (first, most) = if matches!(self.mode, NetMode::Local(_)) {
                (Duration::from_millis(500), Duration::from_secs(2))
            } else {
                (LOOK_IN_USE, Duration::from_secs(3600))
            };
            let wait = first.saturating_mul(1u32 << tries.min(10)).min(most);
            self.channel
                .want_after
                .insert(c.id.clone(), (now + wait, tries.saturating_add(1)));
        }
        let Some(records) = self.records.clone() else {
            return;
        };
        self.channel.swarms.insert(c.id.clone());
        let settings = self.settings();
        tokio::spawn(swarm(Swarm {
            channel: c.clone(),
            db: self.db.clone(),
            records,
            mode: self.mode.clone(),
            only_relays: settings.channel_relays_only,
            use_relays: settings.use_relays,
            serve,
            wants,
            tx: self.tx.clone(),
            stop: self.channel.stop.clone(),
            keys_used: self.shared.channel_swarm_keys.clone(),
        }));
    }

    /// We wrote something: it goes in the DHT now, and whoever comes for the rest can get it.
    pub(super) fn channel_out(&mut self, id: &str) {
        let Some(c) = self.db.channel(id) else { return };
        // Whenever the DHT is there, not at some later check.
        self.channel.check_at = Some(Instant::now());
        self.channel.next_look = self.channel.check_at;
        let Some(records) = self.records.clone() else {
            return;
        };
        let newest = self.db.channel_newest_records(id, KEEP_ALIVE);
        let head = c.head.as_deref().and_then(SignedRecord::from_bytes);
        // All at once: a write can take many seconds.
        for record in head.into_iter().chain(newest) {
            let records = records.clone();
            tokio::spawn(async move {
                let done = records.put_signed(&record).await;
                tracing::debug!("a channel's post written: {done}");
            });
        }
        let pass_on = self.settings().channel_pass_on || c.runs_it();
        self.channel_swarm_if_needed(&c, pass_on);
    }

    pub(super) fn channel_look_now(&mut self) {
        self.channel.check_at = Some(Instant::now());
        self.channel.next_look = self.channel.check_at;
    }

    pub(super) fn on_channel_body(&mut self, id: &str, number: u64, sealed: &[u8]) {
        let (Some(c), Some(post)) = (self.db.channel(id), self.db.channel_post(id, number)) else {
            return;
        };
        let Some(more) = &post.more else { return };
        if !channel::body_is(more, sealed) {
            return;
        }
        let Some(body) = channel::open_body(&c.read, sealed) else {
            return;
        };
        self.db
            .set_channel_body(id, number, more, sealed, &body.text);
        self.channel.want_after.remove(id);
        emit(&self.events, CoreEvent::ChannelsChanged);
    }

    pub(super) fn on_channel_swarm_over(&mut self, id: &str) {
        self.channel.swarms.remove(id);
    }

    /// Everything goes down: with the app, or when the radio can rest.
    pub(super) fn channels_stop(&mut self) {
        self.channel.stop.notify_waiters();
        self.channel.looking = false;
    }
}

async fn read_note<T: serde::de::DeserializeOwned>(
    records: &Records,
    seed: &[u8; 32],
    read: &[u8; 32],
) -> Option<T> {
    let sealed = records.get(seed).await?;
    let plain = channel::open_note(read, &sealed)?;
    postcard::from_bytes(&plain).ok()
}

async fn write_note<T: serde::Serialize>(
    records: &Records,
    seed: &[u8; 32],
    read: &[u8; 32],
    value: &T,
) {
    if let Ok(plain) = postcard::to_stdvec(value) {
        records.put(seed, &channel::seal_note(read, &plain)).await;
    }
}

/// One look at a channel. Whatever is read here is checked: the DHT is anybody's to write to.
async fn look_at(records: &Records, db: &Database, c: &Channel, pass_on: bool) -> Found {
    let mut found = Found {
        id: c.id.clone(),
        head: None,
        newest: c.last,
        posts: Vec::new(),
        missed: 0,
        sketch: None,
        reactions: Vec::new(),
    };
    let head = records.get_signed(&c.key, None).await;
    let head_post = head
        .as_ref()
        .and_then(|h| channel::open_post(&c.key, &c.read, &h.value));
    if let (Some(head), Some(post)) = (head, head_post) {
        let newest = post.number;
        found.newest = newest;
        if newest >= c.last {
            found.head = Some(head.clone());
        }
        // What we miss of the newest ones, newest first. Someone new starts with those, not with
        // what nobody has any more, and what could not be read now gets another chance.
        let from = newest
            .saturating_sub(WINDOW - 1)
            .max(c.floor.saturating_add(1));
        let missing: Vec<u64> = (from..=newest)
            .rev()
            .filter(|number| db.channel_post(&c.id, *number).is_none())
            .take(MOST_PER_LOOK as usize)
            .collect();
        let mut reads = tokio::task::JoinSet::new();
        for number in missing {
            let (records, key) = (records.clone(), c.key);
            reads.spawn(async move {
                let salt = channel::post_salt(number);
                (number, records.get_signed(&key, Some(&salt)).await)
            });
        }
        let mut newest_found = false;
        let mut looked_for_newest = false;
        while let Some(read) = reads.join_next().await {
            let Ok((number, record)) = read else { continue };
            looked_for_newest |= number == newest;
            let opened = record.and_then(|record| {
                channel::open_post(&c.key, &c.read, &record.value)
                    .filter(|post| post.number == number)
                    .map(|post| (post, record))
            });
            match opened {
                Some(item) => {
                    newest_found |= number == newest;
                    found.posts.push(item);
                }
                // The newest one is in the head as well.
                None if number == newest => {}
                None => found.missed += 1,
            }
        }
        if looked_for_newest && !newest_found {
            found.posts.push((post, head));
        }
        found.posts.sort_by_key(|(post, _)| post.number);
    }
    // The counts: what we knew merged with what is there, and whoever has more says so. Whoever runs
    // it is no follower.
    let week = (now_ms() / WEEK) as u64;
    let seed = channel::followers_seed(&c.read, week);
    let mut sketch = if c.sketch_week == week {
        c.sketch.clone()
    } else {
        Sketch::default()
    };
    if !c.runs_it() {
        sketch.add(&c.follower);
    }
    let there: Option<Sketch> = read_note(records, &seed, &c.read).await;
    if let Some(there) = &there {
        sketch.merge(there);
    }
    let mut outside = there.unwrap_or_default();
    if outside.merge(&sketch) {
        write_note(records, &seed, &c.read, &sketch).await;
    }
    found.sketch = Some((week, sketch));
    // The same for reactions on the newest posts.
    let mut reads = tokio::task::JoinSet::new();
    for number in db.channel_recent_numbers(&c.id, 5) {
        let known = db
            .channel_post(&c.id, number)
            .map(|post| post.sketch)
            .unwrap_or_default();
        let (records, read) = (records.clone(), c.read);
        reads.spawn(async move {
            let seed = channel::reactions_seed(&read, number);
            let there: Option<Reactions> = read_note(&records, &seed, &read).await;
            let mut merged = known;
            let grew = there.as_ref().is_some_and(|there| merged.merge(there));
            let mut outside = there.unwrap_or_default();
            if outside.merge(&merged) {
                write_note(&records, &seed, &read, &merged).await;
            }
            (number, grew.then_some(merged))
        });
    }
    while let Some(read) = reads.join_next().await {
        if let Ok((number, Some(merged))) = read {
            found.reactions.push((number, merged));
        }
    }
    // Keeps them there while the author is away. Who wrote them can do so whenever.
    if pass_on || c.runs_it() {
        let head = c.head.as_deref().and_then(SignedRecord::from_bytes);
        let mut writes = tokio::task::JoinSet::new();
        for record in head
            .into_iter()
            .chain(db.channel_newest_records(&c.id, KEEP_ALIVE))
        {
            let records = records.clone();
            writes.spawn(async move { records.put_signed(&record).await });
        }
        while writes.join_next().await.is_some() {}
    }
    found
}

struct Swarm {
    channel: Channel,
    db: Arc<Database>,
    records: Records,
    mode: NetMode,
    only_relays: bool,
    use_relays: bool,
    serve: bool,
    wants: Vec<(u64, More)>,
    tx: mpsc::UnboundedSender<Internal>,
    stop: Arc<Notify>,
    keys_used: KeysUsed,
}

async fn bind_swarm(
    key: SecretKey,
    mode: &NetMode,
    only_relays: bool,
    use_relays: bool,
) -> anyhow::Result<Endpoint> {
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()]);
    match mode {
        NetMode::Internet => {
            anyhow::ensure!(
                !only_relays || use_relays,
                "only through relays, and none on"
            );
            let relays = if use_relays {
                iroh::RelayMode::Default
            } else {
                iroh::RelayMode::Disabled
            };
            let builder = builder.relay_mode(relays);
            let builder = if only_relays {
                builder.clear_ip_transports()
            } else {
                builder
            };
            Ok(builder.bind().await?)
        }
        NetMode::Local(_) => Ok(builder
            .relay_mode(iroh::RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")?
            .bind()
            .await?),
    }
}

async fn swarm(s: Swarm) {
    let id = s.channel.id.clone();
    let tx = s.tx.clone();
    if let Err(e) = swarm_inner(s).await {
        tracing::debug!("a channel's endpoint: {e:#}");
    }
    let _ = tx.send(Internal::ChannelSwarmOver(id));
}

async fn swarm_inner(s: Swarm) -> anyhow::Result<()> {
    // Made for this, used for nothing else, and never again.
    let secret: [u8; 32] = crate::vault::random();
    let key = SecretKey::from_bytes(&secret);
    let me = *key.public().as_bytes();
    {
        let mut used = s.keys_used.lock().unwrap_or_else(|p| p.into_inner());
        if used.len() >= 64 {
            used.remove(0);
        }
        used.push((s.channel.id.clone(), me));
    }
    let ep = bind_swarm(key, &s.mode, s.only_relays, s.use_relays).await?;
    // However this ends, the endpoint goes: that also ends what is serving others.
    let result = tokio::select! {
        result = run_swarm(&s, &ep, me) => result,
        () = s.stop.notified() => Ok(()),
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), ep.close()).await;
    result
}

async fn run_swarm(s: &Swarm, ep: &Endpoint, me: [u8; 32]) -> anyhow::Result<()> {
    if s.serve {
        tokio::spawn(accept_loop(ep.clone(), s.channel.clone(), s.db.clone(), me));
        if matches!(s.mode, NetMode::Internet) {
            let _ = tokio::time::timeout(Duration::from_secs(10), ep.online()).await;
        }
        let record = LocationRecord::from_addr(&ep.addr(), now_ms());
        write_note(
            &s.records,
            &channel::address_seed(
                &s.channel.read,
                crate::records::day_of(now_ms()),
                crate::vault::random::<1>()[0] % channel::SLOTS,
            ),
            &s.channel.read,
            &record,
        )
        .await;
    }
    if !s.wants.is_empty() {
        let mut others = find_others(&s.records, &s.channel.read, &me).await;
        others.truncate(MOST_TRIED);
        let mut left = s.wants.clone();
        for other in others {
            if left.is_empty() {
                break;
            }
            let attempt = tokio::time::timeout(CONNECT_LIMIT, ep.connect(other, ALPN)).await;
            let Ok(Ok(conn)) = attempt else { continue };
            let got = tokio::time::timeout(
                STREAM_LIMIT,
                fetch(conn.clone(), &s.channel, me, &left, &s.tx),
            )
            .await
            .unwrap_or_default();
            left.retain(|(number, _)| !got.contains(number));
            conn.close(0u32.into(), b"thanks");
        }
    }
    if s.serve {
        let lasts = if matches!(s.mode, NetMode::Local(_)) {
            SERVE_FOR_LOCAL
        } else {
            SERVE_FOR
        };
        tokio::time::sleep(lasts).await;
    }
    Ok(())
}

/// Who left an address for this channel, today or yesterday, in no particular order.
async fn find_others(records: &Records, read: &[u8; 32], me: &[u8; 32]) -> Vec<EndpointAddr> {
    let today = crate::records::day_of(now_ms());
    let mut reads = tokio::task::JoinSet::new();
    for day in [today, today.saturating_sub(1)] {
        for slot in 0..channel::SLOTS {
            let (records, read) = (records.clone(), *read);
            reads.spawn(async move {
                read_note::<LocationRecord>(
                    &records,
                    &channel::address_seed(&read, day, slot),
                    &read,
                )
                .await
            });
        }
    }
    let mut found = Vec::new();
    while let Some(read) = reads.join_next().await {
        if let Ok(Some(record)) = read
            && let Some(addr) = record.to_addr()
            && addr.id.as_bytes() != me
        {
            found.push(addr);
        }
    }
    // Not always the same few.
    let mut order: Vec<(u64, EndpointAddr)> = found
        .into_iter()
        .map(|a| (u64::from_be_bytes(crate::vault::random()), a))
        .collect();
    order.sort_by_key(|(n, _)| *n);
    order.into_iter().map(|(_, a)| a).collect()
}

async fn accept_loop(ep: Endpoint, c: Channel, db: Arc<Database>, me: [u8; 32]) {
    while let Some(incoming) = ep.accept().await {
        let (c, db) = (c.clone(), db.clone());
        tokio::spawn(async move {
            let Ok(Ok(conn)) = tokio::time::timeout(CONNECT_LIMIT, async {
                let accepting = incoming.accept()?;
                anyhow::Ok(accepting.await?)
            })
            .await
            else {
                return;
            };
            let _ = tokio::time::timeout(STREAM_LIMIT, serve(conn, &c, &db, me)).await;
        });
    }
}

/// Someone comes for what they miss. Whoever does not show they have the link gets nothing.
async fn serve(conn: Connection, c: &Channel, db: &Database, me: [u8; 32]) -> anyhow::Result<()> {
    let (mut send, mut recv) = conn.accept_bi().await?;
    let mut hello = [0u8; 32];
    recv.read_exact(&mut hello).await?;
    let them = *conn.remote_id().as_bytes();
    if hello != channel::hello(&c.read, &them, &me) {
        conn.close(1u32.into(), b"no");
        return Ok(());
    }
    for _ in 0..MOST_WANTS {
        let mut hash = [0u8; 32];
        if recv.read_exact(&mut hash).await.is_err() {
            break;
        }
        let body = db.channel_body(&c.id, &hash).unwrap_or_default();
        send.write_all(&(body.len() as u32).to_be_bytes()).await?;
        send.write_all(&body).await?;
    }
    let _ = send.finish();
    // Until they are done with it.
    conn.closed().await;
    Ok(())
}

/// What we miss, from someone who may have it. Returns which posts came, checked.
async fn fetch(
    conn: Connection,
    c: &Channel,
    me: [u8; 32],
    wants: &[(u64, More)],
    tx: &mpsc::UnboundedSender<Internal>,
) -> HashSet<u64> {
    let mut got = HashSet::new();
    let Ok((mut send, mut recv)) = conn.open_bi().await else {
        return got;
    };
    let them = *conn.remote_id().as_bytes();
    if send
        .write_all(&channel::hello(&c.read, &me, &them))
        .await
        .is_err()
    {
        return got;
    }
    for (number, more) in wants.iter().take(MOST_WANTS) {
        if send.write_all(&more.hash).await.is_err() {
            break;
        }
        let mut length = [0u8; 4];
        if recv.read_exact(&mut length).await.is_err() {
            break;
        }
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 {
            continue;
        }
        // Not more than the post said, whatever they say now.
        if length != more.size as usize || length > channel::MAX_BODY {
            break;
        }
        let mut sealed = vec![0u8; length];
        if recv.read_exact(&mut sealed).await.is_err() {
            break;
        }
        if !channel::body_is(more, &sealed) {
            // Not what the post said: nothing more from this one.
            break;
        }
        got.insert(*number);
        let _ = tx.send(Internal::ChannelBody {
            id: c.id.clone(),
            number: *number,
            sealed,
        });
    }
    let _ = send.finish();
    got
}
