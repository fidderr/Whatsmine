//! The networking engine: one task that owns the iroh endpoint and every
//! live session, driven by commands from the UI and events from connection
//! tasks. All protocol rules live here.
//!
//! Reliability comes from acknowledgements, not from connections: an
//! outgoing message stays "undelivered" until the peer's `Delivered` receipt
//! arrives, and every new session re-sends what is undelivered. Receivers
//! dedupe by message id, so re-sending is always safe.
//!
//! How much network is kept up follows [`crate::power`]. Nobody is dialled without a reason:
//! something to send, a chat that is open, or a look around (at the start, on opening the app,
//! after a network change, and on each planned check).

use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, PublicKey, SecretKey, Signature};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::NetMode;
use crate::calls::{self, ActiveCall, CallSnapshot, MediaIn, MediaSink};
use crate::db::Database;
use crate::files::{self, ReceiveCtx, Received};
use crate::groups;
use crate::identity::{Identity, key_to_string};
use crate::location::{self, LocationRecord};
use crate::model::{
    Attachment, ContactState, CoreEvent, Message, MessageStatus, MyInvite, NetStatus, Settings,
    TransferState, Trust, now_ms,
};
use crate::power::{self, Conditions, Device, Plan};
use crate::protocol::{
    self, ALPN, Frame, GroupState, MAX_TEXT, PROTO_VERSION, ReceiptKind, VIDEO_STREAM_MAGIC,
    id_from_bytes, id_to_bytes,
};
use crate::records::{self, Records};
use crate::requests::{self, Request};
use crate::retry;
use crate::sealing::{OpenError, Sealing};

mod call;
mod group;

use call::{CallState, GroupCalls};

const DIAL_TIMEOUT: Duration = Duration::from_secs(15);
/// A connection that broke by itself is tried again this soon: most are network blips.
const BLIP_RETRY: Duration = Duration::from_secs(3);
/// Keep trying someone whose connection just broke for this long.
const BLIP_LASTS: Duration = Duration::from_secs(5 * 60);
const TYPING_THROTTLE: Duration = Duration::from_secs(3);
/// While a call rings, its time-out is looked at this often.
const CALL_TICK: Duration = Duration::from_secs(2);
/// Another app using the radio is a cheap moment to look around, but not all the time.
const RADIO_LOOK_EVERY: Duration = Duration::from_secs(60);
/// How long a wake-up keeps the endpoint up, for what it finds to get through.
const AWAKE_FOR: Duration = Duration::from_secs(30);
/// Looking around dials this many people at a time.
const LOOK_AROUND_DIALS: usize = 8;
/// A copy of a message can be under way for about as long as the sender keeps trying, at most.
const SEEN_KEPT_MS: i64 = 400 * 24 * 3600 * 1000;
/// Republish where we are at least this often, also if nothing changed.
const PUBLISH_EVERY: Duration = Duration::from_secs(3600);
/// How long the other side gets to prove who it is.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
/// Look for requests this often while the app is in use, and this often otherwise.
const REQUESTS_IN_USE: Duration = Duration::from_secs(60);
const REQUESTS_REACHABLE: Duration = Duration::from_secs(5 * 60);
/// Tests have no time for that.
const REQUESTS_LOCAL: Duration = Duration::from_millis(100);
/// A request of ours is left again after this long, while unanswered.
const REQUEST_AGAIN: [Duration; 4] = [
    Duration::from_secs(30),
    Duration::from_secs(2 * 60),
    Duration::from_secs(10 * 60),
    Duration::from_secs(60 * 60),
];
/// A wake-up scheduled with the system only changes for a difference of this much.
const WAKE_SLACK: Duration = Duration::from_secs(5);

/// Requests from the `Core` handle.
pub(crate) enum Cmd {
    /// Connect to this contact now if not connected.
    Dial(PublicKey),
    /// Leave this contact our request.
    SendRequest(PublicKey),
    /// A new location, and a new network address if asked: whoever knew the old ones cannot
    /// follow. Contacts are told.
    Rotate {
        network: bool,
    },
    /// The device's state changed.
    Device(Device),
    /// The network may be another one now, also if it is the same kind.
    NetworkChanged,
    /// The app came on screen or went off it.
    InFront(bool),
    /// This chat is open on screen, or none is.
    ChatOpen(Option<PublicKey>),
    /// The system woke us, as asked with [`CoreEvent::WakeIn`].
    Wake,
    /// The radio went active for some app: a cheap moment to look around.
    RadioActive,
    /// How the connection stands, for the screen that shows it.
    Report(tokio::sync::oneshot::Sender<crate::model::ConnectionReport>),
    /// Send this stored message if the contact is connected.
    SendOne {
        contact: PublicKey,
        message_id: String,
    },
    /// We just accepted this contact.
    Accepted(PublicKey),
    /// We read messages from this contact.
    ReadReceipts(PublicKey),
    Typing(PublicKey),
    /// Drop any session with this peer (blocked or deleted).
    Disconnect(PublicKey),
    /// Name, picture or privacy flags changed; re-announce.
    SettingsChanged,
    /// Try a failed incoming file again from the start.
    RetryFile {
        contact: PublicKey,
        message_id: String,
    },
    /// Start a call to this contact or group (id chosen by the caller).
    CallInvite {
        target: String,
        call: [u8; 16],
        video: bool,
    },
    /// Join the group call that is going on.
    CallJoin {
        group: String,
    },
    /// Send this stored group message to the members that are connected.
    SendGroup {
        group: String,
        message_id: String,
    },
    /// The group changed (we are its admin): send its new state around.
    GroupPublish {
        state: GroupState,
        sig: Vec<u8>,
        names: Vec<(String, String)>,
        to: Vec<PublicKey>,
    },
    /// We left a group: tell its admin.
    GroupLeave {
        group: String,
        admin: PublicKey,
    },
    /// Answer the call that is ringing.
    CallAnswer {
        accept: bool,
    },
    /// Hang up, or cancel while ringing.
    CallEnd,
    CallAudio {
        ts: u32,
        data: Vec<u8>,
    },
    CallVideo {
        ts: u32,
        key: bool,
        data: Vec<u8>,
    },
    /// Our camera went on or off.
    CallVideoOn(bool),
    Shutdown(std::sync::mpsc::Sender<()>),
}

/// Events from the connection tasks back into the engine.
enum Internal {
    /// A connection, before anyone has said who they are.
    Connected {
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        /// Whoever we dialled, or nobody when they called us.
        expect: Option<PublicKey>,
    },
    /// Both sides have proven who they are.
    NewConn {
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        outgoing: bool,
        peer: PublicKey,
        curve: [u8; 32],
        one_time: [u8; 32],
    },
    DialFailed(PublicKey),
    Published(Option<EndpointAddr>),
    /// Notes found in the DHT: the invite they were left for, and what they say.
    RequestsFound(Vec<(String, Vec<u8>)>),
    Bound(anyhow::Result<Endpoint>),
    Frame {
        peer: PublicKey,
        frame: Frame,
    },
    Closed {
        peer: PublicKey,
        sid: u64,
        /// It broke by itself, as opposed to either side closing it.
        broken: bool,
    },
    Net(NetStatus),
    Progress {
        contact_id: String,
        message_id: String,
        done: u64,
        total: u64,
    },
    UploadEnded {
        key: String,
    },
    Download {
        peer: PublicKey,
        message_id: Option<String>,
        result: Received,
    },
}

struct Session {
    sid: u64,
    out: mpsc::UnboundedSender<Frame>,
    conn: Connection,
    /// Who opened this connection. Used to pick one when both dialed at once.
    initiator: PublicKey,
    /// When a frame last went either way.
    last_frame: Cell<Instant>,
    /// What they gave at the handshake to start an encrypted session with them.
    curve: [u8; 32],
    one_time: [u8; 32],
    /// Waiting for the encrypted session to exist.
    waiting: std::cell::RefCell<Vec<Frame>>,
    /// Asked for a new session once on this connection: no asking in circles.
    asked_new_session: Cell<bool>,
}

/// State shared with the `Core` handle for cheap synchronous reads.
#[derive(Clone)]
pub(crate) struct Shared {
    pub online: Arc<Mutex<HashSet<String>>>,
    pub net: Arc<Mutex<NetStatus>>,
    /// Group calls that can be joined: group id -> video.
    pub group_calls: Arc<Mutex<HashMap<String, bool>>>,
    /// The call we are in or being rung for, if any.
    pub call: Arc<Mutex<Option<CallSnapshot>>>,
}

pub(crate) struct Engine {
    db: Arc<Database>,
    identity: Identity,
    sealing: Sealing,
    key: SecretKey,
    me: PublicKey,
    /// The DHT, while there is an endpoint.
    records: Option<Records>,
    /// What we last published, and when.
    published: Option<(EndpointAddr, Instant)>,
    publishing: bool,
    next_request_poll: Option<Instant>,
    polling_requests: bool,
    /// Requests of ours waiting for an answer: when to leave them again, and how often we did.
    requests_out: HashMap<PublicKey, (Instant, usize)>,
    /// Contacts that do not have our current location yet.
    owe_accept: HashSet<PublicKey>,
    seen_notes: HashSet<[u8; 32]>,
    events: broadcast::Sender<CoreEvent>,
    shared: Shared,
    mode: NetMode,
    /// Down when there is no reason to hold anything.
    ep: Option<Endpoint>,
    binding: bool,
    /// The relays this endpoint was bound with on or off, to rebind when the setting changes.
    bound_with_relays: bool,
    device: Device,
    in_front: bool,
    /// When a message last went either way.
    last_activity: Option<Instant>,
    /// Up until then, whatever the plan says.
    awake_until: Option<Instant>,
    last_wake: Option<Instant>,
    next_check: Option<Instant>,
    /// Who we have a reason to reach, kept up to date when `wanted_dirty`.
    wanted: HashSet<PublicKey>,
    wanted_dirty: bool,
    chat_open: Option<PublicKey>,
    /// Whose connection broke by itself, and when.
    blips: HashMap<PublicKey, Instant>,
    /// Who to look up once, a few at a time.
    look_around: VecDeque<PublicKey>,
    /// Who got a few more tries since the last look around.
    tried_again: HashSet<PublicKey>,
    /// The wake-up last asked of the system.
    asked_wake: Option<Instant>,
    sessions: HashMap<PublicKey, Session>,
    dialing: HashSet<PublicKey>,
    /// Not before then, and how many tries failed in a row.
    backoff: HashMap<PublicKey, (Instant, u32)>,
    /// Peers that asked us not to show their last seen.
    hides_last_seen: HashSet<PublicKey>,
    typing_sent: HashMap<PublicKey, Instant>,
    next_sid: u64,
    tx: mpsc::UnboundedSender<Internal>,
    media: PathBuf,
    /// Outgoing file transfers running now (message ids).
    uploads: HashSet<String>,
    /// Incoming file transfers running now (shared with receive tasks).
    downloads: Arc<Mutex<HashSet<String>>>,
    /// How often a download failed its hash check (retried once).
    corrupt_retries: HashMap<String, u8>,
    call: Option<CallState>,
    group_calls: GroupCalls,
    media_in: MediaIn,
    video_inflight: Arc<std::sync::atomic::AtomicUsize>,
}

