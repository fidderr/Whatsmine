//! WhatsMine core: everything except the UI.
//!
//! - Identity is an Ed25519 key generated on this device; its public half
//!   is the user's address.
//! - Peers connect directly with iroh (QUIC, hole punching, encrypted and
//!   authenticated), found by key via DNS, the Mainline DHT or the local
//!   network.
//! - Everything is stored locally in SQLite. There is no server and no
//!   account.
//!
//! [`Core`] is the handle the UI uses. Its methods are synchronous and cheap
//! (local database work plus a message to the engine), so they can be called
//! straight from UI event handlers. Updates arrive through [`Core::subscribe`].

pub mod backup;
pub mod calls;
pub mod codes;
pub mod db;
mod engine;
pub mod files;
pub mod groups;
pub mod identity;
pub mod knocks;
pub mod location;
pub mod model;
pub mod paths;
pub mod power;
pub mod protocol;
pub mod records;
pub mod requests;
pub mod retry;
pub mod rich;
pub mod sealing;
#[cfg(feature = "test-hooks")]
mod seed;
pub mod vault;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use iroh::{PublicKey, SecretKey};
use tokio::sync::{broadcast, mpsc};

pub use db::DbRecovery;
pub use model::*;

use db::Database;
use engine::{Cmd, Engine, Shared};
use rich::{
    CallRecord, ChatPrefs, Folder, MessageChange, MessageExtra, Rich, RichChange, SearchHit,
    SearchIn,
};

/// Whether a message fits a search. `query` in lower case, and empty for all of the kind.
fn message_fits(m: &Message, query: &str, within: SearchIn) -> bool {
    let media = |a: &Attachment| a.mime.starts_with("image/") || a.mime.starts_with("video/");
    let has_link = m.body.contains("http://") || m.body.contains("https://");
    let kind = match within {
        SearchIn::All => !query.is_empty(),
        SearchIn::People => false,
        SearchIn::Media => m.attachment.as_ref().is_some_and(media),
        SearchIn::Files => m.attachment.as_ref().is_some_and(|a| !media(a)),
        SearchIn::Links => has_link,
    };
    kind && (query.is_empty()
        || m.body.to_lowercase().contains(query)
        || m.attachment
            .as_ref()
            .is_some_and(|a| a.name.to_lowercase().contains(query)))
}

/// There is always a main invite: the plain link and the name code stand for it.
fn ensure_main_invite(db: &Database) {
    if !db.invites().iter().any(|i| i.main && !i.revoked) {
        db.save_invite(&MyInvite::new("", None, None, true));
    }
}

pub const DB_FILE: &str = "whatsmine.db";

/// How often to look again when the network is still coming up, and how long apart.
const NET_TRIES: u32 = 10;
const NET_WAIT: Duration = Duration::from_millis(500);

/// How the core reaches the network.
#[derive(Clone, Debug, Default)]
pub enum NetMode {
    /// The real internet: n0 DNS + Mainline DHT + mDNS discovery, relays as
    /// fallback.
    #[default]
    Internet,
    /// Loopback only, with a DHT that lives in this process. For tests.
    Local(records::LocalRecords),
}

#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}

struct Inner {
    dir: PathBuf,
    db: Arc<Database>,
    me: PublicKey,
    key: SecretKey,
    recovery: DbRecovery,
    started_clean: bool,
    cmds: mpsc::UnboundedSender<Cmd>,
    events: broadcast::Sender<CoreEvent>,
    shared: Shared,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    /// Where incoming call media goes (whoever called `call_media` last).
    media_sink: calls::MediaSink,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(rt) = self
            .runtime
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            rt.shutdown_background();
        }
    }
}

impl Core {
    /// Opens (or creates) the identity and database in `dir` and starts
    /// networking in the background. Safe to call from inside another async
    /// runtime: the core runs on its own.
    pub fn start(dir: &Path, mode: NetMode) -> anyhow::Result<Core> {
        std::fs::create_dir_all(dir)?;
        let started_clean = paths::clean_start(dir).context("removing old data")?;
        let restored = {
            let vault = vault::Vault::open_or_create(dir).context("opening the vault")?;
            backup::apply_if_left(dir, &vault).context("restoring")?
        };
        let vault = Arc::new(vault::Vault::open_or_create(dir).context("opening the vault")?);
        let (db, recovery) = Database::open_with_recovery(&dir.join(DB_FILE), vault);
        let db = Arc::new(db);
        if let backup::Restored::Words(words) = &restored {
            identity::Identity::from_words(words)?.save(&db)?;
        }
        let me = identity::Identity::load_or_create(&db).context("loading identity")?;
        let key = me.person();
        ensure_main_invite(&db);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("whatsmine-core")
            .enable_all()
            .build()?;

        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(1024);
        let media_sink: calls::MediaSink = Arc::new(Mutex::new(None));
        let shared = Shared {
            online: Arc::new(Mutex::new(HashSet::new())),
            net: Arc::new(Mutex::new(NetStatus::Connecting)),
            group_calls: Arc::new(Mutex::new(std::collections::HashMap::new())),
            call: Arc::new(Mutex::new(None)),
            tries: Arc::new(Mutex::new(std::collections::HashMap::new())),
        };
        runtime.spawn(Engine::run(
            db.clone(),
            me,
            mode,
            events.clone(),
            shared.clone(),
            files::media_dir(dir),
            media_sink.clone(),
            cmd_rx,
        ));

        Ok(Core {
            inner: Arc::new(Inner {
                dir: dir.to_path_buf(),
                db,
                me: key.public(),
                key: key.clone(),
                recovery,
                started_clean,
                cmds,
                events,
                shared,
                runtime: Mutex::new(Some(runtime)),
                media_sink,
            }),
        })
    }

    fn cmd(&self, cmd: Cmd) {
        let _ = self.inner.cmds.send(cmd);
    }

    fn emit(&self, ev: CoreEvent) {
        let _ = self.inner.events.send(ev);
    }

    // ---- calls ----

    /// Starts a call. The result is the call id; what happens next arrives as
    /// [`CoreEvent::Call`] (`Connected` or `Ended`).
    pub fn call_invite(&self, contact_id: &str, video: bool) -> anyhow::Result<String> {
        if groups::is_group_id(contact_id) {
            let info = self.group_info(contact_id).context("not a group")?;
            ensure!(info.active, "group is closed");
            ensure!(
                info.members.iter().any(|m| !m.is_me && m.online),
                "nobody is online"
            );
        } else {
            ensure!(
                self.contact(contact_id)
                    .is_some_and(|c| c.state == ContactState::Accepted),
                "not a contact"
            );
            ensure!(self.is_online(contact_id), "contact is offline");
        }
        let call = *uuid::Uuid::now_v7().as_bytes();
        self.cmd(Cmd::CallInvite {
            target: contact_id.to_string(),
            call,
            video,
        });
        Ok(protocol::id_from_bytes(&call))
    }

    /// Joins the group call going on in this group.
    pub fn call_join(&self, group_id: &str) {
        self.cmd(Cmd::CallJoin {
            group: group_id.to_string(),
        });
    }

