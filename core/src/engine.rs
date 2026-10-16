//! The networking engine: one task that owns the iroh endpoint and every
//! live session, driven by commands from the UI and events from connection
//! tasks. All protocol rules live here.
//!
//! Reliability comes from acknowledgements, not from connections: an
//! outgoing message stays "undelivered" until the peer's `Delivered` receipt
//! arrives, and every new session re-sends what is undelivered. Receivers
//! dedupe by message id, so re-sending is always safe.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, PublicKey, SecretKey};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::NetMode;
use crate::db::Database;
use crate::identity::key_to_string;
use crate::model::{ContactState, CoreEvent, Message, MessageStatus, NetStatus, Settings, now_ms};
use crate::protocol::{
    self, ALPN, Frame, MAX_TEXT, PROTO_VERSION, ReceiptKind, id_from_bytes, id_to_bytes,
};

const DIAL_TIMEOUT: Duration = Duration::from_secs(30);
const BACKOFF_MIN: Duration = Duration::from_secs(3);
const BACKOFF_MAX: Duration = Duration::from_secs(120);
const TYPING_THROTTLE: Duration = Duration::from_secs(3);
const TICK: Duration = Duration::from_secs(2);

/// Requests from the `Core` handle.
pub(crate) enum Cmd {
    /// Connect to this contact now if not connected.
    Dial(PublicKey),
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
    /// Name or privacy flags changed; re-announce.
    SettingsChanged,
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
    Frame {
        peer: PublicKey,
        frame: Frame,
    },
    Closed {
        peer: PublicKey,
        sid: u64,
    },
    Net(NetStatus),
}

struct Session {
    sid: u64,
    out: mpsc::UnboundedSender<Frame>,
    conn: Connection,
    /// Who opened this connection. Used to pick one when both dialed at once.
    initiator: PublicKey,
}

/// State shared with the `Core` handle for cheap synchronous reads.
#[derive(Clone)]
pub(crate) struct Shared {
    pub online: Arc<Mutex<HashSet<String>>>,
    pub net: Arc<Mutex<NetStatus>>,
}

pub(crate) struct Engine {
    db: Arc<Database>,
    key: SecretKey,
    me: PublicKey,
    events: broadcast::Sender<CoreEvent>,
    shared: Shared,
    mode: NetMode,
    sessions: HashMap<PublicKey, Session>,
    dialing: HashSet<PublicKey>,
    backoff: HashMap<PublicKey, (Instant, Duration)>,
    /// Peers that asked us not to show their last seen.
    hides_last_seen: HashSet<PublicKey>,
    typing_sent: HashMap<PublicKey, Instant>,
    next_sid: u64,
    tx: mpsc::UnboundedSender<Internal>,
}

fn emit(events: &broadcast::Sender<CoreEvent>, ev: CoreEvent) {
    // No subscribers is fine.
    let _ = events.send(ev);
}

impl Engine {
    pub(crate) async fn run(
        db: Arc<Database>,
        key: SecretKey,
        mode: NetMode,
        events: broadcast::Sender<CoreEvent>,
        shared: Shared,
        mut cmds: mpsc::UnboundedReceiver<Cmd>,
    ) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let me = key.public();
        let mut engine = Engine {
            db,
            key,
            me,
            events,
            shared,
            mode,
            sessions: HashMap::new(),
            dialing: HashSet::new(),
            backoff: HashMap::new(),
            hides_last_seen: HashSet::new(),
            typing_sent: HashMap::new(),
            next_sid: 0,
            tx,
        };