fn emit(events: &broadcast::Sender<CoreEvent>, ev: CoreEvent) {
    // No subscribers is fine.
    let _ = events.send(ev);
}

impl Engine {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run(
        db: Arc<Database>,
        identity: Identity,
        mode: NetMode,
        events: broadcast::Sender<CoreEvent>,
        shared: Shared,
        media: PathBuf,
        media_sink: MediaSink,
        mut cmds: mpsc::UnboundedReceiver<Cmd>,
    ) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let key = identity.person();
        let me = key.public();
        let sealing = match Sealing::load(db.clone()) {
            Ok(sealing) => sealing,
            Err(e) => {
                tracing::error!("the encryption state cannot be read: {e:#}");
                let _ = events.send(CoreEvent::Net(NetStatus::Offline));
                return;
            }
        };
        let mut engine = Engine {
            identity,
            sealing,
            records: None,
            published: None,
            publishing: false,
            next_request_poll: None,
            polling_requests: false,
            requests_out: HashMap::new(),
            owe_accept: HashSet::new(),
            seen_notes: HashSet::new(),
            media,
            uploads: HashSet::new(),
            downloads: Arc::new(Mutex::new(HashSet::new())),
            corrupt_retries: HashMap::new(),
            call: None,
            group_calls: HashMap::new(),
            media_in: MediaIn {
                active: ActiveCall::default(),
                sink: media_sink,
            },
            video_inflight: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            db,
            key,
            me,
            events,
            shared,
            mode,
            ep: None,
            binding: false,
            bound_with_relays: false,
            device: Device::default(),
            in_front: false,
            last_activity: None,
            awake_until: None,
            last_wake: None,
            next_check: None,
            wanted: HashSet::new(),
            wanted_dirty: true,
            chat_open: None,
            blips: HashMap::new(),
            look_around: VecDeque::new(),
            tried_again: HashSet::new(),
            asked_wake: None,
            sessions: HashMap::new(),
            dialing: HashSet::new(),
            backoff: HashMap::new(),
            hides_last_seen: HashSet::new(),
            typing_sent: HashMap::new(),
            next_sid: 0,
            tx,
        };