    /// If a call is going on in this group that we could join: whether it
    /// is a video call.
    pub fn group_call(&self, group_id: &str) -> Option<bool> {
        self.inner
            .shared
            .group_calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(group_id)
            .copied()
    }

    /// Accepts or declines the call that is ringing.
    pub fn call_answer(&self, accept: bool) {
        self.cmd(Cmd::CallAnswer { accept });
    }

    /// Hangs up, or cancels a call that is still ringing.
    pub fn call_end(&self) {
        self.cmd(Cmd::CallEnd);
    }

    /// Sends one encoded audio packet of the connected call.
    pub fn call_audio(&self, ts: u32, data: Vec<u8>) {
        self.cmd(Cmd::CallAudio { ts, data });
    }

    /// Sends one encoded video frame of the connected call.
    pub fn call_video(&self, ts: u32, key: bool, data: Vec<u8>) {
        self.cmd(Cmd::CallVideo { ts, key, data });
    }

    /// Tells the other side our camera went on or off.
    pub fn call_video_on(&self, on: bool) {
        self.cmd(Cmd::CallVideoOn(on));
    }

    /// The stream of incoming call media. Only the latest caller of this
    /// receives it, so a new screen can take over from one that went away.
    pub fn call_media(&self) -> mpsc::Receiver<calls::CallMedia> {
        let (tx, rx) = mpsc::channel(256);
        *self
            .inner
            .media_sink
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(tx);
        rx
    }

    /// The call in progress (or ringing), for a screen that opens in the middle of it.
    pub fn call_snapshot(&self) -> Option<calls::CallSnapshot> {
        self.inner
            .shared
            .call
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Stops networking, telling connected peers we left. Blocks up to ~2s.
    pub fn shutdown(&self) {
        let (done, wait) = std::sync::mpsc::channel();
        self.cmd(Cmd::Shutdown(done));
        let _ = wait.recv_timeout(Duration::from_secs(3));
        if let Some(rt) = self
            .inner
            .runtime
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            rt.shutdown_background();
        }
    }

    /// Shuts down and deletes the identity and every message on this device.
    /// There is no copy anywhere else, so this cannot be undone. The next
    /// `Core::start` on this directory creates a brand-new identity.
    pub fn wipe_everything(&self) -> anyhow::Result<()> {
        self.shutdown();
        self.inner.db.wipe()?;
        let _ = std::fs::remove_dir_all(self.media_dir());
        let _ = std::fs::remove_file(self.inner.dir.join("avatar.jpg"));
        let _ = std::fs::remove_file(self.inner.dir.join(identity::KEY_FILE));
        Ok(())
    }

    // ---- identity and settings ----

    /// This device's key: the thing you share so people can reach you.
    pub fn my_key(&self) -> String {
        identity::key_to_string(&self.inner.me)
    }

    /// This start removed data of an earlier generation (see [`paths::clean_start`]): say so.
    pub fn started_clean(&self) -> bool {
        self.inner.started_clean
    }

    pub fn db_recovery(&self) -> DbRecovery {
        self.inner.recovery.clone()
    }

    pub fn settings(&self) -> Settings {
        self.inner.db.settings()
    }

    pub fn save_settings(&self, settings: &Settings) -> anyhow::Result<()> {
        self.inner.db.save_settings(settings)?;
        self.cmd(Cmd::SettingsChanged);
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.inner.events.subscribe()
    }

    /// What the device says about itself (Android: network, charger, battery saver).
    pub fn set_device(&self, device: power::Device) {
        self.cmd(Cmd::Device(device));
    }

    /// The network may be another one now, also if it is the same kind. Android does not tell
    /// the network library by itself.
    pub fn network_changed(&self) {
        self.cmd(Cmd::NetworkChanged);
    }

    /// The app came on screen or went off it.
    pub fn set_in_front(&self, in_front: bool) {
        self.cmd(Cmd::InFront(in_front));
    }

    /// This chat is open on screen (keep trying them), or none is.
    pub fn set_chat_open(&self, contact_id: Option<&str>) {
        let peer = contact_id.and_then(identity::parse_key);
        self.cmd(Cmd::ChatOpen(peer));
    }

    /// The system woke us, as asked with [`CoreEvent::WakeIn`].
    pub fn wake(&self) {
        self.cmd(Cmd::Wake);
    }

