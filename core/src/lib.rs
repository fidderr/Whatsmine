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

pub mod calls;
pub mod db;
mod engine;
pub mod files;
pub mod groups;
pub mod identity;
pub mod model;
pub mod paths;
pub mod power;
pub mod protocol;
pub mod retry;
#[cfg(feature = "test-hooks")]
mod seed;

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

pub const DB_FILE: &str = "whatsmine.db";

/// How the core reaches the network.
#[derive(Clone, Debug, Default)]
pub enum NetMode {
    /// The real internet: n0 DNS + Mainline DHT + mDNS discovery, relays as
    /// fallback.
    #[default]
    Internet,
    /// Loopback only, addresses shared in memory. For tests.
    Local(iroh::address_lookup::MemoryLookup),
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
        let key = identity::load_or_create(dir).context("loading identity")?;
        let (db, recovery) = Database::open_with_recovery(&dir.join(DB_FILE));
        let db = Arc::new(db);

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
        };
        runtime.spawn(Engine::run(
            db.clone(),
            key.clone(),
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
        std::fs::remove_file(self.inner.dir.join(identity::KEY_FILE))?;
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
            .filter(|c| c.kind == ContactKind::Person)
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

    /// Adds someone by their key and asks them to connect.
    pub fn add_contact(&self, key: &str, nickname: Option<&str>) -> anyhow::Result<Contact> {
        let Some(peer) = identity::parse_key(key) else {
            bail!("invalid_key");
        };
        ensure!(peer != self.inner.me, "own_key");
        let id = identity::key_to_string(&peer);
        let nickname = nickname.map(str::trim).filter(|n| !n.is_empty());
        let state = match self.inner.db.contact(&id).map(|c| c.state) {
            Some(ContactState::Accepted) => ContactState::Accepted,
            // They already asked us: adding them back is accepting.
            Some(ContactState::PendingIn) => ContactState::Accepted,
            _ => ContactState::PendingOut,
        };
        let contact = self.inner.db.upsert_contact(&id, state, None, nickname)?;
        self.emit(CoreEvent::ContactsChanged);
        if state == ContactState::Accepted {
            self.emit(CoreEvent::Accepted {
                contact_id: id.clone(),
            });
            self.cmd(Cmd::Accepted(peer));
        } else {
            self.cmd(Cmd::Dial(peer));
        }
        Ok(contact)
    }

    pub fn accept(&self, id: &str) -> anyhow::Result<()> {
        let peer = self.peer(id)?;
        self.inner
            .db
            .upsert_contact(id, ContactState::Accepted, None, None)?;
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
        let text = text.trim();
        ensure!(!text.is_empty(), "empty");
        ensure!(text.len() <= protocol::MAX_TEXT, "too_long");
        let peer = self.recipient(contact_id)?;
        let now = now_ms();
        let message = Message {
            id: uuid::Uuid::now_v7().to_string(),
            contact_id: contact_id.to_string(),
            outgoing: true,
            body: text.to_string(),
            sent_at: now,
            received_at: now,
            status: MessageStatus::Queued,
            read: true,
            attachment: None,
            sender: None,
        };
        self.inner.db.insert_message(&message)?;
        self.inner.db.set_draft(contact_id, "");
        self.emit(CoreEvent::MessageAdded(message.clone()));
        self.emit(CoreEvent::ContactsChanged);
        self.dispatch(contact_id, peer, &message);
        Ok(message)
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
            status: MessageStatus::Queued,
            read: true,
            attachment: Some(Attachment {
                name,
                mime: files::clean_mime(mime),
                size,
                sha256: files::to_hex(&sha),
                path: Some(dest.to_string_lossy().into_owned()),
                state: TransferState::Transferring,
                duration_ms,
            }),
            sender: None,
        };
        self.inner.db.insert_message(&message)?;
        self.emit(CoreEvent::MessageAdded(message.clone()));
        self.emit(CoreEvent::ContactsChanged);
        self.dispatch(contact_id, peer, &message);
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