        // The first look around, once there is an endpoint.
        engine.look_around_all();
        engine.load_what_is_owed();
        engine.db.forget_old_seen(now_ms() - SEEN_KEPT_MS);
        loop {
            engine.reconcile();
            let sleep = tokio::time::sleep_until(engine.next_deadline());
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(Cmd::Shutdown(done)) => {
                        for (_, s) in engine.sessions.drain() {
                            s.conn.close(0u32.into(), b"bye");
                        }
                        if let Some(ep) = engine.ep.take() {
                            let _ = tokio::time::timeout(Duration::from_secs(2), ep.close()).await;
                        }
                        let _ = done.send(());
                        return;
                    }
                    Some(cmd) => engine.handle_cmd(cmd),
                    None => {
                        if let Some(ep) = engine.ep.take() {
                            ep.close().await;
                        }
                        return;
                    }
                },
                Some(ev) = rx.recv() => engine.handle_internal(ev),
                () = sleep => engine.call_tick(),
            }
        }
    }

    // ---- how much to keep up ----

    fn conditions(&self) -> Conditions {
        Conditions {
            device: self.device,
            mode: self.settings().speed_mode,
            in_front: self.in_front,
            busy: self.call.is_some()
                || !self.uploads.is_empty()
                || !self
                    .downloads
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .is_empty(),
            since_activity: self.last_activity.map(|at| at.elapsed()),
        }
    }

    fn plan(&self) -> Plan {
        power::plan(&self.conditions())
    }

    /// Makes what is so match what should be so. Runs after everything that happens.
    fn reconcile(&mut self) {
        let now = Instant::now();
        if self.wanted_dirty {
            self.refresh_wanted();
        }
        let plan = self.plan();
        self.blips
            .retain(|_, at| now.duration_since(*at) < BLIP_LASTS);

        // A planned check: up for a moment, and a look around.
        match plan.check_every {
            Some(every) => {
                let due = *self.next_check.get_or_insert(now + every);
                if due <= now {
                    self.next_check = Some(now + every);
                    self.wake_up(now);
                }
            }
            None => self.next_check = None,
        }

        let awake = self.awake_until.is_some_and(|until| until > now);
        let relays_changed = self.ep.is_some()
            && matches!(self.mode, NetMode::Internet)
            && self.settings().use_relays != self.bound_with_relays;
        if relays_changed {
            self.take_down();
        }
        if plan.endpoint_up || awake {
            if self.ep.is_none() && !self.binding {
                self.start_binding();
            }
        } else if self.ep.is_some()
            && self.sessions.is_empty()
            && self.dialing.is_empty()
            && !plan_keeps_busy(&self.conditions())
        {
            self.take_down();
        }

        self.close_idle(&plan, now);
        self.dial_due(now);
        self.publish_due(now);
        self.poll_requests_due(&plan, now);
        self.leave_requests_due(now);
        self.ask_for_wake(&plan, now);
    }

    fn wake_up(&mut self, now: Instant) {
        self.awake_until = Some(now + AWAKE_FOR);
        self.last_wake = Some(now);
        self.look_around_all();
    }

    fn start_binding(&mut self) {
        self.binding = true;
        self.set_net(NetStatus::Connecting);
        if self.identity.network_due(now_ms()) {
            let _ = self.identity.save(&self.db);
        }
        let (key, mode, tx) = (self.identity.network(), self.mode.clone(), self.tx.clone());
        let settings = self.settings();
        self.bound_with_relays = settings.use_relays;
        tokio::spawn(async move {
            let _ = tx.send(Internal::Bound(bind(key, mode, settings).await));
        });
    }

    fn on_bound(&mut self, result: anyhow::Result<Endpoint>) {
        self.binding = false;
        match result {
            Ok(ep) => {
                spawn_accept_loop(ep.clone(), self.tx.clone());
                spawn_online_watch(
                    ep.clone(),
                    self.tx.clone(),
                    matches!(self.mode, NetMode::Local(_)),
                );
                self.ep = Some(ep);
                self.records = match &self.mode {
                    NetMode::Local(local) => Some(Records::Local(local.clone())),
                    NetMode::Internet => match Records::join() {
                        Ok(records) => Some(records),
                        Err(e) => {
                            tracing::warn!("could not join the DHT: {e:#}");
                            None
                        }
                    },
                };
                self.published = None;
                self.next_request_poll = Some(Instant::now());
            }
            Err(e) => {
                tracing::warn!("bind failed: {e:#}");
                self.set_net(NetStatus::Offline);
                // Not in a tight loop: until something else happens, or this.
                self.awake_until = None;
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    let _ = tx.send(Internal::Net(NetStatus::Offline));
                });
            }
        }
    }

    /// Nothing held: the radio can rest.
    fn take_down(&mut self) {
        let Some(ep) = self.ep.take() else { return };
        self.records = None;
        self.published = None;
        self.publishing = false;
        self.polling_requests = false;
        let peers: Vec<PublicKey> = self.sessions.keys().copied().collect();
        for peer in peers {
            self.close_session(peer);
        }
        self.dialing.clear();
        if self.device.network != power::Network::None {
            // Resting is not being offline: nothing to worry anyone with.
            self.set_net(NetStatus::Online);
        }
        tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(2), ep.close()).await;
        });
    }

    fn close_session(&mut self, peer: PublicKey) {
        if let Some(s) = self.sessions.get(&peer) {
            let sid = s.sid;
            s.conn.close(0u32.into(), REASON_IDLE);
            self.on_closed(peer, sid);
        }
    }

    /// Connections without traffic go, unless something is going on there.
    fn close_idle(&mut self, plan: &Plan, now: Instant) {
        let Some(limit) = plan.idle_close else { return };
        let idle: Vec<PublicKey> = self
            .sessions
            .iter()
            .filter(|(peer, s)| {
                Some(**peer) != self.chat_open
                    && !self.wanted.contains(*peer)
                    && now.duration_since(s.last_frame.get()) >= limit
            })
            .map(|(peer, _)| *peer)
            .collect();
        for peer in idle {
            self.close_session(peer);
        }
    }

    /// Who we have a reason to reach.
    fn refresh_wanted(&mut self) {
        self.wanted_dirty = false;
        let mut wanted: HashSet<PublicKey> = self
            .db
            .peers_owed()
            .iter()
            .filter_map(|id| crate::identity::parse_key(id))
            .collect();
        wanted.extend(self.chat_open);
        wanted.extend(self.blips.keys());
        wanted.extend(self.owe_accept.iter().copied());
        if let Some(c) = &self.call
            && c.connected.is_none()
            && c.outgoing
        {
            wanted.extend(c.members.iter().copied());
        }
        wanted.remove(&self.me);
        self.wanted = wanted;
    }

    /// Everyone we could talk to, once: they may have something for us.
    fn look_around_all(&mut self) {
        let in_groups: HashSet<String> = self.db.group_peer_ids().into_iter().collect();
        self.look_around.clear();
        self.tried_again.clear();
        for c in self.db.contacts_in(&[
            ContactState::Accepted,
            ContactState::PendingOut,
            ContactState::GroupPeer,
        ]) {
            if c.state == ContactState::GroupPeer && !in_groups.contains(&c.id) {
                continue;
            }
            if let Some(peer) = crate::identity::parse_key(&c.id) {
                self.look_around.push_back(peer);
            }
        }
    }

    /// When there is something to do next, whatever else happens first.
    fn next_deadline(&self) -> Instant {
        let now = Instant::now();
        let plan = self.plan();
        let mut next = now + Duration::from_secs(3600);
        let mut sooner = |at: Instant| {
            if at < next {
                next = at;
            }
        };
        if self.call.is_some() {
            sooner(now + CALL_TICK);
        }
        if let Some(at) = self.next_check {
            sooner(at);
        }
        if let Some(at) = self.awake_until.filter(|at| *at > now) {
            sooner(at);
        }
        // A recent chat turns into a resting one.
        if let Some(at) = self.last_activity {
            let ends = at + power::RECENT_ACTIVITY;
            if ends > now {
                sooner(ends);
            }
        }
        if let Some(limit) = plan.idle_close {
            for s in self.sessions.values() {
                sooner(s.last_frame.get() + limit);
            }
        }
        for at in self.blips.values() {
            sooner(*at + BLIP_LASTS);
        }
        if self.ep.is_some() {
            for peer in &self.wanted {
                if !self.sessions.contains_key(peer) && !self.dialing.contains(peer) {
                    sooner(self.backoff.get(peer).map_or(now, |(at, _)| *at));
                }
            }
            if let Some(at) = self.next_request_poll {
                sooner(at);
            }
            for (at, _) in self.requests_out.values() {
                sooner(*at);
            }
            if let Some((_, at)) = &self.published {
                sooner(*at + PUBLISH_EVERY);
            }
        }
        next
    }

    /// Timers stop while the system sleeps. With nothing held the system wakes us for the next
    /// thing to do: a check, or trying someone we have something for.
    fn ask_for_wake(&mut self, plan: &Plan, now: Instant) {
        if plan.endpoint_up {
            self.asked_wake = None;
            return;
        }
        let mut next = self.next_check;
        if self.device.network != power::Network::None {
            for peer in &self.wanted {
                if let Some((at, _)) = self.backoff.get(peer) {
                    next = Some(next.map_or(*at, |n| n.min(*at)));
                }
            }
        }
        let Some(next) = next else { return };
        let moved = self
            .asked_wake
            .is_none_or(|asked| asked.max(next).duration_since(asked.min(next)) > WAKE_SLACK);
        if moved {
            self.asked_wake = Some(next);
            let ms = next.saturating_duration_since(now).as_millis() as u64;
            emit(&self.events, CoreEvent::WakeIn { ms });
        }
    }

    // ---- being found ----

    /// Contacts who still have to hear from us, and requests still unanswered, after a restart.
    fn load_what_is_owed(&mut self) {
        let now = Instant::now();
        for c in self
            .db
            .contacts_in(&[ContactState::Accepted, ContactState::PendingOut])
        {
            let Some(peer) = crate::identity::parse_key(&c.id) else {
                continue;
            };
            let keys = self.db.contact_keys(&c.id).unwrap_or_default();
            match c.state {
                ContactState::Accepted
                    if !keys.told && c.kind == crate::model::ContactKind::Person =>
                {
                    self.owe_accept.insert(peer);
                }
                ContactState::PendingOut if keys.invite.is_some() => {
                    self.requests_out.insert(peer, (now, 0));
                }
                _ => {}
            }
        }
        self.wanted_dirty = true;
    }

    /// Where we can be reached right now.
    fn current_addr(&self) -> Option<EndpointAddr> {
        let ep = self.ep.as_ref()?;
        if matches!(self.mode, NetMode::Local(_)) {
            let mut addr = EndpointAddr::new(ep.id());
            for sock in ep.bound_sockets() {
                addr = addr.with_ip_addr(sock);
            }
            return Some(addr);
        }
        Some(ep.addr())
    }

    /// Writes where we are to the DHT when that changed, or after a while. For contacts under
    /// the location secret, and for each group under what its members share.
    fn publish_due(&mut self, now: Instant) {
        if self.publishing {
            return;
        }
        let (Some(records), Some(addr)) = (self.records.clone(), self.current_addr()) else {
            return;
        };
        if addr.is_empty() {
            return;
        }
        let due = self
            .published
            .as_ref()
            .is_none_or(|(last, at)| *last != addr || now.duration_since(*at) >= PUBLISH_EVERY);
        if !due {
            return;
        }
        self.publishing = true;
        let mut secrets = vec![self.identity.location];
        for gid in self.db.active_groups() {
            if let Some(secret) = self.group_secret(&gid) {
                secrets.push(location::group_secret(&secret, &self.me));
            }
        }
        let person = self.key.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let at = now_ms();
            let record = LocationRecord::from_addr(&addr, at);
            let mut any = false;
            for secret in secrets {
                let sealed = location::seal(&secret, record.clone(), &person);
                any |= location::publish(&records, &secret, &sealed, at).await;
            }
            let _ = tx.send(Internal::Published(any.then_some(addr)));
        });
    }

    fn poll_requests_due(&mut self, plan: &Plan, now: Instant) {
        if self.polling_requests || self.next_request_poll.is_none_or(|at| at > now) {
            return;
        }
        let Some(records) = self.records.clone() else {
            return;
        };
        let every = if matches!(self.mode, NetMode::Local(_)) {
            REQUESTS_LOCAL
        } else if plan.level == power::Level::InUse {
            REQUESTS_IN_USE
        } else {
            REQUESTS_REACHABLE
        };
        self.next_request_poll = Some(now + every);
        let wall = now_ms();
        let invites: Vec<MyInvite> = self
            .db
            .invites()
            .into_iter()
            .filter(|i| i.open_for_requests(wall))
            .collect();
        self.polling_requests = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let today = records::day_of(wall);
            let mut reads = tokio::task::JoinSet::new();
            for invite in invites {
                for day in [today, today.saturating_sub(1)] {
                    for slot in 0..requests::SLOTS {
                        let (records, id) = (records.clone(), invite.id.clone());
                        let seed = requests::slot_seed(&invite.drop_secret, day, slot);
                        reads.spawn(async move { (id, records.get(&seed).await) });
                    }
                }
            }
            let mut found = Vec::new();
            while let Some(read) = reads.join_next().await {
                if let Ok((id, Some(note))) = read {
                    found.push((id, note));
                }
            }
            let _ = tx.send(Internal::RequestsFound(found));
        });
    }

    fn on_requests_found(&mut self, found: Vec<(String, Vec<u8>)>) {
        self.polling_requests = false;
        let invites = self.db.invites();
        for (id, note) in found {
            if !self.seen_notes.insert(*blake3::hash(&note).as_bytes()) {
                continue;
            }
            let Some(invite) = invites.iter().find(|i| i.id == id) else {
                continue;
            };
            if let Some(request) = requests::open(invite, &note) {
                self.on_request(invite.clone(), request);
            }
        }
    }

    /// Someone left us a note.
    fn on_request(&mut self, mut invite: MyInvite, request: Request) {
        let Ok(peer) = PublicKey::from_bytes(&request.person) else {
            return;
        };
        if peer == self.me || !invite.open_for_requests(now_ms()) {
            return;
        }
        let id = key_to_string(&peer);
        let name = clean_name(&request.name);
        let state = self.db.contact(&id).map(|c| c.state);
        if state == Some(ContactState::Blocked) {
            return;
        }
        let met = invite.met_token.is_some() && invite.met_token == request.met_token;
        let mut keys = self.db.contact_keys(&id).unwrap_or_default();
        keys.location = Some(request.location);
        keys.via.clone_from(&invite.label);
        if met {
            keys.trust = Trust::MetInPerson;
        }
        match state {
            // They asked again, so they have not heard: repeat the answer.
            Some(ContactState::Accepted) => {
                self.db.set_contact_keys(&id, &keys);
                self.owe_accept.insert(peer);
                self.wanted_dirty = true;
            }
            // We both asked, or we met: that is mutual consent.
            Some(ContactState::PendingOut) => {
                keys.invite = None;
                keys.told = false;
                self.db.set_contact_keys(&id, &keys);
                self.requests_out.remove(&peer);
                self.become_accepted(peer, &name);
                self.owe_accept.insert(peer);
            }
            _ if met => {
                keys.told = false;
                let _ = self
                    .db
                    .upsert_contact(&id, ContactState::Accepted, Some(&name), None);
                self.db.set_contact_keys(&id, &keys);
                self.become_accepted(peer, &name);
                self.owe_accept.insert(peer);
            }
            Some(ContactState::PendingIn) => {
                keys.greeting = request
                    .greeting
                    .chars()
                    .take(requests::MAX_GREETING)
                    .collect();
                self.db.set_contact_keys(&id, &keys);
                self.db.set_their_name(&id, &name);
            }
            None | Some(ContactState::GroupPeer) => {
                keys.greeting = request
                    .greeting
                    .chars()
                    .take(requests::MAX_GREETING)
                    .collect();
                if self
                    .db
                    .upsert_contact(&id, ContactState::PendingIn, Some(&name), None)
                    .is_ok()
                {
                    self.db.set_contact_keys(&id, &keys);
                    emit(
                        &self.events,
                        CoreEvent::RequestReceived {
                            contact_id: id,
                            name,
                        },
                    );
                    emit(&self.events, CoreEvent::ContactsChanged);
                }
            }
            Some(ContactState::Blocked) => {}
        }
        invite.uses += 1;
        self.db.save_invite(&invite);
        self.wanted_dirty = true;
    }

    /// Leaves our notes for whoever has not answered yet.
    fn leave_requests_due(&mut self, now: Instant) {
        let Some(records) = self.records.clone() else {
            return;
        };
        let due: Vec<PublicKey> = self
            .requests_out
            .iter()
            .filter(|(_, (at, _))| *at <= now)
            .map(|(peer, _)| *peer)
            .collect();
        for peer in due {
            let id = key_to_string(&peer);
            let keys = self.db.contact_keys(&id).unwrap_or_default();
            let Some(invite) = keys.invite else {
                self.requests_out.remove(&peer);
                continue;
            };
            let tries = self.requests_out.get(&peer).map_or(0, |(_, n)| *n);
            let delay = REQUEST_AGAIN[tries.min(REQUEST_AGAIN.len() - 1)];
            self.requests_out.insert(peer, (now + delay, tries + 1));
            let request = Request {
                person: *self.me.as_bytes(),
                name: self.settings().display_name,
                greeting: keys.greeting,
                location: self.identity.location,
                met_token: invite.met_token,
                at: now_ms(),
            };
            let Some(sealed) = requests::seal(&invite, &request, &self.key) else {
                tracing::warn!("an invite with a key that cannot be used");
                self.requests_out.remove(&peer);
                continue;
            };
            let seed = requests::slot_seed(
                &invite.drop_secret,
                records::day_of(request.at),
                requests::random_slot(),
            );
            let records = records.clone();
            tokio::spawn(async move {
                records.put(&seed, &sealed).await;
            });
        }
    }

    /// A new location, and a new address if asked. Whoever knew the old ones cannot follow, and
    /// everyone else gets told at the next connection, which we go and make.
    fn rotate(&mut self, network: bool) {
        let now = now_ms();
        self.identity.rotate_location(now);
        if network {
            self.identity.rotate_network(now);
        }
        if let Err(e) = self.identity.save(&self.db) {
            tracing::error!("could not save the new address: {e:#}");
        }
        for c in self.db.contacts_in(&[ContactState::Accepted]) {
            if c.kind != crate::model::ContactKind::Person {
                continue;
            }
            let Some(peer) = crate::identity::parse_key(&c.id) else {
                continue;
            };
            let mut keys = self.db.contact_keys(&c.id).unwrap_or_default();
            keys.told = false;
            self.db.set_contact_keys(&c.id, &keys);
            self.owe_accept.insert(peer);
            self.backoff.remove(&peer);
        }
        self.published = None;
        self.wanted_dirty = true;
        if network {
            self.take_down();
        } else {
            let peers: Vec<PublicKey> = self.sessions.keys().copied().collect();
            for peer in peers {
                self.tell_where_we_are(peer);
            }
        }
    }

    /// They get to know how to find us.
    fn tell_where_we_are(&mut self, peer: PublicKey) {
        let name = self.settings().display_name;
        let location = self.identity.location;
        if self.send(peer, Frame::Accept { name, location }) {
            let id = key_to_string(&peer);
            let mut keys = self.db.contact_keys(&id).unwrap_or_default();
            keys.told = true;
            self.db.set_contact_keys(&id, &keys);
            self.owe_accept.remove(&peer);
            self.wanted_dirty = true;
        }
    }

    /// What the connection screen shows.
    fn report(&self) -> crate::model::ConnectionReport {
        use crate::model::{ConnectionReport, PeerPath};
        let plan = self.plan();
        let net = self.ep.as_ref().and_then(|ep| {
            use iroh::Watcher as _;
            ep.net_report().get()
        });
        let addr = self.ep.as_ref().map(Endpoint::addr);
        let peers = self
            .sessions
            .iter()
            .map(|(peer, s)| {
                let selected = s
                    .conn
                    .paths()
                    .iter()
                    .find(|path| path.is_selected())
                    .map(|path| (path.is_ip(), path.rtt().as_millis() as u64));
                PeerPath {
                    contact_id: key_to_string(peer),
                    direct: selected.is_some_and(|(ip, _)| ip),
                    rtt_ms: selected.map(|(_, rtt)| rtt),
                }
            })
            .collect();
        ConnectionReport {
            network: self.device.network,
            level: plan.level,
            endpoint_up: self.ep.is_some(),
            reachable_direct: addr
                .as_ref()
                .is_some_and(|a| a.ip_addrs().any(|ip| !is_private(ip.ip()))),
            on_relay: addr
                .as_ref()
                .is_some_and(|a| a.relay_urls().next().is_some()),
            ipv6: net.as_ref().is_some_and(|n| n.udp_v6),
            hard_nat: net.as_ref().and_then(|n| n.mapping_varies_by_dest_ipv4),
            peers,
        }
    }

    fn set_net(&self, status: NetStatus) {
        let mut cur = self.shared.net.lock().unwrap_or_else(|p| p.into_inner());
        if *cur != status {
            *cur = status;
            emit(&self.events, CoreEvent::Net(status));
        }
    }

    fn settings(&self) -> Settings {
        self.db.settings()
    }

    // ---- commands ----

    fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Dial(peer) => {
                self.note_activity();
                self.backoff.remove(&peer);
                if !self.sessions.contains_key(&peer) {
                    self.look_around.push_front(peer);
                }
                self.wanted_dirty = true;
            }
            Cmd::SendRequest(peer) => {
                self.note_activity();
                self.requests_out.insert(peer, (Instant::now(), 0));
            }
            Cmd::Rotate { network } => self.rotate(network),
            Cmd::Device(device) => {
                let changed = device.network != self.device.network;
                self.device = device;
                if changed {
                    self.on_network_change();
                }
            }
            Cmd::NetworkChanged => self.on_network_change(),
            Cmd::InFront(in_front) => {
                if in_front && !self.in_front {
                    self.backoff.clear();
                    self.look_around_all();
                }
                self.in_front = in_front;
            }
            Cmd::ChatOpen(peer) => {
                self.chat_open = peer;
                if let Some(peer) = peer {
                    self.backoff.remove(&peer);
                }
                self.wanted_dirty = true;
            }
            Cmd::Wake => self.wake_up(Instant::now()),
            Cmd::RadioActive => {
                let now = Instant::now();
                let rested = self
                    .last_wake
                    .is_none_or(|at| now.duration_since(at) >= RADIO_LOOK_EVERY);
                if !self.plan().endpoint_up && rested {
                    self.wake_up(now);
                }
            }
            Cmd::Report(answer) => {
                let _ = answer.send(self.report());
            }
            Cmd::SendOne {
                contact,
                message_id,
            } => {
                self.note_activity();
                self.backoff.remove(&contact);
                self.wanted_dirty = true;
                if self.sessions.contains_key(&contact)
                    && let Some(m) = self.db.message(&message_id)
                {
                    self.send_message(contact, &m);
                }
            }
            Cmd::Accepted(peer) => {
                self.note_activity();
                self.requests_out.remove(&peer);
                let id = key_to_string(&peer);
                let mut keys = self.db.contact_keys(&id).unwrap_or_default();
                keys.invite = None;
                keys.told = false;
                self.db.set_contact_keys(&id, &keys);
                if self.sessions.contains_key(&peer) {
                    self.tell_where_we_are(peer);
                    self.on_accepted_session(peer);
                } else {
                    self.owe_accept.insert(peer);
                    self.backoff.remove(&peer);
                    self.look_around.push_front(peer);
                    self.wanted_dirty = true;
                }
            }
            Cmd::ReadReceipts(peer) => self.send_read_receipts(peer),
            Cmd::Typing(peer) => {
                if !self.settings().typing_indicators || !self.sessions.contains_key(&peer) {
                    return;
                }
                let now = Instant::now();
                if self
                    .typing_sent
                    .get(&peer)
                    .is_some_and(|t| now.duration_since(*t) < TYPING_THROTTLE)
                {
                    return;
                }
                self.typing_sent.insert(peer, now);
                self.send(peer, Frame::Typing);
            }
            Cmd::Disconnect(peer) => {
                if let Some(s) = self.sessions.remove(&peer) {
                    s.conn.close(0u32.into(), b"bye");
                }
                self.look_around.retain(|p| *p != peer);
                self.blips.remove(&peer);
                self.owe_accept.remove(&peer);
                self.requests_out.remove(&peer);
                self.wanted_dirty = true;
                self.set_online(peer, false, None);
            }
            Cmd::SettingsChanged => {
                let peers: Vec<_> = self.sessions.keys().copied().collect();
                for p in peers {
                    self.send_hello(p);
                    self.send_profile(p);
                }
            }
            Cmd::RetryFile {
                contact,
                message_id,
            } => {
                let _ = std::fs::remove_file(files::partial_path(&self.media, &message_id));
                self.corrupt_retries.remove(&message_id);
                if let Some(m) =
                    self.db
                        .set_transfer(&message_id, TransferState::Transferring, None)
                {
                    emit(&self.events, CoreEvent::MessageUpdated(m));
                }
                self.want_file(contact, &message_id);
            }
            Cmd::CallInvite {
                target,
                call,
                video,
            } => {
                self.note_activity();
                self.call_invite(target, call, video);
                self.wanted_dirty = true;
            }
            Cmd::CallJoin { group } => self.call_join(group),
            Cmd::CallAnswer { accept } => self.call_answer(accept),
            Cmd::CallEnd => self.call_hangup(),
            Cmd::CallAudio { ts, data } => {
                if let Some((call, conns)) = self.call_conns() {
                    for conn in conns {
                        calls::send_audio(&conn, call, ts, &data);
                    }
                }
            }
            Cmd::CallVideo { ts, key, data } => {
                if let Some((call, conns)) = self.call_conns() {
                    for conn in conns {
                        calls::send_video(
                            conn,
                            call,
                            ts,
                            key,
                            data.clone(),
                            self.video_inflight.clone(),
                        );
                    }
                }
            }
            Cmd::CallVideoOn(on) => self.call_video_on(on),
            Cmd::SendGroup { group, message_id } => {
                self.note_activity();
                self.wanted_dirty = true;
                self.send_group_message(&group, &message_id);
            }
            Cmd::GroupPublish {
                state,
                sig,
                names,
                to,
            } => {
                self.refresh_group_messages(&state.id);
                for p in &to {
                    if !self.sessions.contains_key(p) {
                        self.look_around.push_back(*p);
                    }
                }
                self.published = None;
                let Some(secret) = self.group_secret(&state.id) else {
                    return;
                };
                for p in to {
                    self.send(
                        p,
                        Frame::GroupUpdate {
                            state: state.clone(),
                            sig: sig.clone(),
                            names: names.clone(),
                            secret,
                        },
                    );
                }
            }
            Cmd::GroupLeave { group, admin } => {
                self.wanted_dirty = true;
                self.send(admin, Frame::GroupLeave { group });
            }
            Cmd::Shutdown(_) => unreachable!("handled in run loop"),
        }
    }

    // ---- dialing ----

    fn note_activity(&mut self) {
        self.last_activity = Some(Instant::now());
    }

    fn on_network_change(&mut self) {
        if let Some(ep) = self.ep.clone() {
            // Android tells nobody but us: without this, connections hang until they time out.
            tokio::spawn(async move { ep.network_change().await });
        }
        // Everything may work again now.
        self.backoff.clear();
        if self.device.network == power::Network::None {
            self.set_net(NetStatus::Offline);
        } else {
            self.look_around_all();
        }
    }

    /// Dials whoever we have a reason to reach and may try again by now, and a few from the look
    /// around.
    fn dial_due(&mut self, now: Instant) {
        let Some(ep) = self.ep.clone() else { return };
        if self.device.network == power::Network::None {
            return;
        }
        let due: Vec<PublicKey> = self
            .wanted
            .iter()
            .filter(|peer| {
                !self.sessions.contains_key(*peer)
                    && !self.dialing.contains(*peer)
                    && self.backoff.get(*peer).is_none_or(|(at, _)| *at <= now)
            })
            .copied()
            .collect();
        for peer in due {
            self.dial(&ep, peer);
        }
        while self.dialing.len() < LOOK_AROUND_DIALS {
            let Some(peer) = self.look_around.pop_front() else {
                break;
            };
            if !self.sessions.contains_key(&peer) {
                self.dial(&ep, peer);
            }
        }
    }

    fn dial(&mut self, ep: &Endpoint, peer: PublicKey) {
        if peer == self.me || !self.dialing.insert(peer) {
            return;
        }
        let id = key_to_string(&peer);
        let keys = self.db.contact_keys(&id).unwrap_or_default();
        // What they can be looked up with: what they told us, and what we share in groups.
        let mut secrets: Vec<[u8; 32]> = keys.location.into_iter().collect();
        for gid in self.db.groups_with_member(&id) {
            if let Some(secret) = self.group_secret(&gid) {
                secrets.push(location::group_secret(&secret, &peer));
            }
        }
        let (ep, tx, records) = (ep.clone(), self.tx.clone(), self.records.clone());
        tokio::spawn(async move {
            let attempt = async {
                let mut addr = None;
                if let Some(records) = &records {
                    for secret in &secrets {
                        let found = location::lookup(records, secret, &peer, now_ms()).await;
                        if let Some(record) = found.and_then(|r| r.to_addr()) {
                            addr = Some(record);
                            break;
                        }
                    }
                }
                // Not to be found: the last address we knew, for the same Wi-Fi without internet.
                let addr = addr
                    .or_else(|| {
                        keys.network
                            .and_then(|n| iroh::EndpointId::from_bytes(&n).ok())
                            .map(EndpointAddr::new)
                    })
                    .ok_or_else(|| anyhow::anyhow!("nowhere to look"))?;
                let conn = ep.connect(addr, ALPN).await?;
                let (send, recv) = conn.open_bi().await?;
                anyhow::Ok((conn, send, recv))
            };
            let ev = match tokio::time::timeout(DIAL_TIMEOUT, attempt).await {
                Ok(Ok((conn, send, recv))) => Internal::Connected {
                    conn,
                    send,
                    recv,
                    expect: Some(peer),
                },
                Ok(Err(e)) => {
                    tracing::debug!("dial {} failed: {e:#}", peer.fmt_short());
                    Internal::DialFailed(peer)
                }
                Err(_) => Internal::DialFailed(peer),
            };
            let _ = tx.send(ev);
        });
    }

    /// A connection: each side now proves who it is.
    fn start_auth(
        &mut self,
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        expect: Option<PublicKey>,
    ) {
        let (curve, one_time) = self.sealing.keys_to_give();
        let (person, tx) = (self.key.clone(), self.tx.clone());
        tokio::spawn(async move {
            let (mut send, mut recv) = (send, recv);
            let them = tokio::time::timeout(
                AUTH_TIMEOUT,
                prove_and_check(&conn, &mut send, &mut recv, &person, curve, one_time),
            )
            .await;
            match them {
                Ok(Ok((peer, curve, one_time))) if expect.is_none_or(|wanted| wanted == peer) => {
                    let _ = tx.send(Internal::NewConn {
                        conn,
                        send,
                        recv,
                        outgoing: expect.is_some(),
                        peer,
                        curve,
                        one_time,
                    });
                }
                other => {
                    if let Ok(Err(e)) = other {
                        tracing::debug!("no proof from the other side: {e:#}");
                    }
                    conn.close(3u32.into(), b"who are you");
                    if let Some(wanted) = expect {
                        let _ = tx.send(Internal::DialFailed(wanted));
                    }
                }
            }
        });
    }

    fn bump_backoff(&mut self, peer: PublicKey) {
        let failures = self.backoff.get(&peer).map_or(1, |(_, n)| n + 1);
        let delay = if self.plan().level == power::Level::InUse {
            retry::delay_in_use(failures)
        } else {
            retry::delay_after(failures)
        };
        self.backoff
            .insert(peer, (Instant::now() + delay, failures));
    }

    // ---- internal events ----

    fn handle_internal(&mut self, ev: Internal) {
        match ev {
            Internal::Connected {
                conn,
                send,
                recv,
                expect,
            } => self.start_auth(conn, send, recv, expect),
            Internal::NewConn {
                conn,
                send,
                recv,
                outgoing,
                peer,
                curve,
                one_time,
            } => self.on_new_conn(conn, send, recv, outgoing, peer, curve, one_time),
            Internal::Published(addr) => {
                self.publishing = false;
                // If nothing took it: again in a while, not in a loop.
                let now = Instant::now();
                let addr = addr.or_else(|| self.current_addr());
                if let Some(addr) = addr {
                    self.published = Some((addr, now));
                }
            }
            Internal::RequestsFound(found) => self.on_requests_found(found),
            Internal::DialFailed(peer) => {
                self.dialing.remove(&peer);
                self.bump_backoff(peer);
                // What we looked up may be from before they came back: worth a few more tries,
                // once per look around.
                if self.tried_again.insert(peer) {
                    self.blips.insert(peer, Instant::now());
                    self.wanted_dirty = true;
                }
            }
            Internal::Bound(result) => self.on_bound(result),
            Internal::Frame { peer, frame } => {
                if let Some(s) = self.sessions.get(&peer) {
                    s.last_frame.set(Instant::now());
                }
                self.on_wire_frame(peer, frame);
            }
            Internal::Closed { peer, sid, broken } => {
                let current = self.sessions.get(&peer).map(|s| s.sid) == Some(sid);
                self.on_closed(peer, sid);
                if broken && current {
                    self.on_broken(peer);
                }
            }
            Internal::Net(status) => self.set_net(status),
            Internal::Progress {
                contact_id,
                message_id,
                done,
                total,
            } => emit(
                &self.events,
                CoreEvent::Transfer {
                    contact_id,
                    message_id,
                    done,
                    total,
                },
            ),
            Internal::UploadEnded { key } => {
                self.uploads.remove(&key);
                self.note_activity();
            }
            Internal::Download {
                peer,
                message_id,
                result,
            } => {
                if let Some(id) = message_id {
                    self.on_download_ended(peer, &id, result);
                }
            }
        }
    }

    fn on_download_ended(&mut self, peer: PublicKey, id: &str, result: Received) {
        match result {
            Received::Done(path) => {
                self.corrupt_retries.remove(id);
                if let Some(m) =
                    self.db
                        .set_transfer(id, TransferState::Done, Some(&path.to_string_lossy()))
                {
                    emit(&self.events, CoreEvent::MessageUpdated(m));
                }
                if let Some(mid) = id_to_bytes(id) {
                    self.send(
                        peer,
                        Frame::Receipt {
                            kind: ReceiptKind::Delivered,
                            ids: vec![mid],
                        },
                    );
                }
            }
            Received::Corrupt => {
                let tries = self.corrupt_retries.entry(id.to_string()).or_insert(0);
                *tries += 1;
                if *tries <= 1 {
                    self.want_file(peer, id);
                } else if let Some(m) = self.db.set_transfer(id, TransferState::Failed, None) {
                    emit(&self.events, CoreEvent::MessageUpdated(m));
                }
            }
            // The rest arrives when the sender re-offers on the next session,
            // or right away if this session is still up.
            Received::Interrupted => self.want_file(peer, id),
            Received::Rejected => {}
        }
    }

    /// Asks the peer for (the rest of) an incoming file.
    fn want_file(&self, peer: PublicKey, id: &str) {
        let Some(mid) = id_to_bytes(id) else { return };
        if self
            .downloads
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(id)
        {
            return;
        }
        let offset = std::fs::metadata(files::partial_path(&self.media, id))
            .map(|m| m.len())
            .unwrap_or(0);
        self.send(peer, Frame::FileWant { id: mid, offset });
    }

    fn start_upload(&mut self, peer: PublicKey, m: &Message, offset: u64) {
        let Some(att) = &m.attachment else { return };
        let (Some(path), Some(mid)) = (att.path.clone(), id_to_bytes(&m.id)) else {
            return;
        };
        let Some(conn) = self.sessions.get(&peer).map(|s| s.conn.clone()) else {
            return;
        };
        let upload_key = format!("{}:{}", m.id, key_to_string(&peer));
        if offset > att.size || !self.uploads.insert(upload_key.clone()) {
            return;
        }
        let tx = self.tx.clone();
        let (contact_id, message_id, size) = (m.contact_id.clone(), m.id.clone(), att.size);
        tokio::spawn(async move {
            let progress_tx = tx.clone();
            let (c, i) = (contact_id.clone(), message_id.clone());
            let result = files::upload(conn, path.into(), mid, offset, size, move |done, total| {
                let _ = progress_tx.send(Internal::Progress {
                    contact_id: c.clone(),
                    message_id: i.clone(),
                    done,
                    total,
                });
            })
            .await;
            if let Err(e) = result {
                tracing::debug!("upload {message_id} stopped: {e:#}");
            }
            let _ = tx.send(Internal::UploadEnded { key: upload_key });
        });
    }

    /// What came off the wire: only what is sealed counts.
    fn on_wire_frame(&mut self, peer: PublicKey, frame: Frame) {
        let id = key_to_string(&peer);
        match frame {
            Frame::Sealed { kind, bytes } => {
                let Some(curve) = self.sessions.get(&peer).map(|s| s.curve) else {
                    return;
                };
                match self.sealing.open(&id, &curve, kind, &bytes) {
                    Ok(plain) => {
                        self.flush_waiting(peer);
                        match postcard::from_bytes::<Frame>(&plain) {
                            Ok(inner) => self.on_opened_frame(peer, inner),
                            // From a newer version: nothing to do with it, and not fatal.
                            Err(e) => tracing::debug!("a frame we do not know: {e}"),
                        }
                    }
                    Err(OpenError::Invalid) => {
                        tracing::debug!("{} sent something invalid", peer.fmt_short());
                    }
                    Err(_) => {
                        // They have a session we lost, or the other way around.
                        let asked = self
                            .sessions
                            .get(&peer)
                            .is_none_or(|s| s.asked_new_session.replace(true));
                        if !asked {
                            self.send_plain(peer, Frame::NeedSession);
                        }
                    }
                }
            }
            Frame::NeedSession => {
                let keys = self.sessions.get(&peer).map(|s| (s.curve, s.one_time));
                let Some((curve, one_time)) = keys else {
                    return;
                };
                if self.sealing.start_session(&id, &curve, &one_time) {
                    // A first message opens the session on their side.
                    self.send(
                        peer,
                        Frame::Hello {
                            version: PROTO_VERSION,
                            name: self.settings().display_name,
                            share_last_seen: self.settings().share_last_seen,
                        },
                    );
                    self.flush_waiting(peer);
                    self.on_session_start(peer);
                }
            }
            // Anything else in the clear is not to be believed.
            other => tracing::debug!("ignoring an unsealed frame: {other:?}"),
        }
    }

    fn on_opened_frame(&mut self, peer: PublicKey, frame: Frame) {
        match &frame {
            Frame::Msg { .. } | Frame::FileOffer { .. } => self.note_activity(),
            Frame::Receipt { .. } | Frame::Accept { .. } | Frame::GroupUpdate { .. } => {
                self.wanted_dirty = true;
            }
            _ => {}
        }
        self.on_frame(peer, frame);
    }

    /// What was waiting for the encrypted session to exist.
    fn flush_waiting(&self, peer: PublicKey) {
        let waiting = self
            .sessions
            .get(&peer)
            .map(|s| std::mem::take(&mut *s.waiting.borrow_mut()))
            .unwrap_or_default();
        for frame in waiting {
            self.send(peer, frame);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_new_conn(
        &mut self,
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        outgoing: bool,
        peer: PublicKey,
        curve: [u8; 32],
        one_time: [u8; 32],
    ) {
        // Whoever we have nothing to do with gets nothing. They should not even have the address.
        let known = self.db.contact(&key_to_string(&peer)).map(|c| c.state);
        let allowed = match known {
            Some(ContactState::Accepted | ContactState::PendingOut) => true,
            Some(ContactState::GroupPeer) => {
                !self.db.groups_with_member(&key_to_string(&peer)).is_empty()
            }
            _ => false,
        };
        if !allowed {
            conn.close(1u32.into(), b"no");
            if outgoing {
                self.dialing.remove(&peer);
            }
            return;
        }
        if outgoing {
            self.dialing.remove(&peer);
        }
        // Seen: whatever was wrong, it works now.
        self.backoff.remove(&peer);
        self.blips.remove(&peer);
        self.wanted_dirty = true;
        let id = key_to_string(&peer);
        // For the same Wi-Fi without internet next time.
        let mut keys = self.db.contact_keys(&id).unwrap_or_default();
        let network = *conn.remote_id().as_bytes();
        if keys.network != Some(network) {
            keys.network = Some(network);
            self.db.set_contact_keys(&id, &keys);
        }

        // Both sides may dial each other at the same moment. Both then apply
        // the same rule (keep the connection opened by the smaller key) and
        // agree on which one survives.
        let initiator = if outgoing { self.me } else { peer };
        if let Some(existing) = self.sessions.get(&peer)
            && existing.initiator != initiator
            && existing.initiator.as_bytes() < initiator.as_bytes()
        {
            conn.close(0u32.into(), b"duplicate");
            return;
        }
        if let Some(old) = self.sessions.remove(&peer) {
            old.conn.close(0u32.into(), b"replaced");
        }

        self.next_sid += 1;
        let sid = self.next_sid;
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        spawn_writer(send, out_rx, conn.clone());
        spawn_reader(recv, peer, sid, self.tx.clone(), conn.clone());
        calls::spawn_datagram_reader(conn.clone(), peer, self.media_in.clone());
        spawn_file_acceptor(
            conn.clone(),
            peer,
            self.media_in.clone(),
            self.tx.clone(),
            ReceiveCtx {
                peer_id: id.clone(),
                db: self.db.clone(),
                media: self.media.clone(),
                active: self.downloads.clone(),
            },
        );
        self.sessions.insert(
            peer,
            Session {
                sid,
                out: out_tx,
                conn,
                initiator,
                last_frame: Cell::new(Instant::now()),
                curve,
                one_time,
                waiting: std::cell::RefCell::new(Vec::new()),
                asked_new_session: Cell::new(false),
            },
        );
        // One side starts the encrypted session, and which one is plain from the keys.
        if !self.sealing.has_session(&id) && self.me.as_bytes() < peer.as_bytes() {
            self.sealing.start_session(&id, &curve, &one_time);
        }
        self.on_session_start(peer);
    }

    fn on_session_start(&mut self, peer: PublicKey) {
        self.send_hello(peer);
        if self.db.contact(&key_to_string(&peer)).map(|c| c.state) == Some(ContactState::Accepted) {
            self.tell_where_we_are(peer);
            self.on_accepted_session(peer);
        }
        self.on_group_session(peer);
        self.call_on_session(peer);
    }

    /// The peer is connected and both sides have accepted: show them online
    /// and catch up on anything that did not get through yet.
    fn on_accepted_session(&mut self, peer: PublicKey) {
        self.set_online(peer, true, None);
        self.send_profile(peer);
        for m in self.db.undelivered(&key_to_string(&peer)) {
            // A declined file stays declined; don't keep offering it.
            if m.attachment
                .as_ref()
                .is_some_and(|a| a.state == TransferState::Declined)
            {
                continue;
            }
            self.send_message(peer, &m);
        }
        self.send_read_receipts(peer);
    }

    fn on_closed(&mut self, peer: PublicKey, sid: u64) {
        if self.sessions.get(&peer).map(|s| s.sid) != Some(sid) {
            return; // an older, replaced session
        }
        self.sessions.remove(&peer);
        self.typing_sent.remove(&peer);
        self.call_session_closed(peer);
        let id = key_to_string(&peer);
        match self.db.contact(&id).map(|c| c.state) {
            Some(ContactState::Accepted) => {
                let last_seen = (!self.hides_last_seen.contains(&peer)).then(now_ms);
                self.db.set_last_seen(&id, last_seen);
                self.set_online(peer, false, last_seen);
            }
            Some(ContactState::GroupPeer) => self.set_online(peer, false, None),
            _ => {}
        }
        self.wanted_dirty = true;
    }

    /// A connection broke by itself: we did not close it, and neither did they.
    fn on_broken(&mut self, peer: PublicKey) {
        let now = Instant::now();
        self.blips.insert(peer, now);
        self.backoff.insert(peer, (now + BLIP_RETRY, 0));
        self.wanted_dirty = true;
    }

    fn on_frame(&mut self, peer: PublicKey, frame: Frame) {
        let id = key_to_string(&peer);
        let contact = self.db.contact(&id);
        let state = contact.as_ref().map(|c| c.state);
        match frame {
            Frame::Hello {
                version,
                name,
                share_last_seen,
            } => {
                tracing::debug!("hello from {} v{version}", peer.fmt_short());
                if share_last_seen {
                    self.hides_last_seen.remove(&peer);
                } else {
                    self.hides_last_seen.insert(peer);
                }
                if contact.is_some() && self.db.set_their_name(&id, &clean_name(&name)) {
                    emit(&self.events, CoreEvent::ContactsChanged);
                }
            }
            Frame::Accept { name, location } => match state {
                Some(ContactState::PendingOut) => {
                    let mut keys = self.db.contact_keys(&id).unwrap_or_default();
                    keys.location = Some(location);
                    keys.invite = None;
                    keys.told = false;
                    self.db.set_contact_keys(&id, &keys);
                    self.requests_out.remove(&peer);
                    self.become_accepted(peer, &name);
                    self.tell_where_we_are(peer);
                }
                Some(ContactState::Accepted) => {
                    let mut keys = self.db.contact_keys(&id).unwrap_or_default();
                    if keys.location != Some(location) {
                        keys.location = Some(location);
                        self.db.set_contact_keys(&id, &keys);
                    }
                    if self.db.set_their_name(&id, &clean_name(&name)) {
                        emit(&self.events, CoreEvent::ContactsChanged);
                    }
                }
                // An Accept we never asked for is ignored: nobody can force
                // themselves into our contacts.
                _ => {}
            },
            Frame::Msg {
                id: mid,
                text,
                sent_at,
                group: Some(group),
            } => self.on_group_msg(peer, mid, text, sent_at, group),
            Frame::Msg {
                id: mid,
                text,
                sent_at,
                group: None,
            } => {
                if state != Some(ContactState::Accepted) || text.len() > MAX_TEXT {
                    return;
                }
                let message = Message {
                    id: id_from_bytes(&mid),
                    contact_id: id,
                    outgoing: false,
                    body: text,
                    sent_at,
                    received_at: now_ms(),
                    status: MessageStatus::Delivered,
                    read: false,
                    attachment: None,
                    sender: None,
                };
                if self.db.insert_message(&message).unwrap_or(false) {
                    emit(&self.events, CoreEvent::MessageAdded(message));
                    emit(&self.events, CoreEvent::ContactsChanged);
                }
                // Acknowledge duplicates too: our earlier ack may have been lost.
                self.send(
                    peer,
                    Frame::Receipt {
                        kind: ReceiptKind::Delivered,
                        ids: vec![mid],
                    },
                );
            }
            Frame::Receipt { kind, ids } => {
                // Group messages are confirmed member by member.
                let ids: Vec<[u8; 16]> = ids
                    .into_iter()
                    .filter(|mid| !self.group_receipt(peer, mid))
                    .collect();
                if ids.is_empty() || state != Some(ContactState::Accepted) {
                    return;
                }
                // With read receipts off we neither send nor show them.
                let status = match kind {
                    ReceiptKind::Read if self.settings().read_receipts => MessageStatus::Read,
                    _ => MessageStatus::Delivered,
                };
                for mid in ids {
                    let message_id = id_from_bytes(&mid);
                    // A delivered file is complete on their side.
                    if let Some(m) = self.db.message(&message_id)
                        && m.outgoing
                        && m.contact_id == id
                        && m.attachment
                            .as_ref()
                            .is_some_and(|a| a.state == TransferState::Transferring)
                        && let Some(m) =
                            self.db.set_transfer(&message_id, TransferState::Done, None)
                    {
                        emit(&self.events, CoreEvent::MessageUpdated(m));
                    }
                    if self.db.advance_status(&id, &message_id, status) {
                        emit(
                            &self.events,
                            CoreEvent::MessageStatus {
                                contact_id: id.clone(),
                                message_id,
                                status,
                            },
                        );
                    }
                }
            }
            Frame::Typing => {
                if state == Some(ContactState::Accepted) && self.settings().typing_indicators {
                    emit(&self.events, CoreEvent::Typing { contact_id: id });
                }
            }
            // Only ever on the wire, and handled there.
            Frame::Auth { .. } | Frame::Sealed { .. } | Frame::NeedSession => {}
            Frame::Profile { avatar } => {
                if state == Some(ContactState::Accepted) {
                    self.on_profile(&id, contact.and_then(|c| c.avatar), avatar);
                }
            }
            Frame::FileOffer {
                id: mid,
                sent_at,
                name,
                mime,
                size,
                sha256,
                caption,
                duration_ms,
                group,
            } => {
                if let Some(group) = group {
                    return self.on_group_file_offer(
                        peer,
                        mid,
                        sent_at,
                        Attachment {
                            name: files::clean_file_name(&name),
                            mime: files::clean_mime(&mime),
                            size,
                            sha256: files::to_hex(&sha256),
                            path: None,
                            state: TransferState::Transferring,
                            duration_ms,
                        },
                        caption,
                        group,
                    );
                }
                if state != Some(ContactState::Accepted) || caption.len() > MAX_TEXT {
                    return;
                }
                self.on_file_offer(
                    peer,
                    Message {
                        id: id_from_bytes(&mid),
                        contact_id: id,
                        outgoing: false,
                        body: caption,
                        sent_at,
                        received_at: now_ms(),
                        status: MessageStatus::Delivered,
                        read: false,
                        attachment: Some(Attachment {
                            name: files::clean_file_name(&name),
                            mime: files::clean_mime(&mime),
                            size,
                            sha256: files::to_hex(&sha256),
                            path: None,
                            state: TransferState::Transferring,
                            duration_ms,
                        }),
                        sender: None,
                    },
                );
            }
            Frame::FileWant { id: mid, offset } => {
                let message_id = id_from_bytes(&mid);
                if let Some(m) = self.db.message(&message_id)
                    && m.outgoing
                    && self.may_fetch(&m, &id, state)
                    && m.attachment
                        .as_ref()
                        .is_some_and(|a| a.state != TransferState::Declined)
                {
                    self.start_upload(peer, &m, offset);
                }
            }
            Frame::GroupUpdate {
                state,
                sig,
                names,
                secret,
            } => self.on_group_update(peer, state, sig, names, secret),
            Frame::GroupLeave { group } => self.on_group_leave(peer, group),
            f @ (Frame::CallInvite { .. }
            | Frame::CallAnswer { .. }
            | Frame::CallEnd { .. }
            | Frame::CallVideo { .. }) => self.on_call_frame(peer, f),
            Frame::FileDecline { id: mid } => {
                if self.group_declined(peer, &mid) {
                    return;
                }
                let message_id = id_from_bytes(&mid);
                if let Some(m) = self.db.message(&message_id)
                    && m.outgoing
                    && m.contact_id == id
                    && let Some(m) =
                        self.db
                            .set_transfer(&message_id, TransferState::Declined, None)
                {
                    emit(&self.events, CoreEvent::MessageUpdated(m));
                }
            }
        }
    }

    fn on_profile(&self, contact_id: &str, current: Option<String>, avatar: Option<Vec<u8>>) {
        match avatar {
            Some(bytes)
                if bytes.len() <= protocol::MAX_AVATAR && files::looks_like_image(&bytes) =>
            {
                let dir = files::avatar_dir(&self.media);
                let path = dir.join(format!("{contact_id}.jpg"));
                if std::fs::read(&path).ok().as_deref() == Some(bytes.as_slice())
                    && current.is_some()
                {
                    return;
                }
                if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&path, &bytes).is_ok() {
                    self.db
                        .set_avatar(contact_id, Some(&path.to_string_lossy()));
                    emit(&self.events, CoreEvent::ContactsChanged);
                }
            }
            None if current.is_some() => {
                if let Some(p) = current {
                    let _ = std::fs::remove_file(p);
                }
                self.db.set_avatar(contact_id, None);
                emit(&self.events, CoreEvent::ContactsChanged);
            }
            _ => {}
        }
    }

    fn on_file_offer(&mut self, peer: PublicKey, offered: Message) {
        let mid = offered.id.clone();
        if self.db.insert_message(&offered).unwrap_or(false) {
            emit(&self.events, CoreEvent::MessageAdded(offered.clone()));
            emit(&self.events, CoreEvent::ContactsChanged);
        }
        let Some(raw) = id_to_bytes(&mid) else { return };
        let Some(m) = self.db.message(&mid) else {
            // Seen before and deleted since: they can stop offering it.
            self.send(
                peer,
                Frame::Receipt {
                    kind: ReceiptKind::Delivered,
                    ids: vec![raw],
                },
            );
            return;
        };
        let Some(att) = m.attachment.as_ref() else {
            return;
        };
        if m.outgoing || m.contact_id != offered.contact_id {
            return;
        }
        match att.state {
            // Re-offered after our ack got lost: ack again.
            TransferState::Done => {
                self.send(
                    peer,
                    Frame::Receipt {
                        kind: ReceiptKind::Delivered,
                        ids: vec![raw],
                    },
                );
            }
            TransferState::Declined => {
                self.send(peer, Frame::FileDecline { id: raw });
            }
            TransferState::Failed => {}
            TransferState::Transferring => {
                let limit_mb = self.settings().max_file_mb;
                if limit_mb > 0 && att.size > limit_mb.saturating_mul(1024 * 1024) {
                    if let Some(m) = self.db.set_transfer(&mid, TransferState::Declined, None) {
                        emit(&self.events, CoreEvent::MessageUpdated(m));
                    }
                    self.send(peer, Frame::FileDecline { id: raw });
                } else {
                    self.want_file(peer, &mid);
                }
            }
        }
    }

    fn become_accepted(&mut self, peer: PublicKey, their_name: &str) {
        let id = key_to_string(&peer);
        let name = clean_name(their_name);
        if self
            .db
            .upsert_contact(&id, ContactState::Accepted, Some(&name), None)
            .is_ok()
        {
            emit(&self.events, CoreEvent::Accepted { contact_id: id });
            emit(&self.events, CoreEvent::ContactsChanged);
            self.on_accepted_session(peer);
        }
    }

    // ---- sending ----

    /// Sends, sealed end to end. If the encrypted session is not there yet it waits for it.
    fn send(&self, peer: PublicKey, frame: Frame) -> bool {
        let Some(s) = self.sessions.get(&peer) else {
            return false;
        };
        s.last_frame.set(Instant::now());
        let Ok(plain) = postcard::to_stdvec(&frame) else {
            return false;
        };
        match self.sealing.seal(&key_to_string(&peer), &plain) {
            Some((kind, bytes)) => s.out.send(Frame::Sealed { kind, bytes }).is_ok(),
            None => {
                s.waiting.borrow_mut().push(frame);
                true
            }
        }
    }

    /// The little that goes outside the seal.
    fn send_plain(&self, peer: PublicKey, frame: Frame) -> bool {
        self.sessions
            .get(&peer)
            .is_some_and(|s| s.out.send(frame).is_ok())
    }

    fn send_hello(&self, peer: PublicKey) {
        let s = self.settings();
        self.send(
            peer,
            Frame::Hello {
                version: PROTO_VERSION,
                name: s.display_name,
                share_last_seen: s.share_last_seen,
            },
        );
    }

    /// Our picture goes only to accepted contacts: a stranger who merely
    /// asked to connect gets nothing, and gets to store nothing on our side.
    fn send_profile(&self, peer: PublicKey) {
        let accepted = self
            .db
            .contact(&key_to_string(&peer))
            .is_some_and(|c| c.state == ContactState::Accepted);
        if !accepted {
            return;
        }
        let avatar = self
            .settings()
            .avatar
            .and_then(|p| std::fs::read(p).ok())
            .filter(|b| b.len() <= protocol::MAX_AVATAR);
        self.send(peer, Frame::Profile { avatar });
    }

    fn send_message(&self, peer: PublicKey, m: &Message) {
        let Some(id) = id_to_bytes(&m.id) else { return };
        let group = groups::is_group_id(&m.contact_id).then(|| m.contact_id.clone());
        let frame = match &m.attachment {
            Some(a) => {
                let Some(sha256) = files::from_hex32(&a.sha256) else {
                    return;
                };
                Frame::FileOffer {
                    id,
                    sent_at: m.sent_at,
                    name: a.name.clone(),
                    mime: a.mime.clone(),
                    size: a.size,
                    sha256,
                    caption: m.body.clone(),
                    duration_ms: a.duration_ms,
                    group,
                }
            }
            None => Frame::Msg {
                id,
                text: m.body.clone(),
                sent_at: m.sent_at,
                group,
            },
        };
        let sent = self.send(peer, frame);
        if sent
            && self
                .db
                .advance_status(&m.contact_id, &m.id, MessageStatus::Sent)
        {
            emit(
                &self.events,
                CoreEvent::MessageStatus {
                    contact_id: m.contact_id.clone(),
                    message_id: m.id.clone(),
                    status: MessageStatus::Sent,
                },
            );
        }
    }

    fn send_read_receipts(&self, peer: PublicKey) {
        let id = key_to_string(&peer);
        let pending = self.db.pending_read_receipts(&id);
        if pending.is_empty() {
            return;
        }
        if !self.settings().read_receipts {
            // Never send these, not even later if the setting flips back.
            self.db.mark_read_receipts_sent(&pending);
            return;
        }
        let ids: Vec<[u8; 16]> = pending.iter().filter_map(|m| id_to_bytes(m)).collect();
        if self.send(
            peer,
            Frame::Receipt {
                kind: ReceiptKind::Read,
                ids,
            },
        ) {
            self.db.mark_read_receipts_sent(&pending);
        }
    }

    fn set_online(&self, peer: PublicKey, online: bool, last_seen: Option<i64>) {
        let id = key_to_string(&peer);
        let changed = {
            let mut set = self.shared.online.lock().unwrap_or_else(|p| p.into_inner());
            if online {
                set.insert(id.clone())
            } else {
                set.remove(&id)
            }
        };
        if changed || (!online && last_seen.is_some()) {
            emit(
                &self.events,
                CoreEvent::Presence {
                    contact_id: id,
                    online,
                    last_seen,
                },
            );
        }
    }
}