    /// Where a message of ours is: when the next try is, and why it is not there yet.
    pub fn delivery_info(&self, message_id: &str) -> Option<DeliveryInfo> {
        let m = self.inner.db.message(message_id)?;
        let trying = matches!(m.status, MessageStatus::Queued | MessageStatus::Sent);
        let tries = self
            .inner
            .shared
            .tries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&m.contact_id)
            .copied();
        let connected = self.is_online(&m.contact_id);
        Some(DeliveryInfo {
            status: m.status,
            connected,
            next_try_in_ms: tries
                .filter(|_| trying && !connected)
                .map(|t| t.next_at.saturating_sub(now_ms()) as u64),
            failures: tries.map_or(0, |t| t.failures),
            knocked: tries.is_some_and(|t| t.knocked),
        })
    }

    /// Tries a message again that we stopped trying.
    pub fn retry_message(&self, message_id: &str) {
        if self.inner.db.retry_message(message_id)
            && let Some(m) = self.inner.db.message(message_id)
        {
            self.emit(CoreEvent::MessageStatus {
                contact_id: m.contact_id.clone(),
                message_id: m.id.clone(),
                status: MessageStatus::Queued,
            });
            let peer = identity::parse_key(&m.contact_id);
            self.dispatch(&m.contact_id, peer, &m);
        }
    }

    /// What the keep-trying time would do once it runs out, right now. For tests.
    pub fn give_up_now(&self) {
        self.cmd(Cmd::GiveUpBefore(now_ms() + 1));
    }

    /// A file that waited for the network rules: fetch it now.
    pub fn fetch_file(&self, message_id: &str) {
        self.retry_file(message_id);
    }

    /// The radio went active for some app: a cheap moment to look around.
    pub fn radio_active(&self) {
        self.cmd(Cmd::RadioActive);
    }

    /// How the connection stands, for the screen that explains it.
    pub async fn connection_report(&self) -> Option<ConnectionReport> {
        let (answer, report) = tokio::sync::oneshot::channel();
        self.cmd(Cmd::Report(answer));
        report.await.ok()
    }

    pub fn net_status(&self) -> NetStatus {
        *self
            .inner
            .shared
            .net
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    pub fn is_online(&self, contact_id: &str) -> bool {
        self.inner
            .shared
            .online
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(contact_id)
    }

    // ---- contacts ----

    /// Chats: accepted contacts and requests we sent, most recent first.
    pub fn chats(&self) -> Vec<ChatSummary> {
        self.inner.db.chat_summaries()
    }

    /// Requests from people who want to add us.
    pub fn requests(&self) -> Vec<Contact> {
        self.inner.db.contacts_in(&[ContactState::PendingIn])
    }

    /// Everyone we have added (and everyone we asked), by name. Chats that
    /// were deleted from the chat list are still here.
    pub fn address_book(&self) -> Vec<Contact> {
        let mut people: Vec<Contact> = self
            .inner
            .db
            .contacts_in(&[ContactState::Accepted, ContactState::PendingOut])
            .into_iter()
            .filter(|c| c.kind == ContactKind::Person && c.id != self.my_key())
            .collect();
        people.sort_by_cached_key(|c| c.display_name().to_lowercase());
        people
    }

    pub fn blocked(&self) -> Vec<Contact> {
        self.inner.db.contacts_in(&[ContactState::Blocked])
    }

    pub fn contact(&self, id: &str) -> Option<Contact> {
        self.inner.db.contact(id)
    }

    /// Adds someone from their invite, and leaves them a request.
    pub fn add_contact(&self, invite: &str, nickname: Option<&str>) -> anyhow::Result<Contact> {
        self.add_contact_with_greeting(invite, nickname, "")
    }

    /// The same, with a few words so they know who is asking.
    pub fn add_contact_with_greeting(
        &self,
        invite: &str,
        nickname: Option<&str>,
        greeting: &str,
    ) -> anyhow::Result<Contact> {
        let Some(invite) = identity::parse_invite(invite) else {
            bail!("invalid_key");
        };
        let Some(peer) = invite.key() else {
            bail!("invalid_key");
        };
        ensure!(peer != self.inner.me, "own_key");
        let id = identity::key_to_string(&peer);
        let nickname = nickname.map(str::trim).filter(|n| !n.is_empty());
        let current = self.inner.db.contact(&id).map(|c| c.state);
        let state = match current {
            Some(ContactState::Accepted) => ContactState::Accepted,
            // They already asked us: adding them back is accepting.
            Some(ContactState::PendingIn) => ContactState::Accepted,
            _ => ContactState::PendingOut,
        };
        let their_name = invite.name().map(str::to_owned);
        let contact = self
            .inner
            .db
            .upsert_contact(&id, state, their_name.as_deref(), nickname)?;
        self.emit(CoreEvent::ContactsChanged);
        if state == ContactState::Accepted {
            self.emit(CoreEvent::Accepted {
                contact_id: id.clone(),
            });
            self.cmd(Cmd::Accepted(peer));
        } else {
            let mut keys = self.inner.db.contact_keys(&id).unwrap_or_default();
            keys.invite = Some(invite);
            keys.greeting = greeting.chars().take(requests::MAX_GREETING).collect();
            self.inner.db.set_contact_keys(&id, &keys);
            self.cmd(Cmd::SendRequest(peer));
        }
        Ok(contact)
    }

    // ---- our public address ----

    /// The invite behind our plain link and name code.
    pub fn main_invite(&self) -> identity::Invite {
        let mine = self
            .inner
            .db
            .invites()
            .into_iter()
            .find(|i| i.main && !i.revoked)
            .unwrap_or_else(|| {
                let fresh = MyInvite::new("", None, None, true);
                self.inner.db.save_invite(&fresh);
                fresh
            });
        mine.to_invite(&self.inner.me, &self.settings().display_name)
    }

    /// The link that adds us.
    pub fn invite_link(&self) -> String {
        self.main_invite().link()
    }

    /// What a QR code shows when it is meant to be scanned right here: a token for this one
    /// meeting means both are added without asking, and it works once.
    pub fn in_person_code(&self) -> String {
        let mut mine = MyInvite::new("In person", None, Some(1), false);
        mine.met_token = Some(vault::random());
        mine.expires_at = Some(now_ms() + 15 * 60 * 1000);
        self.inner.db.save_invite(&mine);
        mine.to_invite(&self.inner.me, &self.settings().display_name)
            .qr_text()
    }

    /// Our name code, like `roy-7kq4x2`.
    pub fn handle(&self) -> String {
        codes::name_code_text(&self.main_invite())
    }

    /// The link of one of our invites, if it still works.
    pub fn invite_link_for(&self, id: &str) -> Option<String> {
        let mine = self
            .inner
            .db
            .invites()
            .into_iter()
            .find(|i| i.id == id && !i.revoked)?;
        Some(
            mine.to_invite(&self.inner.me, &self.settings().display_name)
                .link(),
        )
    }

    /// A code to read out on the phone: it works once, for a few minutes.
    pub async fn read_out_code(&self) -> String {
        let code = codes::new_read_out();
        let mut mine = MyInvite::new(&code, Some(now_ms() + codes::READ_OUT_MS), Some(1), false);
        mine.label = code.clone();
        self.inner.db.save_invite(&mine);
        let invite = mine.to_invite(&self.inner.me, &self.settings().display_name);
        // There may be no way out yet: the network takes a moment to come up.
        for _ in 0..NET_TRIES {
            let (done, answer) = tokio::sync::oneshot::channel();
            self.cmd(Cmd::PublishCode {
                code: code.clone(),
                invite: invite.clone(),
                done,
            });
            let sent = tokio::time::timeout(Duration::from_secs(15), answer).await;
            if matches!(sent, Ok(Ok(true))) {
                break;
            }
            tokio::time::sleep(NET_WAIT).await;
        }
        code
    }

    /// Whatever someone gave you: a link, a message with one, a name code or a code read out.
    pub async fn add_by_text(&self, text: &str, greeting: &str) -> anyhow::Result<Contact> {
        if identity::parse_invite(text).is_some() {
            return self.add_contact_with_greeting(text, None, greeting);
        }
        ensure!(codes::looks_like_code(text), "invalid_key");
        for _ in 0..NET_TRIES {
            let (reply, answer) = tokio::sync::oneshot::channel();
            self.cmd(Cmd::LookUpCode {
                code: text.to_owned(),
                reply,
            });
            match tokio::time::timeout(Duration::from_secs(20), answer).await {
                Ok(Ok(Ok(Some(invite)))) => {
                    return self.add_contact_with_greeting(&invite.link(), None, greeting);
                }
                Ok(Ok(Ok(None))) => bail!("code_not_found"),
                _ => tokio::time::sleep(NET_WAIT).await,
            }
        }
        bail!("code_not_found")
    }

    pub fn invites(&self) -> Vec<MyInvite> {
        self.inner.db.invites()
    }

    /// Another link, for one place or one person, that can be revoked by itself.
    pub fn new_invite(
        &self,
        label: &str,
        expires_at: Option<Millis>,
        max_uses: Option<u32>,
    ) -> String {
        let mine = MyInvite::new(label, expires_at, max_uses, false);
        self.inner.db.save_invite(&mine);
        mine.to_invite(&self.inner.me, &self.settings().display_name)
            .link()
    }

    pub fn revoke_invite(&self, id: &str) {
        if let Some(mut mine) = self.inner.db.invites().into_iter().find(|i| i.id == id) {
            mine.revoked = true;
            self.inner.db.save_invite(&mine);
        }
    }

    /// "Being bothered": all links and codes stop working, waiting requests go, a new address
    /// comes straight away. Contacts keep talking to us and do nothing: they are told.
    pub fn reset_public_address(&self) -> identity::Invite {
        for contact in self.requests() {
            self.inner.db.delete_contact(&contact.id);
        }
        self.inner.db.delete_invites();
        self.cmd(Cmd::Rotate { network: true });
        self.emit(CoreEvent::ContactsChanged);
        self.main_invite()
    }

    /// The 12 words that bring this person back on another device.
    pub fn recovery_words(&self) -> Vec<String> {
        identity::Identity::load_or_create(&self.inner.db)
            .map(|me| me.recovery_words())
            .unwrap_or_default()
    }

    /// Everything except the files, in one file only the 12 words open.
    pub fn make_backup(&self) -> anyhow::Result<Vec<u8>> {
        let me = identity::Identity::load_or_create(&self.inner.db)?;
        let copy = self.inner.dir.join("backup.tmp");
        self.inner.db.snapshot(&copy)?;
        let read = std::fs::read(&copy);
        let _ = std::fs::remove_file(&copy);
        backup::seal(&me, self.inner.db.vault().key(), read?)
    }

    /// Who you are from the words, with a backup or without. Happens at the next start, which
    /// replaces what is here now. Errors: `wrong_words`, `wrong_backup`.
    pub fn restore_on_next_start(
        &self,
        words: &str,
        backup_file: Option<Vec<u8>>,
    ) -> anyhow::Result<()> {
        backup::leave_for_next_start(&self.inner.dir, self.inner.db.vault(), words, backup_file)
    }

    /// Your own chat: nobody else is in it and it needs no network. Made the first time.
    pub fn notes_chat(&self, name: &str) -> String {
        let me = self.my_key();
        if self.inner.db.contact(&me).is_none() {
            let _ =
                self.inner
                    .db
                    .upsert_contact(&me, ContactState::Accepted, Some(name), Some(name));
            self.emit(CoreEvent::ContactsChanged);
        }
        me
    }

    /// What we know about how this contact got here.
    pub fn contact_keys(&self, id: &str) -> ContactKeys {
        self.inner.db.contact_keys(id).unwrap_or_default()
    }

    pub fn accept(&self, id: &str) -> anyhow::Result<()> {
        let peer = self.peer(id)?;
        self.inner
            .db
            .upsert_contact(id, ContactState::Accepted, None, None)?;
        self.inner
            .db
            .start_with_default_timer(id, self.settings().disappear_default_ms);
        self.emit(CoreEvent::Accepted {
            contact_id: id.to_string(),
        });
        self.emit(CoreEvent::ContactsChanged);
        self.cmd(Cmd::Accepted(peer));
        Ok(())
    }

    /// Declines a request without blocking: they may ask again later.
    pub fn decline(&self, id: &str) -> anyhow::Result<()> {
        let peer = self.peer(id)?;
        self.inner.db.delete_contact(id);
        self.cmd(Cmd::Disconnect(peer));
        self.emit(CoreEvent::ContactsChanged);
        Ok(())
    }

    pub fn block(&self, id: &str) -> anyhow::Result<()> {
        let peer = self.peer(id)?;
        self.inner
            .db
            .upsert_contact(id, ContactState::Blocked, None, None)?;
        self.cmd(Cmd::Disconnect(peer));
        // They knew where we were. Not any more, and contacts are told.
        self.cmd(Cmd::Rotate { network: true });
        self.emit(CoreEvent::ContactsChanged);
        Ok(())
    }

    /// Unblocks and asks to connect again. If they still have us as a
    /// contact, they accept automatically.
    pub fn unblock(&self, id: &str) -> anyhow::Result<()> {
        let peer = self.peer(id)?;
        self.inner
            .db
            .upsert_contact(id, ContactState::PendingOut, None, None)?;
        self.cmd(Cmd::Dial(peer));
        self.emit(CoreEvent::ContactsChanged);
        Ok(())
    }

    pub fn rename(&self, id: &str, nickname: &str) {
        let nickname = nickname.trim();
        self.inner
            .db
            .set_nickname(id, (!nickname.is_empty()).then_some(nickname));
        self.emit(CoreEvent::ContactsChanged);
    }

    /// Deletes the contact and the whole chat from this device.
    pub fn delete_contact(&self, id: &str) -> anyhow::Result<()> {
        if groups::is_group_id(id) {
            return self.delete_group(id);
        }
        let peer = self.peer(id)?;
        self.delete_chat_files(id);
        if let Some(avatar) = self.inner.db.contact(id).and_then(|c| c.avatar) {
            let _ = std::fs::remove_file(avatar);
        }
        self.inner.db.delete_contact(id);
        self.cmd(Cmd::Disconnect(peer));
        self.emit(CoreEvent::ContactsChanged);
        Ok(())
    }

    /// Removes the chat from the chat list and deletes its messages, but keeps
    /// the contact (or group). A new message from them brings the chat back.
    pub fn hide_chat(&self, id: &str) {
        self.delete_chat_files(id);
        self.inner.db.clear_chat(id);
        self.inner.db.set_draft(id, "");
        self.inner.db.set_hidden(id, true);
        self.emit(CoreEvent::ContactsChanged);
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) {
        self.inner.db.set_pinned(id, pinned);
        self.emit(CoreEvent::ContactsChanged);
    }

    pub fn set_muted(&self, id: &str, muted: bool) {
        self.inner.db.set_muted(id, muted);
        self.emit(CoreEvent::ContactsChanged);
    }

    /// Clears the messages but keeps the contact.
    pub fn clear_chat(&self, id: &str) {
        self.delete_chat_files(id);
        self.inner.db.clear_chat(id);
        self.emit(CoreEvent::ContactsChanged);
    }

    fn delete_chat_files(&self, contact_id: &str) {
        for m in self.inner.db.messages(contact_id, None, u32::MAX) {
            self.remove_files(&m);
        }
    }

    fn peer(&self, id: &str) -> anyhow::Result<PublicKey> {
        identity::parse_key(id).ok_or_else(|| anyhow::anyhow!("invalid_key"))
    }

    // ---- messages ----

    /// Up to `limit` messages before `before` (exclusive), oldest first.
    pub fn messages(&self, contact_id: &str, before: Option<Millis>, limit: u32) -> Vec<Message> {
        self.inner.db.messages(contact_id, before, limit)
    }

    /// Stores a message and sends it now, or as soon as the contact is
    /// reachable.
    pub fn send(&self, contact_id: &str, text: &str) -> anyhow::Result<Message> {
        self.send_with(contact_id, text, MessageExtra::default(), None)
    }

    /// A text with a reply, a mention, no sound. `at`: not now but at this moment.
    pub fn send_with(
        &self,
        contact_id: &str,
        text: &str,
        mut extra: MessageExtra,
        at: Option<Millis>,
    ) -> anyhow::Result<Message> {
        let text = text.trim();
        ensure!(!text.is_empty(), "empty");
        ensure!(text.len() <= protocol::MAX_TEXT, "too_long");
        let peer = self.recipient(contact_id)?;
        extra.disappear_ms = self.inner.db.chat_prefs(contact_id).disappear_ms;
        let now = now_ms();
        let later = at.filter(|at| *at > now);
        let to_self = contact_id == self.my_key();
        let message = Message {
            id: uuid::Uuid::now_v7().to_string(),
            contact_id: contact_id.to_string(),
            outgoing: true,
            body: text.to_string(),
            sent_at: later.unwrap_or(now),
            received_at: now,
            status: if later.is_some() {
                MessageStatus::Scheduled
            } else if to_self {
                MessageStatus::Read
            } else {
                MessageStatus::Queued
            },
            read: true,
            attachment: None,
            sender: None,
            extra,
        };
        self.inner.db.insert_message(&message)?;
        self.inner.db.set_draft(contact_id, "");
        if later.is_some() {
            self.cmd(Cmd::TimersChanged);
            self.emit(CoreEvent::ContactsChanged);
            return Ok(message);
        }
        if message.extra.disappear_ms.is_some() {
            self.cmd(Cmd::TimersChanged);
        }
        self.emit(CoreEvent::MessageAdded(message.clone()));
        self.emit(CoreEvent::ContactsChanged);
        if !to_self {
            self.dispatch(contact_id, peer, &message);
        }
        Ok(message)
    }

    pub fn send_reply(
        &self,
        contact_id: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> anyhow::Result<Message> {
        let extra = MessageExtra {
            reply_to: reply_to.map(str::to_owned),
            ..MessageExtra::default()
        };
        self.send_with(contact_id, text, extra, None)
    }

    pub fn send_later(&self, contact_id: &str, text: &str, at: Millis) -> anyhow::Result<Message> {
        self.send_with(contact_id, text, MessageExtra::default(), Some(at))
    }

    /// A poll, list, plan, place or contact card. Its one line is what older versions show.
    pub fn send_rich(&self, contact_id: &str, rich: Rich) -> anyhow::Result<Message> {
        let summary = rich.summary().to_owned();
        let extra = MessageExtra {
            rich: Some(rich),
            ..MessageExtra::default()
        };
        self.send_with(contact_id, &summary, extra, None)
    }

    /// Messages waiting for their moment.
    pub fn scheduled(&self) -> Vec<Message> {
        self.inner.db.scheduled()
    }

    pub fn cancel_scheduled(&self, message_id: &str) {
        if self
            .inner
            .db
            .message(message_id)
            .is_some_and(|m| m.status == MessageStatus::Scheduled)
        {
            self.inner.db.delete_message(message_id);
            self.cmd(Cmd::TimersChanged);
            self.emit(CoreEvent::ContactsChanged);
        }
    }

    /// Now instead of later.
    pub fn send_scheduled_now(&self, message_id: &str) {
        if let Some(mut m) = self.inner.db.message(message_id)
            && m.status == MessageStatus::Scheduled
        {
            m.sent_at = now_ms();
            self.inner.db.set_scheduled_time(message_id, m.sent_at);
            self.cmd(Cmd::TimersChanged);
        }
    }

    // ---- what happens to a message later ----

    /// Who gets to hear about something in this chat.
    fn people_in(&self, chat: &str) -> Vec<String> {
        if groups::is_group_id(chat) {
            let me = self.my_key();
            return self
                .inner
                .db
                .group(chat)
                .map(|(state, _, _)| state.members.into_iter().filter(|k| *k != me).collect())
                .unwrap_or_default();
        }
        vec![chat.to_owned()]
    }

    fn tell_chat(&self, chat: &str, target: &str, change: &MessageChange) {
        let peers = self.people_in(chat);
        let group = groups::is_group_id(chat).then_some(chat);
        let id = uuid::Uuid::now_v7().to_string();
        let Ok(bytes) = serde_json::to_vec(change) else {
            return;
        };
        self.inner
            .db
            .queue_change(&id, &peers, target, group, &bytes);
        self.cmd(Cmd::SendChanges(
            peers
                .iter()
                .filter_map(|p| identity::parse_key(p))
                .collect(),
        ));
    }

    fn changed(&self, m: Message) {
        self.inner.db.update_message(&m);
        self.emit(CoreEvent::MessageUpdated(m));
        self.emit(CoreEvent::ContactsChanged);
    }

    fn is_mine(&self, m: &Message) -> bool {
        m.outgoing
    }

    /// `None` takes your reaction back.
    pub fn react(&self, message_id: &str, emoji: Option<&str>) {
        let Some(mut m) = self.inner.db.message(message_id) else {
            return;
        };
        m.extra.react(&self.my_key(), emoji);
        let chat = m.contact_id.clone();
        self.changed(m);
        self.tell_chat(
            &chat,
            message_id,
            &MessageChange::React(emoji.map(str::to_owned)),
        );
    }

    /// Only what you wrote yourself.
    pub fn edit_message(&self, message_id: &str, text: &str) -> anyhow::Result<()> {
        let text = text.trim();
        ensure!(!text.is_empty(), "empty");
        ensure!(text.len() <= protocol::MAX_TEXT, "too_long");
        let mut m = self.inner.db.message(message_id).context("no_message")?;
        ensure!(self.is_mine(&m) && !m.extra.unsent, "not_yours");
        m.body = text.to_owned();
        let waiting = m.status == MessageStatus::Scheduled;
        m.extra.edited = !waiting;
        let chat = m.contact_id.clone();
        self.changed(m);
        if !waiting {
            self.tell_chat(&chat, message_id, &MessageChange::Edit(text.to_owned()));
        }
        Ok(())
    }

    /// Takes it back for everyone. What is left says so.
    pub fn unsend_message(&self, message_id: &str) -> anyhow::Result<()> {
        let mut m = self.inner.db.message(message_id).context("no_message")?;
        ensure!(self.is_mine(&m), "not_yours");
        self.remove_files(&m);
        m.body = String::new();
        m.attachment = None;
        m.extra.rich = None;
        m.extra.unsent = true;
        let chat = m.contact_id.clone();
        self.changed(m);
        self.tell_chat(&chat, message_id, &MessageChange::Unsend);
        Ok(())
    }

    /// A vote, a mark on a list, an answer to a plan.
    pub fn change_rich(&self, message_id: &str, change: RichChange) {
        let Some(mut m) = self.inner.db.message(message_id) else {
            return;
        };
        let me = self.my_key();
        let Some(rich) = &mut m.extra.rich else {
            return;
        };
        rich.apply(&me, &change);
        let chat = m.contact_id.clone();
        self.changed(m);
        self.tell_chat(&chat, message_id, &MessageChange::Rich(change));
    }

    // ---- how a chat is treated here ----

    pub fn chat_prefs(&self, chat: &str) -> ChatPrefs {
        self.inner.db.chat_prefs(chat)
    }

    /// A new disappearing time goes for the others as well.
    pub fn set_chat_prefs(&self, chat: &str, prefs: &ChatPrefs) {
        let before = self.inner.db.chat_prefs(chat);
        self.inner.db.set_chat_prefs(chat, prefs);
        if before.disappear_ms != prefs.disappear_ms {
            self.tell_chat(chat, "", &MessageChange::ChatTimer(prefs.disappear_ms));
        }
        self.emit(CoreEvent::ContactsChanged);
    }

    pub fn folders(&self) -> Vec<Folder> {
        self.inner
            .db
            .secret("folders")
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default()
    }

    pub fn save_folders(&self, folders: &[Folder]) {
        if let Ok(json) = serde_json::to_vec(folders) {
            let _ = self.inner.db.set_secret("folders", &json);
        }
        self.emit(CoreEvent::ContactsChanged);
    }

    /// The chats a folder holds: whatever matches any of its rules, and what was put there.
    pub fn chats_in(&self, folder: &Folder) -> Vec<ChatSummary> {
        self.chats()
            .into_iter()
            .filter(|c| {
                let group = c.contact.kind == ContactKind::Group;
                folder.chats.contains(&c.contact.id)
                    || (folder.groups && group)
                    || (folder.people && !group)
                    || (folder.unread && (c.unread > 0 || c.prefs.marked_unread))
                    || folder.labels.iter().any(|l| c.prefs.labels.contains(l))
            })
            .collect()
    }

    // ---- saved and reminded ----

    /// On your own list, with these words. Nobody else hears of it.
    pub fn save_message(&self, message_id: &str, tags: &[String]) {
        if let Some(mut m) = self.inner.db.message(message_id) {
            m.extra.saved = true;
            m.extra.tags = tags.to_vec();
            self.inner.db.update_message(&m);
            self.emit(CoreEvent::MessageUpdated(m));
        }
    }

    pub fn unsave_message(&self, message_id: &str) {
        if let Some(mut m) = self.inner.db.message(message_id) {
            m.extra.saved = false;
            m.extra.tags = Vec::new();
            self.inner.db.update_message(&m);
            self.emit(CoreEvent::MessageUpdated(m));
        }
    }

    pub fn saved_messages(&self) -> Vec<Message> {
        self.inner.db.saved_messages()
    }

    pub fn remind_me(&self, message_id: &str, at: Millis) {
        self.inner.db.add_reminder(message_id, at, false);
        self.cmd(Cmd::TimersChanged);
    }

    /// Only if nothing came back from them by then.
    pub fn remind_if_no_answer(&self, message_id: &str, at: Millis) {
        self.inner.db.add_reminder(message_id, at, true);
        self.cmd(Cmd::TimersChanged);
    }

    pub fn cancel_reminder(&self, message_id: &str) {
        self.inner.db.drop_reminder(message_id);
        self.cmd(Cmd::TimersChanged);
    }

    /// Message, moment: soonest first.
    pub fn reminders(&self) -> Vec<(String, Millis)> {
        self.inner.db.reminders()
    }

    // ---- finding ----

    /// What fits, newest first. People before messages.
    pub fn search(&self, query: &str, within: SearchIn) -> Vec<SearchHit> {
        const MOST: usize = 500;
        let query = query.trim().to_lowercase();
        let mut hits = Vec::new();
        if matches!(within, SearchIn::All | SearchIn::People) && !query.is_empty() {
            for c in self.inner.db.contacts_in(&[ContactState::Accepted]) {
                if c.display_name().to_lowercase().contains(&query) {
                    hits.push(SearchHit::Person(c.id));
                }
            }
        }
        if within == SearchIn::People {
            return hits;
        }
        for m in self.inner.db.searchable(None) {
            if hits.len() >= MOST {
                break;
            }
            if message_fits(&m, &query, within) {
                hits.push(SearchHit::Message {
                    chat: m.contact_id,
                    id: m.id,
                });
            }
        }
        hits
    }

    /// One chat: what fits, newest first. Without words, everything of the kind.
    pub fn search_chat(&self, chat: &str, query: &str, within: SearchIn) -> Vec<Message> {
        let query = query.trim().to_lowercase();
        self.inner
            .db
            .searchable(Some(chat))
            .into_iter()
            .filter(|m| message_fits(m, &query, within))
            .collect()
    }

    // ---- calls that were ----

    /// Who of a group has it: how many, of how many.
    pub fn group_acks(&self, message_id: &str) -> (u32, u32) {
        self.inner.db.group_ack_counts(message_id)
    }

    // ---- how much is kept ----

    /// What the files take, by kind and by chat: the largest chat first.
    pub fn storage_report(&self) -> StorageReport {
        let mut report = StorageReport::default();
        let mut chats: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for m in self.inner.db.searchable(None) {
            let Some(a) = &m.attachment else { continue };
            if a.path.is_none() {
                continue;
            }
            match a.kind() {
                MediaKind::Image | MediaKind::Video => report.media += a.size,
                MediaKind::Audio => report.voice += a.size,
                MediaKind::File => report.files += a.size,
            }
            *chats.entry(m.contact_id).or_default() += a.size;
        }
        report.chats = chats.into_iter().collect();
        report
            .chats
            .sort_by_key(|(_, size)| std::cmp::Reverse(*size));
        report
    }

    // ---- being sure who someone is ----

    /// Six words for this contact and you, the same on both sides whatever else happens. Reading
    /// them to each other proves there is nobody in between.
    pub fn safety_words(&self, contact_id: &str) -> Vec<String> {
        let me = self.my_key();
        let (first, second) = if me.as_str() < contact_id {
            (me.as_str(), contact_id)
        } else {
            (contact_id, me.as_str())
        };
        let mut hasher = blake3::Hasher::new_derive_key("whatsmine safety words v1");
        hasher.update(first.as_bytes());
        hasher.update(second.as_bytes());
        let hash = hasher.finalize();
        identity::words_for(&hash.as_bytes()[..12])
            .into_iter()
            .take(6)
            .collect()
    }

    pub fn mark_safety_words(&self, contact_id: &str) {
        let mut keys = self.inner.db.contact_keys(contact_id).unwrap_or_default();
        keys.trust = Trust::SafetyWords;
        self.inner.db.set_contact_keys(contact_id, &keys);
        self.emit(CoreEvent::ContactsChanged);
    }

    /// What this contact gave to pass them on with, if they did.
    pub fn contact_card(&self, contact_id: &str) -> Option<String> {
        self.inner
            .db
            .contact_keys(contact_id)
            .and_then(|k| k.card)
            .filter(|c| !c.is_empty())
    }

    // ---- calls that were ----

    /// Newest first.
    pub fn call_records(&self) -> Vec<CallRecord> {
        self.inner.db.call_records()
    }

    pub fn delete_call_record(&self, id: &str) {
        self.inner.db.delete_call_record(id);
    }

    /// Marks the chat as read (sends read receipts if enabled).
    pub fn mark_read(&self, contact_id: &str) {
        if groups::is_group_id(contact_id) {
            // Nobody is told when we read a group message.
            if !self.inner.db.mark_read(contact_id).is_empty() {
                self.emit(CoreEvent::ContactsChanged);
            }
            return;
        }
        let Ok(peer) = self.peer(contact_id) else {
            return;
        };
        if !self.inner.db.mark_read(contact_id).is_empty() {
            self.emit(CoreEvent::ContactsChanged);
        }
        self.cmd(Cmd::ReadReceipts(peer));
    }

    /// Call while the user types; throttled inside.
    pub fn typing(&self, contact_id: &str) {
        if let Ok(peer) = self.peer(contact_id) {
            self.cmd(Cmd::Typing(peer));
        }
    }

    /// Deletes a message on this device only (and its file, if any).
    pub fn delete_message(&self, message_id: &str) {
        if let Some(m) = self.inner.db.message(message_id) {
            self.remove_files(&m);
        }
        self.inner.db.delete_message(message_id);
        self.emit(CoreEvent::ContactsChanged);
    }

    pub fn message(&self, message_id: &str) -> Option<Message> {
        self.inner.db.message(message_id)
    }

    /// Where files are kept on this device.
    /// The folder everything of this identity lives in.
    pub fn data_dir(&self) -> &Path {
        &self.inner.dir
    }

    pub fn media_dir(&self) -> PathBuf {
        files::media_dir(&self.inner.dir)
    }

    /// Sends a file (photo, video, voice note, document). It is copied into
    /// WhatsMine's own storage first, so the original may be moved or
    /// deleted afterwards. Blocks while copying: call it off the UI thread
    /// for big files.
    pub fn send_file(
        &self,
        contact_id: &str,
        src: &Path,
        name: &str,
        mime: &str,
        caption: &str,
        duration_ms: Option<u64>,
    ) -> anyhow::Result<Message> {
        self.send_file_from(contact_id, src, name, mime, caption, duration_ms, false)
    }

    /// Like [`send_file`](Self::send_file), but when `take_ownership` is set
    /// the file is *moved* into WhatsMine's storage when possible (same
    /// filesystem) instead of copied. Use it for scratch files the caller
    /// would delete anyway, such as a fresh recording or a camera capture:
    /// it saves writing a 100 MB file twice.
    #[allow(clippy::too_many_arguments)]
    pub fn send_file_from(
        &self,
        contact_id: &str,
        src: &Path,
        name: &str,
        mime: &str,
        caption: &str,
        duration_ms: Option<u64>,
        take_ownership: bool,
    ) -> anyhow::Result<Message> {
        let caption = caption.trim();
        ensure!(caption.len() <= protocol::MAX_TEXT, "too_long");
        let peer = self.recipient(contact_id)?;
        let id = uuid::Uuid::now_v7().to_string();
        let name = files::clean_file_name(name);
        let dest = files::final_path(&self.media_dir(), &id, &name);
        // Photos go out upright and without location data.
        let lower = name.to_ascii_lowercase();
        let is_jpeg = mime.eq_ignore_ascii_case("image/jpeg")
            || lower.ends_with(".jpg")
            || lower.ends_with(".jpeg");
        let cleaned = dest.with_extension("clean.tmp");
        let (src, take_ownership) =
            if is_jpeg && matches!(files::clean_photo(src, &cleaned), Ok(true)) {
                if take_ownership {
                    let _ = std::fs::remove_file(src);
                }
                (cleaned.as_path(), true)
            } else {
                (src, take_ownership)
            };
        let (size, sha) = files::import(src, &dest, take_ownership).context("file_unreadable")?;
        if size == 0 {
            let _ = std::fs::remove_file(&dest);
            bail!("empty_file");
        }
        let now = now_ms();
        let message = Message {
            id,
            contact_id: contact_id.to_string(),
            outgoing: true,
            body: caption.to_string(),
            sent_at: now,
            received_at: now,
            status: if contact_id == self.my_key() {
                MessageStatus::Read
            } else {
                MessageStatus::Queued
            },
            read: true,
            attachment: Some(Attachment {
                name,
                mime: files::clean_mime(mime),
                size,
                sha256: files::to_hex(&sha),
                path: Some(dest.to_string_lossy().into_owned()),
                state: if contact_id == self.my_key() {
                    TransferState::Done
                } else {
                    TransferState::Transferring
                },
                duration_ms,
            }),
            sender: None,
            extra: MessageExtra {
                disappear_ms: self.inner.db.chat_prefs(contact_id).disappear_ms,
                ..MessageExtra::default()
            },
        };
        self.inner.db.insert_message(&message)?;
        if message.extra.disappear_ms.is_some() {
            self.cmd(Cmd::TimersChanged);
        }
        self.emit(CoreEvent::MessageAdded(message.clone()));
        self.emit(CoreEvent::ContactsChanged);
        if contact_id != self.my_key() {
            self.dispatch(contact_id, peer, &message);
        }
        Ok(message)
    }

    /// Asks for a failed incoming file again, from the start.
    pub fn retry_file(&self, message_id: &str) {
        let Some(m) = self.inner.db.message(message_id) else {
            return;
        };
        if m.outgoing || m.attachment.is_none() {
            return;
        }
        if let Ok(peer) = self.peer(m.sender.as_deref().unwrap_or(&m.contact_id)) {
            self.cmd(Cmd::RetryFile {
                contact: peer,
                message_id: message_id.to_string(),
            });
        }
    }

    /// Sets our profile picture from any image file (cropped to a square,
    /// made small) and shares it with contacts.
    pub fn set_avatar(&self, src: &Path) -> anyhow::Result<()> {
        let bytes = files::make_avatar(src).context("not_an_image")?;
        let path = self.inner.dir.join("avatar.jpg");
        let tmp = self.inner.dir.join("avatar.jpg.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        let mut s = self.settings();
        s.avatar = Some(path.to_string_lossy().into_owned());
        self.save_settings(&s)
    }

    pub fn clear_avatar(&self) -> anyhow::Result<()> {
        let mut s = self.settings();
        if let Some(p) = s.avatar.take() {
            let _ = std::fs::remove_file(p);
        }
        self.save_settings(&s)
    }

    /// Deletes a message's local file and any partial download.
    fn remove_files(&self, m: &Message) {
        if let Some(path) = m.attachment.as_ref().and_then(|a| a.path.as_ref()) {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(files::partial_path(&self.media_dir(), &m.id));
    }

    // ---- groups ----

    /// Who a message to `contact_id` goes to: `Some(key)` for a person,
    /// `None` for a group we are still in.
    fn recipient(&self, contact_id: &str) -> anyhow::Result<Option<PublicKey>> {
        if groups::is_group_id(contact_id) {
            ensure!(
                self.inner
                    .db
                    .group(contact_id)
                    .is_some_and(|(_, _, active)| active),
                "not_a_contact"
            );
            return Ok(None);
        }
        let peer = self.peer(contact_id)?;
        match self.inner.db.contact(contact_id).map(|c| c.state) {
            Some(ContactState::Accepted | ContactState::PendingOut) => Ok(Some(peer)),
            _ => bail!("not_a_contact"),
        }
    }

    /// Hands a stored outgoing message to the network.
    fn dispatch(&self, contact_id: &str, peer: Option<PublicKey>, m: &Message) {
        match peer {
            Some(contact) => self.cmd(Cmd::SendOne {
                contact,
                message_id: m.id.clone(),
            }),
            None => {
                let me = self.my_key();
                if let Some((state, _, _)) = self.inner.db.group(contact_id) {
                    let others: Vec<String> =
                        state.members.into_iter().filter(|k| *k != me).collect();
                    self.inner.db.add_group_acks(&m.id, &others);
                }
                self.cmd(Cmd::SendGroup {
                    group: contact_id.to_string(),
                    message_id: m.id.clone(),
                });
            }
        }
    }

    /// The name a person chose for themselves: what we may tell others.
    /// (Our private nicknames for people never leave this device.)
    fn public_name(&self, id: &str) -> String {
        if id == self.my_key() {
            return self.settings().display_name;
        }
        self.inner
            .db
            .contact(id)
            .map(|c| c.their_name)
            .unwrap_or_default()
    }

    fn name_of(&self, id: &str) -> String {
        if id == self.my_key() {
            return self.settings().display_name;
        }
        self.inner
            .db
            .contact(id)
            .map(|c| c.display_name())
            .unwrap_or_else(|| model::short_id(id))
    }

    /// Sends a group's signed state to its members (and to also_to).
    fn broadcast_group(&self, state: &protocol::GroupState, sig: Vec<u8>, also_to: Vec<PublicKey>) {
        let me = self.my_key();
        let mut to: Vec<PublicKey> = state
            .members
            .iter()
            .filter(|m| **m != me)
            .filter_map(|m| identity::parse_key(m))
            .collect();
        to.extend(also_to);
        let names = state
            .members
            .iter()
            .map(|k| (k.clone(), self.public_name(k)))
            .collect();
        self.cmd(Cmd::GroupPublish {
            state: state.clone(),
            sig,
            names,
            to,
        });
    }

    /// Creates a group of our contacts. We become its admin.
    pub fn create_group(&self, name: &str, members: &[String]) -> anyhow::Result<Contact> {
        let name = engine::clean_name(name);
        ensure!(!name.is_empty(), "empty");
        let me = self.my_key();
        let mut keys: Vec<String> = Vec::new();
        for m in members {
            ensure!(
                self.inner.db.contact(m).is_some_and(|c| {
                    c.kind == model::ContactKind::Person && c.state == ContactState::Accepted
                }),
                "not_a_contact"
            );
            if !keys.contains(m) {
                keys.push(m.clone());
            }
        }
        ensure!(!keys.is_empty(), "no_members");
        ensure!(keys.len() < protocol::MAX_GROUP_MEMBERS, "too_many");
        keys.push(me.clone());
        keys.sort();
        let state = protocol::GroupState {
            id: groups::new_group_id(),
            name,
            admin: me.clone(),
            version: 1,
            members: keys,
            closed: false,
        };
        let sig = groups::sign(&self.inner.key, &state);
        self.inner.db.save_group(&state, &sig, true, &[], &me)?;
        self.inner
            .db
            .start_with_default_timer(&state.id, self.settings().disappear_default_ms);
        self.inner
            .db
            .set_secret(&groups::secret_name(&state.id), &vault::random::<32>())?;
        self.emit(CoreEvent::ContactsChanged);
        self.broadcast_group(&state, sig, Vec::new());
        self.inner
            .db
            .contact(&state.id)
            .context("group was not stored")
    }

    pub fn group_info(&self, id: &str) -> Option<GroupInfo> {
        let (state, _, active) = self.inner.db.group(id)?;
        let me = self.my_key();
        let members = state
            .members
            .iter()
            .map(|k| GroupMember {
                id: k.clone(),
                name: self.name_of(k),
                is_me: *k == me,
                is_admin: *k == state.admin,
                online: self.is_online(k),
                is_contact: self
                    .inner
                    .db
                    .contact(k)
                    .is_some_and(|c| c.state == ContactState::Accepted),
            })
            .collect();
        Some(GroupInfo {
            id: state.id.clone(),
            name: self
                .inner
                .db
                .contact(id)
                .map(|c| c.display_name())
                .unwrap_or(state.name),
            admin: state.admin.clone(),
            i_am_admin: state.admin == me,
            active,
            members,
        })
    }

    /// The group's state, if we administer it and are still in it.
    fn admin_state(&self, id: &str) -> anyhow::Result<protocol::GroupState> {
        let (state, _, active) = self.inner.db.group(id).context("not a group")?;
        ensure!(active && state.admin == self.my_key(), "not_admin");
        Ok(state)
    }

    fn publish_new_state(
        &self,
        mut state: protocol::GroupState,
        also_to: Vec<PublicKey>,
    ) -> anyhow::Result<()> {
        state.version += 1;
        let sig = groups::sign(&self.inner.key, &state);
        let me = self.my_key();
        let active = state.members.contains(&me) && !state.closed;
        self.inner.db.save_group(&state, &sig, active, &[], &me)?;
        self.emit(CoreEvent::ContactsChanged);
        self.broadcast_group(&state, sig, also_to);
        Ok(())
    }

    /// Admin only: adds one of our contacts to the group.
    pub fn group_add(&self, id: &str, member: &str) -> anyhow::Result<()> {
        let mut state = self.admin_state(id)?;
        ensure!(!state.members.iter().any(|m| m == member), "already_member");
        ensure!(
            state.members.len() < protocol::MAX_GROUP_MEMBERS,
            "too_many"
        );
        ensure!(
            self.inner.db.contact(member).is_some_and(|c| {
                c.kind == model::ContactKind::Person && c.state == ContactState::Accepted
            }),
            "not_a_contact"
        );
        state.members.push(member.to_string());
        state.members.sort();
        self.publish_new_state(state, Vec::new())
    }

    /// Admin only: removes a member. They are told if they are online.
    pub fn group_remove(&self, id: &str, member: &str) -> anyhow::Result<()> {
        let mut state = self.admin_state(id)?;
        ensure!(member != self.my_key(), "cannot_remove_self");
        ensure!(state.members.iter().any(|m| m == member), "not_a_member");
        state.members.retain(|m| m != member);
        self.inner.db.remove_member_acks(id, member);
        let removed = identity::parse_key(member).into_iter().collect();
        // They knew how the others find each other.
        self.inner
            .db
            .set_secret(&groups::secret_name(id), &vault::random::<32>())?;
        self.publish_new_state(state, removed)?;
        self.inner.db.prune_group_peers();
        Ok(())
    }

    /// Admin only.
    pub fn group_rename(&self, id: &str, name: &str) -> anyhow::Result<()> {
        let mut state = self.admin_state(id)?;
        let name = engine::clean_name(name);
        ensure!(!name.is_empty(), "empty");
        state.name = name;
        self.publish_new_state(state, Vec::new())
    }

    /// Leaves the group (history stays). If we are its admin, the group ends
    /// for everyone.
    pub fn leave_group(&self, id: &str) -> anyhow::Result<()> {
        let (mut state, _, active) = self.inner.db.group(id).context("not a group")?;
        if !active {
            return Ok(());
        }
        if state.admin == self.my_key() {
            state.closed = true;
            return self.publish_new_state(state, Vec::new());
        }
        self.inner.db.set_group_active(id, false);
        self.inner.db.set_group_left(id, true);
        if let Some(admin) = identity::parse_key(&state.admin) {
            self.cmd(Cmd::GroupLeave {
                group: id.to_string(),
                admin,
            });
        }
        self.emit(CoreEvent::ContactsChanged);
        Ok(())
    }

    /// Leaves (if still in) and removes the group and its history from this device.
    pub fn delete_group(&self, id: &str) -> anyhow::Result<()> {
        self.leave_group(id)?;
        self.delete_chat_files(id);
        self.inner.db.delete_group(id);
        self.emit(CoreEvent::ContactsChanged);
        Ok(())
    }

    pub fn draft(&self, contact_id: &str) -> String {
        self.inner.db.draft(contact_id)
    }

    pub fn set_draft(&self, contact_id: &str, text: &str) {
        self.inner.db.set_draft(contact_id, text);
    }
}