        let ep = loop {
            engine.set_net(NetStatus::Connecting);
            match engine.bind().await {
                Ok(ep) => break ep,
                Err(e) => {
                    tracing::warn!("bind failed: {e:#}");
                    engine.set_net(NetStatus::Offline);
                    // Still honour shutdown while we cannot bind.
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                        cmd = cmds.recv() => match cmd {
                            Some(Cmd::Shutdown(done)) => {
                                let _ = done.send(());
                                return;
                            }
                            None => return,
                            // The database already holds the state; the
                            // first sessions after binding catch up.
                            Some(_) => {}
                        }
                    }
                }
            }
        };

        spawn_accept_loop(ep.clone(), engine.tx.clone());
        spawn_online_watch(ep.clone(), engine.tx.clone());

        let mut tick = tokio::time::interval(TICK);
        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(Cmd::Shutdown(done)) => {
                        for (_, s) in engine.sessions.drain() {
                            s.conn.close(0u32.into(), b"bye");
                        }
                        let _ = tokio::time::timeout(Duration::from_secs(2), ep.close()).await;
                        let _ = done.send(());
                        return;
                    }
                    Some(cmd) => engine.handle_cmd(&ep, cmd),
                    None => {
                        ep.close().await;
                        return;
                    }
                },
                Some(ev) = rx.recv() => engine.handle_internal(ev),
                _ = tick.tick() => engine.dial_due(&ep),
            }
        }
    }

    async fn bind(&self) -> anyhow::Result<Endpoint> {
        match &self.mode {
            NetMode::Internet => {
                let ep = Endpoint::builder(presets::N0)
                    .secret_key(self.key.clone())
                    .alpns(vec![ALPN.to_vec()])
                    // Publish/resolve on the BitTorrent Mainline DHT too, so
                    // finding a peer does not depend on any one company.
                    .address_lookup(iroh_mainline_address_lookup::DhtAddressLookup::builder())
                    // Find peers on the same Wi-Fi without the internet.
                    .address_lookup(iroh_mdns_address_lookup::MdnsAddressLookup::builder())
                    .bind()
                    .await?;
                Ok(ep)
            }
            NetMode::Local(lookup) => {
                let ep = Endpoint::builder(presets::Minimal)
                    .secret_key(self.key.clone())
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

    fn handle_cmd(&mut self, ep: &Endpoint, cmd: Cmd) {
        match cmd {
            Cmd::Dial(peer) => {
                self.backoff.remove(&peer);
                if !self.sessions.contains_key(&peer) {
                    self.dial(ep, peer);
                }
            }
            Cmd::SendOne {
                contact,
                message_id,
            } => {
                if self.sessions.contains_key(&contact)
                    && let Some(m) = self.db.message(&message_id)
                {
                    self.send_message(contact, &m);
                }
            }
            Cmd::Accepted(peer) => {
                if self.sessions.contains_key(&peer) {
                    let name = self.settings().display_name;
                    self.send(peer, Frame::Accept { name });
                    self.on_accepted_session(peer);
                } else {
                    self.backoff.remove(&peer);
                    self.dial(ep, peer);
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
                self.set_online(peer, false, None);
            }
            Cmd::SettingsChanged => {
                let peers: Vec<_> = self.sessions.keys().copied().collect();
                for p in peers {
                    self.send_hello(p);
                }
            }
            Cmd::Shutdown(_) => unreachable!("handled in run loop"),
        }
    }

    // ---- dialing ----

    /// Dials every contact we want to talk to that is not connected and
    /// whose backoff has expired.
    fn dial_due(&mut self, ep: &Endpoint) {
        let now = Instant::now();
        let wanted = self
            .db
            .contacts_in(&[ContactState::Accepted, ContactState::PendingOut]);
        for c in wanted {
            let Some(peer) = crate::identity::parse_key(&c.id) else {
                continue;
            };
            if self.sessions.contains_key(&peer) || self.dialing.contains(&peer) {
                continue;
            }
            if self.backoff.get(&peer).is_some_and(|(at, _)| *at > now) {
                continue;
            }
            self.dial(ep, peer);
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
        let delay = self
            .backoff
            .get(&peer)
            .map(|(_, d)| (*d * 2).min(BACKOFF_MAX))
            .unwrap_or(BACKOFF_MIN);
        self.backoff.insert(peer, (Instant::now() + delay, delay));
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
            Internal::Frame { peer, frame } => self.on_frame(peer, frame),
            Internal::Closed { peer, sid } => self.on_closed(peer, sid),
            Internal::Net(status) => self.set_net(status),
        }
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
            self.backoff.remove(&peer);
        }
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
        self.sessions.insert(
            peer,
            Session {
                sid,
                out: out_tx,
                conn,
                initiator,
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
    }

    /// The peer is connected and both sides have accepted: show them online
    /// and catch up on anything that did not get through yet.
    fn on_accepted_session(&mut self, peer: PublicKey) {
        self.set_online(peer, true, None);
        for m in self.db.undelivered(&key_to_string(&peer)) {
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
        let id = key_to_string(&peer);
        let accepted = self
            .db
            .contact(&id)
            .is_some_and(|c| c.state == ContactState::Accepted);
        if accepted {
            let last_seen = (!self.hides_last_seen.contains(&peer)).then(now_ms);
            self.db.set_last_seen(&id, last_seen);
            self.set_online(peer, false, last_seen);
        }
        // Try again soon: most disconnects are network blips.
        self.backoff
            .insert(peer, (Instant::now() + BACKOFF_MIN, BACKOFF_MIN));
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
                None => {
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
                if state != Some(ContactState::Accepted) {
                    return;
                }
                // With read receipts off we neither send nor show them.
                let status = match kind {
                    ReceiptKind::Read if self.settings().read_receipts => MessageStatus::Read,
                    _ => MessageStatus::Delivered,
                };
                for mid in ids {
                    let message_id = id_from_bytes(&mid);
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

    fn send_message(&self, peer: PublicKey, m: &Message) {
        let Some(id) = id_to_bytes(&m.id) else { return };
        let sent = self.send(
            peer,
            Frame::Msg {
                id,
                text: m.body.clone(),
                sent_at: m.sent_at,
            },
        );
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

/// Names are display-only; keep them short and single-line.
fn clean_name(name: &str) -> String {
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

fn spawn_online_watch(ep: Endpoint, tx: mpsc::UnboundedSender<Internal>) {
    tokio::spawn(async move {
        let online = ep.online();
        tokio::pin!(online);
        tokio::select! {
            _ = &mut online => {}
            _ = tokio::time::sleep(Duration::from_secs(15)) => {
                let _ = tx.send(Internal::Net(NetStatus::Offline));
                online.await;
            }
        }
        let _ = tx.send(Internal::Net(NetStatus::Online));
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

fn spawn_reader(
    mut recv: RecvStream,
    peer: PublicKey,
    sid: u64,
    tx: mpsc::UnboundedSender<Internal>,
    conn: Connection,
) {
    tokio::spawn(async move {
        loop {
            match protocol::read_frame(&mut recv).await {
                Ok(frame) => {
                    if tx.send(Internal::Frame { peer, frame }).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    tracing::debug!("session {sid} with {} ended: {e:#}", peer.fmt_short());
                    conn.close(0u32.into(), b"closed");
                    let _ = tx.send(Internal::Closed { peer, sid });
                    return;
                }
            }
        }
    });
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