/// How a connection closes when both sides simply have nothing to say.
const REASON_IDLE: &[u8] = b"idle";

/// With a file moving or a call on, the endpoint stays, whatever the sessions say.
fn plan_keeps_busy(c: &Conditions) -> bool {
    c.busy
}

/// Not reachable from elsewhere, as far as an address can tell.
fn is_private(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
        }
    }
}

/// The relays to use: n0's public ones plus any set by hand, or none at all.
fn relay_mode(settings: &Settings) -> iroh::RelayMode {
    use iroh::{RelayMap, RelayMode};
    if !settings.use_relays {
        return RelayMode::Disabled;
    }
    if settings.extra_relays.is_empty() {
        return RelayMode::Default;
    }
    let map = RelayMode::Default.relay_map();
    match RelayMap::try_from_iter(settings.extra_relays.iter().map(String::as_str)) {
        Ok(extra) => {
            map.extend(&extra);
            RelayMode::Custom(map)
        }
        Err(e) => {
            tracing::warn!("ignoring the extra relays: {e:#}");
            RelayMode::Default
        }
    }
}

async fn bind(key: SecretKey, mode: NetMode, settings: Settings) -> anyhow::Result<Endpoint> {
    match mode {
        NetMode::Internet => {
            let ep = Endpoint::builder(presets::Minimal)
                .secret_key(key)
                .alpns(vec![ALPN.to_vec()])
                .relay_mode(relay_mode(&settings))
                // Where we are goes in the DHT sealed, see `location`. This only finds peers on the
                // same Wi-Fi without the internet.
                .address_lookup(iroh_mdns_address_lookup::MdnsAddressLookup::builder())
                .bind()
                .await?;
            Ok(ep)
        }
        NetMode::Local(_) => {
            let ep = Endpoint::builder(presets::Minimal)
                .secret_key(key)
                .alpns(vec![ALPN.to_vec()])
                .relay_mode(iroh::RelayMode::Disabled)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")?
                .bind()
                .await?;
            Ok(ep)
        }
    }
}

