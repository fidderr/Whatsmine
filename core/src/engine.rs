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
use iroh::{Endpoint, PublicKey, SecretKey};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::NetMode;
use crate::calls::{self, ActiveCall, CallSnapshot, MediaIn, MediaSink};
use crate::db::Database;
use crate::files::{self, ReceiveCtx, Received};
use crate::groups;
use crate::identity::key_to_string;
use crate::model::{
    Attachment, ContactState, CoreEvent, Message, MessageStatus, NetStatus, Settings,
    TransferState, now_ms,
};
use crate::power::{self, Conditions, Device, Plan};
use crate::protocol::{
    self, ALPN, Frame, GroupState, MAX_TEXT, PROTO_VERSION, ReceiptKind, VIDEO_STREAM_MAGIC,
    id_from_bytes, id_to_bytes,
};
use crate::retry;

mod call;
mod group;

use call::{CallState, GroupCalls};

const DIAL_TIMEOUT: Duration = Duration::from_secs(30);
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
/// A wake-up scheduled with the system only changes for a difference of this much.
const WAKE_SLACK: Duration = Duration::from_secs(5);

/// Requests from the `Core` handle.
pub(crate) enum Cmd {
    /// Connect to this contact now if not connected.
    Dial(PublicKey),
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
    NewConn {
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        outgoing: bool,
    },
    DialFailed(PublicKey),
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
    key: SecretKey,
    me: PublicKey,
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
        key: SecretKey,
        mode: NetMode,
        events: broadcast::Sender<CoreEvent>,
        shared: Shared,
        media: PathBuf,
        media_sink: MediaSink,
        mut cmds: mpsc::UnboundedReceiver<Cmd>,
    ) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let me = key.public();
        let mut engine = Engine {
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
        let (key, mode, tx) = (self.key.clone(), self.mode.clone(), self.tx.clone());
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
                if self.sessions.contains_key(&peer) {
                    let name = self.settings().display_name;
                    self.send(peer, Frame::Accept { name });
                    self.on_accepted_session(peer);
                } else {
                    self.backoff.remove(&peer);
                    self.look_around.push_front(peer);
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
                for p in to {
                    self.send(
                        p,
                        Frame::GroupUpdate {
                            state: state.clone(),
                            sig: sig.clone(),
                            names: names.clone(),
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
        let ep = ep.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let attempt = async {
                let conn = ep.connect(peer, ALPN).await?;
                let (send, recv) = conn.open_bi().await?;
                anyhow::Ok((conn, send, recv))
            };
            let ev = match tokio::time::timeout(DIAL_TIMEOUT, attempt).await {
                Ok(Ok((conn, send, recv))) => Internal::NewConn {
                    conn,
                    send,
                    recv,
                    outgoing: true,
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
            Internal::NewConn {
                conn,
                send,
                recv,
                outgoing,
            } => self.on_new_conn(conn, send, recv, outgoing),
            Internal::DialFailed(peer) => {
                self.dialing.remove(&peer);
                self.bump_backoff(peer);
            }
            Internal::Bound(result) => self.on_bound(result),
            Internal::Frame { peer, frame } => {
                if let Some(s) = self.sessions.get(&peer) {
                    s.last_frame.set(Instant::now());
                }
                match &frame {
                    Frame::Msg { .. } | Frame::FileOffer { .. } => self.note_activity(),
                    Frame::Receipt { .. } | Frame::Accept { .. } | Frame::GroupUpdate { .. } => {
                        self.wanted_dirty = true;
                    }
                    _ => {}
                }
                self.on_frame(peer, frame);
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

    fn on_new_conn(
        &mut self,
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        outgoing: bool,
    ) {
        let peer = conn.remote_id();
        if outgoing {
            self.dialing.remove(&peer);
        }
        // Seen: whatever was wrong, it works now.
        self.backoff.remove(&peer);
        self.blips.remove(&peer);
        self.wanted_dirty = true;
        let id = key_to_string(&peer);
        if self
            .db
            .contact(&id)
            .is_some_and(|c| c.state == ContactState::Blocked)
        {
            conn.close(1u32.into(), b"blocked");
            return;
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
            },
        );
        self.on_session_start(peer);
    }

    fn on_session_start(&mut self, peer: PublicKey) {
        self.send_hello(peer);
        let settings = self.settings();
        match self.db.contact(&key_to_string(&peer)).map(|c| c.state) {
            Some(ContactState::Accepted) => {
                self.send(
                    peer,
                    Frame::Accept {
                        name: settings.display_name,
                    },
                );
                self.on_accepted_session(peer);
            }
            Some(ContactState::PendingOut) => {
                self.send(
                    peer,
                    Frame::Request {
                        name: settings.display_name,
                    },
                );
            }
            _ => {}
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
            Frame::Request { name } => match state {
                None | Some(ContactState::GroupPeer) => {
                    let name = clean_name(&name);
                    if self
                        .db
                        .upsert_contact(&id, ContactState::PendingIn, Some(&name), None)
                        .is_ok()
                    {
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
                Some(ContactState::PendingOut) => {
                    // We both asked: that is mutual consent.
                    self.become_accepted(peer, &name);
                    let my_name = self.settings().display_name;
                    self.send(peer, Frame::Accept { name: my_name });
                }
                Some(ContactState::Accepted) => {
                    // They lost their copy of our acceptance; repeat it.
                    let my_name = self.settings().display_name;
                    self.send(peer, Frame::Accept { name: my_name });
                }
                Some(ContactState::PendingIn) => {
                    self.db.set_their_name(&id, &clean_name(&name));
                }
                Some(ContactState::Blocked) => self.drop_session(peer),
            },
            Frame::Accept { name } => match state {
                Some(ContactState::PendingOut) => self.become_accepted(peer, &name),
                Some(ContactState::Accepted) if self.db.set_their_name(&id, &clean_name(&name)) => {
                    emit(&self.events, CoreEvent::ContactsChanged);
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
            // Only keeps the session alive; see the reader's timeout.
            Frame::Ping => {}
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
            Frame::GroupUpdate { state, sig, names } => {
                self.on_group_update(peer, state, sig, names)
            }
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
        let Some(m) = self.db.message(&mid) else {
            return;
        };
        let Some(att) = m.attachment.as_ref() else {
            return;
        };
        if m.outgoing || m.contact_id != offered.contact_id {
            return;
        }
        let Some(raw) = id_to_bytes(&mid) else { return };
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

    fn send(&self, peer: PublicKey, frame: Frame) -> bool {
        self.sessions.get(&peer).is_some_and(|s| {
            s.last_frame.set(Instant::now());
            s.out.send(frame).is_ok()
        })
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

    fn drop_session(&mut self, peer: PublicKey) {
        if let Some(s) = self.sessions.remove(&peer) {
            s.conn.close(1u32.into(), b"blocked");
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
                // Publish and resolve on the BitTorrent Mainline DHT, so finding a peer does not
                // depend on any one company.
                .address_lookup(iroh_mainline_address_lookup::DhtAddressLookup::builder())
                // Find peers on the same Wi-Fi without the internet.
                .address_lookup(iroh_mdns_address_lookup::MdnsAddressLookup::builder())
                .bind()
                .await?;
            Ok(ep)
        }
        NetMode::Local(lookup) => {
            let ep = Endpoint::builder(presets::Minimal)
                .secret_key(key)
                .alpns(vec![ALPN.to_vec()])
                .relay_mode(iroh::RelayMode::Disabled)
                .clear_ip_transports()
                .bind_addr("127.0.0.1:0")?
                .address_lookup(lookup.clone())
                .bind()
                .await?;
            let mut addr = iroh::EndpointAddr::new(ep.id());
            for sock in ep.bound_sockets() {
                addr = addr.with_ip_addr(sock);
            }
            lookup.set_endpoint_info(addr);
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
                        let _ = tx.send(Internal::NewConn {
                            conn,
                            send,
                            recv,
                            outgoing: false,
                        });
                    }
                    Ok(Err(e)) => tracing::debug!("incoming failed: {e:#}"),
                    Err(_) => tracing::debug!("incoming timed out"),
                }
            });
        }
    });
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
