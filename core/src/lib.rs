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

pub mod db;
mod engine;
pub mod files;
pub mod identity;
pub mod model;
pub mod paths;
pub mod protocol;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use iroh::PublicKey;
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
    recovery: DbRecovery,
    cmds: mpsc::UnboundedSender<Cmd>,
    events: broadcast::Sender<CoreEvent>,
    shared: Shared,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
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
        let shared = Shared {
            online: Arc::new(Mutex::new(HashSet::new())),
            net: Arc::new(Mutex::new(NetStatus::Connecting)),
        };
        runtime.spawn(Engine::run(
            db.clone(),
            key.clone(),
            mode,
            events.clone(),
            shared.clone(),
            files::media_dir(dir),
            cmd_rx,
        ));

        Ok(Core {
            inner: Arc::new(Inner {
                dir: dir.to_path_buf(),
                db,
                me: key.public(),
                recovery,
                cmds,
                events,
                shared,
                runtime: Mutex::new(Some(runtime)),
            }),
        })
    }

    fn cmd(&self, cmd: Cmd) {
        let _ = self.inner.cmds.send(cmd);
    }

    fn emit(&self, ev: CoreEvent) {
        let _ = self.inner.events.send(ev);
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
        let peer = self.peer(contact_id)?;
        match self.inner.db.contact(contact_id).map(|c| c.state) {
            Some(ContactState::Accepted | ContactState::PendingOut) => {}
            _ => bail!("not_a_contact"),
        }
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
        };
        self.inner.db.insert_message(&message)?;
        self.inner.db.set_draft(contact_id, "");
        self.emit(CoreEvent::MessageAdded(message.clone()));
        self.emit(CoreEvent::ContactsChanged);
        self.cmd(Cmd::SendOne {
            contact: peer,
            message_id: message.id.clone(),
        });
        Ok(message)
    }

    /// Marks the chat as read (sends read receipts if enabled).
    pub fn mark_read(&self, contact_id: &str) {
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
        let caption = caption.trim();
        ensure!(caption.len() <= protocol::MAX_TEXT, "too_long");
        let peer = self.peer(contact_id)?;
        match self.inner.db.contact(contact_id).map(|c| c.state) {
            Some(ContactState::Accepted | ContactState::PendingOut) => {}
            _ => bail!("not_a_contact"),
        }
        let id = uuid::Uuid::now_v7().to_string();
        let name = files::clean_file_name(name);
        let dest = files::final_path(&self.media_dir(), &id, &name);
        let (size, sha) = files::copy_and_hash(src, &dest).context("file_unreadable")?;
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
        };
        self.inner.db.insert_message(&message)?;
        self.emit(CoreEvent::MessageAdded(message.clone()));
        self.emit(CoreEvent::ContactsChanged);
        self.cmd(Cmd::SendOne {
            contact: peer,
            message_id: message.id.clone(),
        });
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
        if let Ok(peer) = self.peer(&m.contact_id) {
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

    pub fn draft(&self, contact_id: &str) -> String {
        self.inner.db.draft(contact_id)
    }

    pub fn set_draft(&self, contact_id: &str, text: &str) {
        self.inner.db.set_draft(contact_id, text);
    }
}