/// Names are display-only; keep them short and single-line.
pub(crate) fn clean_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect::<String>()
        .trim()
        .to_string()
}

fn spawn_accept_loop(ep: Endpoint, tx: mpsc::UnboundedSender<Internal>) {
    tokio::spawn(async move {
        while let Some(incoming) = ep.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let attempt = async {
                    let conn = incoming.accept()?.await?;
                    let (send, recv) = conn.accept_bi().await?;
                    anyhow::Ok((conn, send, recv))
                };
                match tokio::time::timeout(DIAL_TIMEOUT, attempt).await {
                    Ok(Ok((conn, send, recv))) => {
                        let _ = tx.send(Internal::Connected {
                            conn,
                            send,
                            recv,
                            expect: None,
                        });
                    }
                    Ok(Err(e)) => tracing::debug!("incoming failed: {e:#}"),
                    Err(_) => tracing::debug!("incoming timed out"),
                }
            });
        }
    });
}

/// What each side signs: this very connection, so a proof cannot be replayed on another one.
fn auth_bytes(exporter: &[u8; 32], curve: &[u8; 32], one_time: &[u8; 32]) -> Vec<u8> {
    let mut out = b"whatsmine-auth-v2".to_vec();
    out.extend_from_slice(exporter);
    out.extend_from_slice(curve);
    out.extend_from_slice(one_time);
    out
}

/// Says who we are and hears who they are. Both signed over this connection's own secret.
async fn prove_and_check(
    conn: &Connection,
    send: &mut SendStream,
    recv: &mut RecvStream,
    person: &SecretKey,
    curve: [u8; 32],
    one_time: [u8; 32],
) -> anyhow::Result<(PublicKey, [u8; 32], [u8; 32])> {
    let mut exporter = [0u8; 32];
    conn.export_keying_material(&mut exporter, b"whatsmine-auth", b"")
        .map_err(|e| anyhow::anyhow!("no keying material: {e:?}"))?;
    let sig = person
        .sign(&auth_bytes(&exporter, &curve, &one_time))
        .to_bytes()
        .to_vec();
    protocol::write_frame(
        send,
        &Frame::Auth {
            person: *person.public().as_bytes(),
            curve,
            one_time,
            sig,
        },
    )
    .await?;
    let Ok(frame) = protocol::read_frame(recv).await? else {
        anyhow::bail!("an unreadable first frame");
    };
    let Frame::Auth {
        person: them,
        curve,
        one_time,
        sig,
    } = frame
    else {
        anyhow::bail!("the first frame must say who it is");
    };
    let peer = PublicKey::from_bytes(&them)?;
    let sig: [u8; 64] = sig
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("a signature is 64 bytes"))?;
    peer.verify(
        &auth_bytes(&exporter, &curve, &one_time),
        &Signature::from_bytes(&sig),
    )?;
    Ok((peer, curve, one_time))
}

/// Online means someone can reach us: an address that goes somewhere, or a relay. A phone in a
/// tunnel has neither. Ends with the endpoint.
fn spawn_online_watch(ep: Endpoint, tx: mpsc::UnboundedSender<Internal>, loopback_counts: bool) {
    use iroh::Watcher as _;
    tokio::spawn(async move {
        let mut addrs = ep.watch_addr();
        let started = Instant::now();
        loop {
            let addr = addrs.get();
            let online = addr.relay_urls().next().is_some()
                || addr
                    .ip_addrs()
                    .any(|ip| loopback_counts || !ip.ip().is_loopback());
            if online {
                let _ = tx.send(Internal::Net(NetStatus::Online));
            } else if started.elapsed() > Duration::from_secs(15) {
                let _ = tx.send(Internal::Net(NetStatus::Offline));
            }
            // Also now and then, for the time-out above.
            match tokio::time::timeout(Duration::from_secs(16), addrs.updated()).await {
                Ok(Err(_)) => return,
                Ok(Ok(_)) => {}
                Err(_) if ep.is_closed() => return,
                Err(_) => {}
            }
        }
    });
}

fn spawn_writer(mut send: SendStream, mut rx: mpsc::UnboundedReceiver<Frame>, conn: Connection) {
    tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if let Err(e) = protocol::write_frame(&mut send, &frame).await {
                tracing::debug!("write failed: {e:#}");
                conn.close(2u32.into(), b"write error");
                return;
            }
        }
        let _ = send.finish();
    });
}

/// Accepts the file-data streams a peer opens on this connection.
fn spawn_file_acceptor(
    conn: Connection,
    peer: PublicKey,
    media_in: MediaIn,
    tx: mpsc::UnboundedSender<Internal>,
    ctx: ReceiveCtx,
) {
    let ctx = Arc::new(ctx);
    tokio::spawn(async move {
        while let Ok(mut recv) = conn.accept_uni().await {
            let (tx, ctx, media_in) = (tx.clone(), ctx.clone(), media_in.clone());
            tokio::spawn(async move {
                let mut header = [0u8; files::STREAM_HEADER];
                let read =
                    tokio::time::timeout(Duration::from_secs(20), recv.read_exact(&mut header))
                        .await;
                if !matches!(read, Ok(Ok(()))) {
                    let _ = recv.stop(1u32.into());
                    return;
                }
                if &header[..4] == VIDEO_STREAM_MAGIC {
                    calls::receive_video(recv, header, peer, &media_in).await;
                    return;
                }
                let progress_tx = tx.clone();
                let contact_id = ctx.peer_id.clone();
                let (message_id, result) =
                    files::receive(recv, header, &ctx, move |id, done, total| {
                        let _ = progress_tx.send(Internal::Progress {
                            contact_id: contact_id.clone(),
                            message_id: id.to_string(),
                            done,
                            total,
                        });
                    })
                    .await;
                let _ = tx.send(Internal::Download {
                    peer,
                    message_id,
                    result,
                });
            });
        }
    });
}

fn spawn_reader(
    mut recv: RecvStream,
    peer: PublicKey,
    sid: u64,
    tx: mpsc::UnboundedSender<Internal>,
    conn: Connection,
) {
    tokio::spawn(async move {
        loop {
            // QUIC's own heartbeat and idle time-out notice a peer that is gone.
            match protocol::read_frame(&mut recv).await {
                Ok(Ok(frame)) => {
                    if tx.send(Internal::Frame { peer, frame }).is_err() {
                        return;
                    }
                }
                Ok(Err(e)) => tracing::debug!("skipping unreadable frame: {e:#}"),
                Err(e) => {
                    tracing::debug!("session {sid} with {} ended: {e:#}", peer.fmt_short());
                    let broken = !closed_on_purpose(&conn);
                    conn.close(0u32.into(), b"closed");
                    let _ = tx.send(Internal::Closed { peer, sid, broken });
                    return;
                }
            }
        }
    });
}

/// One of the two sides closed it, as opposed to the connection breaking.
fn closed_on_purpose(conn: &Connection) -> bool {
    use iroh::endpoint::ConnectionError;
    matches!(
        conn.close_reason(),
        Some(ConnectionError::ApplicationClosed(_) | ConnectionError::LocallyClosed)
    )
}

#[cfg(test)]
mod tests {
    use super::clean_name;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Roy\n\u{7}  "), "Roy");
        assert_eq!(clean_name(&"x".repeat(100)).len(), 64);
    }
}
