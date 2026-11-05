//! Local storage. Everything lives in one SQLite file on this device.
//!
//! Patterns ported from AlarmFree: `PRAGMA user_version` migrations, WAL, and
//! open-with-recovery (a corrupt file is quarantined, not deleted).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::model::{
    Attachment, ChatSummary, Contact, ContactKind, ContactState, Message, MessageStatus, Millis,
    Settings, TransferState, now_ms,
};
use crate::model::{ContactKeys, MyInvite};
use crate::protocol::GroupState;
use crate::rich::{CallRecord, ChatPrefs, MessageExtra};
use crate::vault::Vault;

const CURRENT_VERSION: u32 = 5;

/// The clean start of this generation removed everything older, so there is one schema. Sealed
/// with the vault: names, message text, attachment details, drafts, settings, keys.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS settings (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS contacts (
    id             TEXT PRIMARY KEY,
    their_name     BLOB,
    nickname       BLOB,
    state          TEXT NOT NULL,
    last_seen      INTEGER,
    created_at     INTEGER NOT NULL,
    last_activity  INTEGER NOT NULL,
    avatar         TEXT,
    kind           TEXT NOT NULL DEFAULT 'person',
    hidden         INTEGER NOT NULL DEFAULT 0,
    pinned         INTEGER NOT NULL DEFAULT 0,
    muted          INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS messages (
    id                 TEXT PRIMARY KEY,
    contact_id         TEXT NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    outgoing           INTEGER NOT NULL,
    body               BLOB NOT NULL,
    sent_at            INTEGER NOT NULL,
    received_at        INTEGER NOT NULL,
    status             INTEGER NOT NULL DEFAULT 0,
    read               INTEGER NOT NULL DEFAULT 0,
    read_receipt_sent  INTEGER NOT NULL DEFAULT 0,
    attachment         BLOB,
    sender             TEXT,
    extra              BLOB,
    expires_at         INTEGER,
    saved              INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS messages_by_contact ON messages(contact_id, sent_at);
CREATE INDEX IF NOT EXISTS messages_expiring ON messages(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS messages_scheduled ON messages(status) WHERE status = -2;
CREATE TABLE IF NOT EXISTS changes_out (
    seq      INTEGER PRIMARY KEY AUTOINCREMENT,
    id       TEXT NOT NULL,
    peer     TEXT NOT NULL,
    target   TEXT NOT NULL,
    group_id TEXT,
    change   BLOB NOT NULL,
    at       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS changes_by_peer ON changes_out(peer, seq);
CREATE TABLE IF NOT EXISTS reminders (
    message_id   TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    at           INTEGER NOT NULL,
    if_no_answer INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS chat_prefs (
    contact_id TEXT PRIMARY KEY REFERENCES contacts(id) ON DELETE CASCADE,
    sealed     BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS calls (
    id          TEXT PRIMARY KEY,
    contact_id  TEXT NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    outgoing    INTEGER NOT NULL,
    video       INTEGER NOT NULL,
    at          INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    missed      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS drafts (
    contact_id TEXT PRIMARY KEY REFERENCES contacts(id) ON DELETE CASCADE,
    text       BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS groups (
    id       TEXT PRIMARY KEY REFERENCES contacts(id) ON DELETE CASCADE,
    admin    TEXT NOT NULL,
    version  INTEGER NOT NULL,
    closed   INTEGER NOT NULL DEFAULT 0,
    active   INTEGER NOT NULL DEFAULT 1,
    left     INTEGER NOT NULL DEFAULT 0,
    sig      BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS group_members (
    group_id  TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    member_id TEXT NOT NULL,
    PRIMARY KEY (group_id, member_id)
);
CREATE TABLE IF NOT EXISTS group_acks (
    message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    member_id  TEXT NOT NULL,
    status     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (message_id, member_id)
);
CREATE TABLE IF NOT EXISTS seen_ids (
    id TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tombstones (
    id TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS secrets (
    name  TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS olm_sessions (
    person TEXT PRIMARY KEY,
    pickle BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS contact_keys (
    contact_id TEXT PRIMARY KEY REFERENCES contacts(id) ON DELETE CASCADE,
    sealed     BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS invites (
    id         TEXT PRIMARY KEY,
    sealed     BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
";

/// What happened when opening the database, so the UI can tell the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbRecovery {
    Healthy,
    /// The file was unreadable. It was moved aside and a fresh one created.
    Recovered {
        backup: PathBuf,
    },
    /// Even a fresh file failed. Running in memory; nothing will be saved.
    Volatile,
}

pub struct Database {
    conn: Mutex<Connection>,
    vault: Arc<Vault>,
}

impl Database {
    pub fn open(path: &Path, vault: Arc<Vault>) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn, vault)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?, Arc::new(Vault::in_memory()))
    }

    fn init(conn: Connection, vault: Arc<Vault>) -> rusqlite::Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(&format!("PRAGMA user_version = {CURRENT_VERSION}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
            vault,
        })
    }

    /// Opens `path`, recovering from corruption instead of failing.
    pub fn open_with_recovery(path: &Path, vault: Arc<Vault>) -> (Self, DbRecovery) {
        if let Ok(db) = Self::open_healthy(path, vault.clone()) {
            return (db, DbRecovery::Healthy);
        }
        if let Some(backup) = quarantine(path)
            && let Ok(db) = Self::open_healthy(path, vault.clone())
        {
            return (db, DbRecovery::Recovered { backup });
        }
        let db = Connection::open_in_memory()
            .and_then(|conn| Self::init(conn, vault))
            .expect("in-memory SQLite must open");
        (db, DbRecovery::Volatile)
    }

    fn open_healthy(path: &Path, vault: Arc<Vault>) -> rusqlite::Result<Self> {
        let db = Self::open(path, vault)?;
        let ok: String = db
            .lock()
            .query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if ok == "ok" {
            Ok(db)
        } else {
            Err(rusqlite::Error::InvalidQuery)
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Deletes every row and compacts the file so nothing lingers on disk.
    pub fn wipe(&self) -> rusqlite::Result<()> {
        self.lock().execute_batch(
            "DELETE FROM changes_out; DELETE FROM reminders; DELETE FROM chat_prefs; DELETE FROM calls; DELETE FROM tombstones; DELETE FROM secrets; DELETE FROM olm_sessions; DELETE FROM contact_keys; DELETE FROM invites; DELETE FROM seen_ids; DELETE FROM drafts; DELETE FROM group_acks; DELETE FROM messages; DELETE FROM group_members; DELETE FROM groups; DELETE FROM contacts;
             DELETE FROM settings; PRAGMA wal_checkpoint(TRUNCATE); VACUUM;",
        )
    }

    // ---- settings ----

    pub fn settings(&self) -> Settings {
        self.lock()
            .query_row("SELECT data FROM settings WHERE id = 1", [], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .optional()
            .ok()
            .flatten()
            .and_then(|sealed| self.vault.open(&sealed))
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, s: &Settings) -> rusqlite::Result<()> {
        let json = serde_json::to_vec(s).expect("settings serialize");
        self.lock().execute(
            "INSERT INTO settings (id, data) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            params![self.vault.seal(&json)],
        )?;
        Ok(())
    }

    // ---- contacts ----

    pub fn contact(&self, id: &str) -> Option<Contact> {
        self.lock()
            .query_row(
                "SELECT id, their_name, nickname, state, last_seen, created_at, avatar, kind, pinned, muted
                 FROM contacts WHERE id = ?1",
                params![id],
                |r| contact_from_row(r, &self.vault),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn contacts_in(&self, states: &[ContactState]) -> Vec<Contact> {
        self.contacts_where(states, "1")
    }

    /// Contacts in `states` that also match `condition` (a trusted SQL
    /// snippet), pinned first, then the most recent.
    fn contacts_where(&self, states: &[ContactState], condition: &str) -> Vec<Contact> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT id, their_name, nickname, state, last_seen, created_at, avatar, kind, pinned, muted
                 FROM contacts WHERE {condition} ORDER BY pinned DESC, last_activity DESC"
            ))
            .expect("valid SQL");
        stmt.query_map([], |r| contact_from_row(r, &self.vault))
            .map(|rows| {
                rows.filter_map(Result::ok)
                    .filter(|c| states.contains(&c.state))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Inserts a contact or updates its state. Returns the stored contact.
    pub fn upsert_contact(
        &self,
        id: &str,
        state: ContactState,
        their_name: Option<&str>,
        nickname: Option<&str>,
    ) -> rusqlite::Result<Contact> {
        let now = now_ms();
        let their_name = their_name.map(|n| self.vault.seal_str(n));
        let nickname = nickname.map(|n| self.vault.seal_str(n));
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO contacts (id, their_name, nickname, state, created_at, last_activity)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    state = excluded.state,
                    their_name = COALESCE(?2, contacts.their_name),
                    nickname = COALESCE(?3, contacts.nickname),
                    last_activity = excluded.last_activity,
                    hidden = 0",
                params![id, their_name, nickname, state.as_str(), now],
            )?;
        }
        Ok(self.contact(id).expect("just upserted"))
    }

    pub fn set_their_name(&self, id: &str, name: &str) -> bool {
        if self.contact(id).is_none_or(|c| c.their_name == name) {
            return false;
        }
        self.lock()
            .execute(
                "UPDATE contacts SET their_name = ?2 WHERE id = ?1",
                params![id, self.vault.seal_str(name)],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn set_nickname(&self, id: &str, nickname: Option<&str>) {
        let _ = self.lock().execute(
            "UPDATE contacts SET nickname = ?2 WHERE id = ?1",
            params![id, nickname.map(|n| self.vault.seal_str(n))],
        );
    }

    pub fn set_last_seen(&self, id: &str, at: Option<Millis>) {
        let _ = self.lock().execute(
            "UPDATE contacts SET last_seen = ?2 WHERE id = ?1",
            params![id, at],
        );
    }

    /// Removes the contact and (via cascade) its messages and draft.
    pub fn delete_contact(&self, id: &str) {
        let _ = self
            .lock()
            .execute("DELETE FROM contacts WHERE id = ?1", params![id]);
    }

    /// Hides the chat from the chat list (the contact stays).
    pub fn set_hidden(&self, id: &str, hidden: bool) {
        let _ = self.lock().execute(
            "UPDATE contacts SET hidden = ?2 WHERE id = ?1",
            params![id, hidden],
        );
    }

    pub fn set_pinned(&self, id: &str, pinned: bool) {
        let _ = self.lock().execute(
            "UPDATE contacts SET pinned = ?2 WHERE id = ?1",
            params![id, pinned],
        );
    }

    pub fn set_muted(&self, id: &str, muted: bool) {
        let _ = self.lock().execute(
            "UPDATE contacts SET muted = ?2 WHERE id = ?1",
            params![id, muted],
        );
    }

    pub fn clear_chat(&self, id: &str) {
        let conn = self.lock();
        let _ = conn.execute(
            "INSERT OR IGNORE INTO tombstones (id, at)
             SELECT id, ?2 FROM messages WHERE contact_id = ?1",
            params![id, now_ms()],
        );
        // Remembered long enough for anything still on its way, and no longer.
        let _ = conn.execute(
            "DELETE FROM tombstones WHERE at < ?1",
            params![now_ms() - 90 * 24 * 3600 * 1000],
        );
        let _ = conn.execute("DELETE FROM messages WHERE contact_id = ?1", params![id]);
    }

    pub fn chat_summaries(&self) -> Vec<ChatSummary> {
        let contacts = self.contacts_where(
            &[ContactState::Accepted, ContactState::PendingOut],
            "hidden = 0",
        );
        let conn = self.lock();
        contacts
            .into_iter()
            .map(|contact| {
                let last_message = conn
                    .query_row(
                        &format!(
                            "SELECT {MSG_COLS} FROM messages WHERE contact_id = ?1 AND status != -2
                             ORDER BY sent_at DESC, id DESC LIMIT 1"
                        ),
                        params![contact.id],
                        |r| message_from_row(r, &self.vault),
                    )
                    .optional()
                    .ok()
                    .flatten();
                let unread: u32 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM messages
                         WHERE contact_id = ?1 AND outgoing = 0 AND read = 0",
                        params![contact.id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                let prefs = chat_prefs_in(&conn, &self.vault, &contact.id);
                ChatSummary {
                    contact,
                    last_message,
                    unread,
                    prefs,
                }
            })
            .collect()
    }

    // ---- messages ----

    /// Inserts a message. Returns false if a message with this id already exists.
    /// Stores a message once, ever: also if it was deleted since, a late copy of it stays away.
    pub fn insert_message(&self, m: &Message) -> rusqlite::Result<bool> {
        let conn = self.lock();
        // Deleted here, and sent again by someone who has not heard it arrived: stays deleted.
        let deleted: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tombstones WHERE id = ?1)",
            params![m.id],
            |r| r.get(0),
        )?;
        if deleted {
            return Ok(false);
        }
        let first = conn.execute(
            "INSERT OR IGNORE INTO seen_ids (id, at) VALUES (?1, ?2)",
            params![m.id, now_ms()],
        )?;
        if first == 0 {
            return Ok(false);
        }
        let n = conn.execute(
            "INSERT OR IGNORE INTO messages
             (id, contact_id, outgoing, body, sent_at, received_at, status, read, attachment, sender, extra, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                m.id,
                m.contact_id,
                m.outgoing,
                self.vault.seal_str(&m.body),
                m.sent_at,
                m.received_at,
                m.status as i64,
                m.read,
                m.attachment.as_ref().map(|a| self
                    .vault
                    .seal(&serde_json::to_vec(a).expect("attachment serializes"))),
                m.sender,
                self.seal_extra(&m.extra),
                // One waiting for its moment starts counting when it goes.
                (m.status != MessageStatus::Scheduled)
                    .then_some(m.extra.disappear_ms)
                    .flatten()
                    .map(|ms| m.received_at.saturating_add(ms)),
            ],
        )?;
        if n > 0 {
            conn.execute(
                "UPDATE contacts SET last_activity = ?2, hidden = 0 WHERE id = ?1",
                params![m.contact_id, now_ms()],
            )?;
        }
        Ok(n > 0)
    }

    pub fn message(&self, id: &str) -> Option<Message> {
        self.lock()
            .query_row(
                &format!("SELECT {MSG_COLS} FROM messages WHERE id = ?1"),
                params![id],
                |r| message_from_row(r, &self.vault),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// The newest `limit` messages before `before` (exclusive), oldest first.
    pub fn messages(&self, contact_id: &str, before: Option<Millis>, limit: u32) -> Vec<Message> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {MSG_COLS} FROM messages
                 WHERE contact_id = ?1 AND sent_at < ?2 AND status != -2
                 ORDER BY sent_at DESC, id DESC LIMIT ?3"
            ))
            .expect("valid SQL");
        let mut out: Vec<Message> = stmt
            .query_map(
                params![contact_id, before.unwrap_or(i64::MAX), limit],
                |r| message_from_row(r, &self.vault),
            )
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default();
        out.reverse();
        out
    }

    /// Outgoing messages the contact has not confirmed yet, oldest first.
    pub fn undelivered(&self, contact_id: &str) -> Vec<Message> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {MSG_COLS} FROM messages
                 WHERE contact_id = ?1 AND outgoing = 1 AND status IN (0, 1)
                 ORDER BY sent_at ASC, id ASC"
            ))
            .expect("valid SQL");
        stmt.query_map(params![contact_id], |r| message_from_row(r, &self.vault))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Moves an outgoing message's status forward (never backward).
    /// Returns true if it changed.
    /// Everyone something still has to reach: a message of ours, a request, a group message, a
    /// goodbye to a group's admin. Who is blocked or deleted is owed nothing.
    pub fn peers_owed(&self) -> Vec<String> {
        let conn = self.lock();
        let mut owed: Vec<String> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT m.contact_id, m.attachment FROM messages m
             JOIN contacts c ON c.id = m.contact_id
             WHERE m.outgoing = 1 AND m.status IN (0, 1) AND c.kind = 'person' AND c.state = 'accepted'",
        ) && let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<Vec<u8>>>(1)?))
        }) {
            for (contact, attachment) in rows.filter_map(Result::ok) {
                let declined = attachment
                    .and_then(|sealed| self.vault.open(&sealed))
                    .and_then(|json| serde_json::from_slice::<Attachment>(&json).ok())
                    .is_some_and(|a| a.state == TransferState::Declined);
                if !declined {
                    owed.push(contact);
                }
            }
        }
        if let Ok(mut stmt) = conn.prepare(
            "SELECT a.member_id FROM group_acks a JOIN messages m ON m.id = a.message_id
             WHERE a.status < 2 AND m.status >= 0
             UNION SELECT g.admin FROM groups g WHERE g.left = 1
             UNION SELECT o.peer FROM changes_out o JOIN contacts c ON c.id = o.peer
                   WHERE c.state IN ('accepted', 'group_peer')",
        ) && let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0))
        {
            owed.extend(rows.filter_map(Result::ok));
        }
        owed.sort();
        owed.dedup();
        owed
    }

    // ---- what happens to a message later ----

    fn seal_extra(&self, extra: &MessageExtra) -> Option<Vec<u8>> {
        (*extra != MessageExtra::default()).then(|| {
            self.vault
                .seal(&serde_json::to_vec(extra).expect("extra serializes"))
        })
    }

    /// Writes back what changed in a message: its text, file and extras.
    pub fn update_message(&self, m: &Message) {
        let _ = self.lock().execute(
            "UPDATE messages SET body = ?2, attachment = ?3, extra = ?4, saved = ?5 WHERE id = ?1",
            params![
                m.id,
                self.vault.seal_str(&m.body),
                m.attachment.as_ref().map(|a| self
                    .vault
                    .seal(&serde_json::to_vec(a).expect("attachment serializes"))),
                self.seal_extra(&m.extra),
                m.extra.saved,
            ],
        );
    }

    pub fn is_deleted(&self, id: &str) -> bool {
        self.lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM tombstones WHERE id = ?1)",
                params![id],
                |r| r.get(0),
            )
            .unwrap_or(false)
    }

    /// One change for each of `peers`, to be sent until confirmed.
    pub fn queue_change(
        &self,
        id: &str,
        peers: &[String],
        target: &str,
        group: Option<&str>,
        change: &[u8],
    ) {
        let conn = self.lock();
        let sealed = self.vault.seal(change);
        for peer in peers {
            let _ = conn.execute(
                "INSERT INTO changes_out (id, peer, target, group_id, change, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, peer, target, group, sealed, now_ms()],
            );
        }
    }

    /// Changes still to reach this person, oldest first: id, target, group, change.
    pub fn changes_for(&self, peer: &str) -> Vec<(String, String, Option<String>, Vec<u8>)> {
        let conn = self.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, target, group_id, change FROM changes_out WHERE peer = ?1 ORDER BY seq",
        ) else {
            return Vec::new();
        };
        stmt.query_map(params![peer], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map(|rows| {
            rows.filter_map(Result::ok)
                .filter_map(|(id, target, group, sealed)| {
                    Some((id, target, group, self.vault.open(&sealed)?))
                })
                .collect()
        })
        .unwrap_or_default()
    }

    pub fn change_arrived(&self, id: &str, peer: &str) {
        let _ = self.lock().execute(
            "DELETE FROM changes_out WHERE id = ?1 AND peer = ?2",
            params![id, peer],
        );
    }

    /// Who was away this long does not need to hear about old reactions.
    pub fn forget_old_changes(&self, before: Millis) {
        let _ = self
            .lock()
            .execute("DELETE FROM changes_out WHERE at < ?1", params![before]);
    }

    pub fn chat_prefs(&self, contact_id: &str) -> ChatPrefs {
        chat_prefs_in(&self.lock(), &self.vault, contact_id)
    }

    pub fn set_chat_prefs(&self, contact_id: &str, prefs: &ChatPrefs) {
        let _ = self.lock().execute(
            "INSERT INTO chat_prefs (contact_id, sealed) VALUES (?1, ?2)
             ON CONFLICT(contact_id) DO UPDATE SET sealed = excluded.sealed",
            params![
                contact_id,
                self.vault
                    .seal(&serde_json::to_vec(prefs).expect("prefs serialize"))
            ],
        );
    }

    /// Messages waiting for their moment, soonest first.
    pub fn scheduled(&self) -> Vec<Message> {
        self.messages_where("status = -2 ORDER BY sent_at ASC, id ASC", params![])
    }

    pub fn set_scheduled_time(&self, id: &str, at: Millis) {
        let _ = self.lock().execute(
            "UPDATE messages SET sent_at = ?2 WHERE id = ?1 AND status = -2",
            params![id, at],
        );
    }

    /// The ones whose moment has come go in the queue, from now on counted as sent now.
    pub fn take_due(&self, now: Millis) -> Vec<Message> {
        let due = self.messages_where(
            "status = -2 AND sent_at <= ?1 ORDER BY sent_at ASC, id ASC",
            params![now],
        );
        let conn = self.lock();
        for m in &due {
            let _ = conn.execute(
                "UPDATE messages SET status = 0, sent_at = ?2, received_at = ?2, expires_at = ?3
                 WHERE id = ?1",
                params![
                    m.id,
                    now,
                    m.extra.disappear_ms.map(|ms| now.saturating_add(ms))
                ],
            );
        }
        drop(conn);
        due.into_iter()
            .filter_map(|m| self.message(&m.id))
            .collect()
    }

    /// What is past its time.
    pub fn expired(&self, now: Millis) -> Vec<Message> {
        self.messages_where("expires_at <= ?1", params![now])
    }

    pub fn saved_messages(&self) -> Vec<Message> {
        self.messages_where("saved = 1 ORDER BY sent_at DESC, id DESC", params![])
    }

    /// Everything that can be looked through, newest first. Text is sealed on disk, so a search
    /// has to open it.
    pub fn searchable(&self, chat: Option<&str>) -> Vec<Message> {
        match chat {
            Some(chat) => self.messages_where(
                "contact_id = ?1 AND status != -2 ORDER BY sent_at DESC, id DESC",
                params![chat],
            ),
            None => self.messages_where("status != -2 ORDER BY sent_at DESC, id DESC", params![]),
        }
    }

    fn messages_where(&self, condition: &str, args: &[&dyn rusqlite::ToSql]) -> Vec<Message> {
        let conn = self.lock();
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT {MSG_COLS} FROM messages WHERE {condition}"
        )) else {
            return Vec::new();
        };
        stmt.query_map(args, |r| message_from_row(r, &self.vault))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn add_reminder(&self, message_id: &str, at: Millis, if_no_answer: bool) {
        let _ = self.lock().execute(
            "INSERT INTO reminders (message_id, at, if_no_answer) VALUES (?1, ?2, ?3)
             ON CONFLICT(message_id) DO UPDATE SET at = excluded.at, if_no_answer = excluded.if_no_answer",
            params![message_id, at, if_no_answer],
        );
    }

    pub fn drop_reminder(&self, message_id: &str) {
        let _ = self.lock().execute(
            "DELETE FROM reminders WHERE message_id = ?1",
            params![message_id],
        );
    }

    /// Message, moment: soonest first.
    pub fn reminders(&self) -> Vec<(String, Millis)> {
        let conn = self.lock();
        let Ok(mut stmt) = conn.prepare("SELECT message_id, at FROM reminders ORDER BY at") else {
            return Vec::new();
        };
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// The reminders whose moment has come leave the list. Those that only count without an
    /// answer are left out if one came.
    pub fn take_due_reminders(&self, now: Millis) -> Vec<String> {
        let conn = self.lock();
        let due: Vec<(String, bool)> = conn
            .prepare("SELECT message_id, if_no_answer FROM reminders WHERE at <= ?1")
            .and_then(|mut stmt| {
                stmt.query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        for (id, if_no_answer) in due {
            let answered = if_no_answer
                && conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM messages later, messages m
                         WHERE m.id = ?1 AND later.contact_id = m.contact_id
                         AND later.outgoing = 0 AND later.sent_at > m.sent_at)",
                        params![id],
                        |r| r.get(0),
                    )
                    .unwrap_or(false);
            let _ = conn.execute("DELETE FROM reminders WHERE message_id = ?1", params![id]);
            if !answered {
                out.push(id);
            }
        }
        out
    }

    /// When something is due next: a message to send, a reminder, something to remove.
    pub fn next_timer(&self) -> Option<Millis> {
        self.lock()
            .query_row(
                "SELECT MIN(at) FROM (
                     SELECT MIN(sent_at) AS at FROM messages WHERE status = -2
                     UNION ALL SELECT MIN(at) FROM reminders
                     UNION ALL SELECT MIN(expires_at) FROM messages WHERE expires_at IS NOT NULL
                 )",
                [],
                |r| r.get(0),
            )
            .ok()
            .flatten()
    }

    pub fn add_call_record(&self, c: &CallRecord) {
        let _ = self.lock().execute(
            "INSERT OR IGNORE INTO calls (id, contact_id, outgoing, video, at, duration_ms, missed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                c.id,
                c.contact_id,
                c.outgoing,
                c.video,
                c.at,
                c.duration_ms as i64,
                c.missed
            ],
        );
    }

    pub fn call_records(&self) -> Vec<CallRecord> {
        let conn = self.lock();
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, contact_id, outgoing, video, at, duration_ms, missed FROM calls
             ORDER BY at DESC LIMIT 500",
        ) else {
            return Vec::new();
        };
        stmt.query_map([], |r| {
            Ok(CallRecord {
                id: r.get(0)?,
                contact_id: r.get(1)?,
                outgoing: r.get(2)?,
                video: r.get(3)?,
                at: r.get(4)?,
                duration_ms: r.get::<_, i64>(5)? as u64,
                missed: r.get(6)?,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    pub fn delete_call_record(&self, id: &str) {
        let _ = self
            .lock()
            .execute("DELETE FROM calls WHERE id = ?1", params![id]);
    }

    /// Forgets which messages were seen after so long that no copy can still be under way.
    pub fn forget_old_seen(&self, before: Millis) {
        let _ = self
            .lock()
            .execute("DELETE FROM seen_ids WHERE at < ?1", params![before]);
    }

    /// A secret kept in the database, sealed.
    pub fn secret(&self, name: &str) -> Option<Vec<u8>> {
        let sealed: Vec<u8> = self
            .lock()
            .query_row(
                "SELECT value FROM secrets WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        self.vault.open(&sealed)
    }

    pub fn set_secret(&self, name: &str, value: &[u8]) -> rusqlite::Result<()> {
        self.lock().execute(
            "INSERT INTO secrets (name, value) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            params![name, self.vault.seal(value)],
        )?;
        Ok(())
    }

    /// The key that seals an Olm pickle: a purpose of its own.
    pub fn pickle_key(&self) -> [u8; 32] {
        self.vault.derive("olm pickles")
    }

    pub fn olm_session(&self, person: &str) -> Option<Vec<u8>> {
        self.lock()
            .query_row(
                "SELECT pickle FROM olm_sessions WHERE person = ?1",
                params![person],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_olm_session(&self, person: &str, pickle: &[u8]) {
        let _ = self.lock().execute(
            "INSERT INTO olm_sessions (person, pickle) VALUES (?1, ?2)
             ON CONFLICT(person) DO UPDATE SET pickle = excluded.pickle",
            params![person, pickle],
        );
    }

    pub fn drop_olm_session(&self, person: &str) {
        let _ = self.lock().execute(
            "DELETE FROM olm_sessions WHERE person = ?1",
            params![person],
        );
    }

    /// What we know to reach this contact by.
    pub fn contact_keys(&self, contact_id: &str) -> Option<ContactKeys> {
        let sealed: Vec<u8> = self
            .lock()
            .query_row(
                "SELECT sealed FROM contact_keys WHERE contact_id = ?1",
                params![contact_id],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()?;
        postcard::from_bytes(&self.vault.open(&sealed)?).ok()
    }

    pub fn set_contact_keys(&self, contact_id: &str, keys: &ContactKeys) {
        let bytes = postcard::to_stdvec(keys).expect("keys serialize");
        let _ = self.lock().execute(
            "INSERT INTO contact_keys (contact_id, sealed) VALUES (?1, ?2)
             ON CONFLICT(contact_id) DO UPDATE SET sealed = excluded.sealed",
            params![contact_id, self.vault.seal(&bytes)],
        );
    }

    /// Our invitations, newest first.
    pub fn invites(&self) -> Vec<MyInvite> {
        let conn = self.lock();
        let Ok(mut stmt) = conn.prepare("SELECT sealed FROM invites ORDER BY created_at DESC")
        else {
            return Vec::new();
        };
        stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))
            .map(|rows| {
                rows.filter_map(Result::ok)
                    .filter_map(|sealed| self.vault.open(&sealed))
                    .filter_map(|bytes| postcard::from_bytes(&bytes).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn save_invite(&self, invite: &MyInvite) {
        let bytes = postcard::to_stdvec(invite).expect("invite serializes");
        let _ = self.lock().execute(
            "INSERT INTO invites (id, sealed, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET sealed = excluded.sealed",
            params![invite.id, self.vault.seal(&bytes), invite.created_at],
        );
    }

    pub fn delete_invites(&self) {
        let _ = self.lock().execute("DELETE FROM invites", []);
    }

    /// Messages sent before `before` stop being tried. Returns which, for the screens.
    pub fn give_up_before(&self, before: Millis) -> Vec<(String, String)> {
        let conn = self.lock();
        let found: Vec<(String, String)> = conn
            .prepare(
                "SELECT contact_id, id FROM messages
                 WHERE outgoing = 1 AND status IN (0, 1) AND sent_at < ?1",
            )
            .and_then(|mut stmt| {
                stmt.query_map(params![before], |r| Ok((r.get(0)?, r.get(1)?)))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default();
        let _ = conn.execute(
            "UPDATE messages SET status = -1
             WHERE outgoing = 1 AND status IN (0, 1) AND sent_at < ?1",
            params![before],
        );
        found
    }

    /// Back in the queue, with a new chance of the time set.
    pub fn retry_message(&self, id: &str) -> bool {
        self.lock()
            .execute(
                "UPDATE messages SET status = 0, sent_at = ?2
                 WHERE id = ?1 AND outgoing = 1 AND status = -1",
                params![id, now_ms()],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn advance_status(&self, contact_id: &str, id: &str, status: MessageStatus) -> bool {
        self.lock()
            .execute(
                "UPDATE messages SET status = ?3
                 WHERE id = ?1 AND contact_id = ?2 AND outgoing = 1 AND status < ?3",
                params![id, contact_id, status as i64],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// Marks all incoming messages from a contact as read. Returns their ids.
    pub fn mark_read(&self, contact_id: &str) -> Vec<String> {
        let conn = self.lock();
        let ids: Vec<String> = conn
            .prepare("SELECT id FROM messages WHERE contact_id = ?1 AND outgoing = 0 AND read = 0")
            .and_then(|mut s| {
                s.query_map(params![contact_id], |r| r.get(0))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default();
        let _ = conn.execute(
            "UPDATE messages SET read = 1 WHERE contact_id = ?1 AND outgoing = 0 AND read = 0",
            params![contact_id],
        );
        ids
    }

    /// Read incoming messages whose read receipt has not gone out yet.
    pub fn pending_read_receipts(&self, contact_id: &str) -> Vec<String> {
        self.lock()
            .prepare(
                "SELECT id FROM messages
                 WHERE contact_id = ?1 AND outgoing = 0 AND read = 1 AND read_receipt_sent = 0",
            )
            .and_then(|mut s| {
                s.query_map(params![contact_id], |r| r.get(0))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    pub fn mark_read_receipts_sent(&self, ids: &[String]) {
        let conn = self.lock();
        for id in ids {
            let _ = conn.execute(
                "UPDATE messages SET read_receipt_sent = 1 WHERE id = ?1",
                params![id],
            );
        }
    }

    pub fn delete_message(&self, id: &str) {
        let conn = self.lock();
        let _ = conn.execute(
            "INSERT OR IGNORE INTO tombstones (id, at) VALUES (?1, ?2)",
            params![id, now_ms()],
        );
        let _ = conn.execute("DELETE FROM messages WHERE id = ?1", params![id]);
    }

    /// Replaces a message's attachment metadata.
    pub fn set_attachment(&self, id: &str, a: &Attachment) {
        let json = serde_json::to_vec(a).expect("attachment serializes");
        let _ = self.lock().execute(
            "UPDATE messages SET attachment = ?2 WHERE id = ?1",
            params![id, self.vault.seal(&json)],
        );
    }

    /// Updates just the transfer state (and optionally the local path).
    /// Returns the updated message.
    pub fn set_transfer(
        &self,
        id: &str,
        state: TransferState,
        path: Option<&str>,
    ) -> Option<Message> {
        let mut m = self.message(id)?;
        let a = m.attachment.as_mut()?;
        a.state = state;
        if let Some(p) = path {
            a.path = Some(p.to_string());
        }
        self.set_attachment(id, a);
        Some(m)
    }

    /// Local files of attachments in a chat (to delete with it).
    pub fn attachment_paths(&self, contact_id: &str) -> Vec<String> {
        self.messages(contact_id, None, u32::MAX)
            .into_iter()
            .filter_map(|m| m.attachment.and_then(|a| a.path))
            .collect()
    }

    pub fn set_avatar(&self, contact_id: &str, path: Option<&str>) {
        let _ = self.lock().execute(
            "UPDATE contacts SET avatar = ?2 WHERE id = ?1",
            params![contact_id, path],
        );
    }

    // ---- groups ----

    /// The stored state of a group, its signature, and whether we are still in it.
    pub fn group(&self, id: &str) -> Option<(GroupState, Vec<u8>, bool)> {
        let conn = self.lock();
        let (admin, version, closed, active, sig): (String, i64, bool, bool, Vec<u8>) = conn
            .query_row(
                "SELECT admin, version, closed, active, sig FROM groups WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .ok()
            .flatten()?;
        let name: Option<Vec<u8>> = conn
            .query_row(
                "SELECT their_name FROM contacts WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok()?;
        let name = name
            .map(|sealed| self.vault.open_str(&sealed))
            .unwrap_or_default();
        let mut members: Vec<String> = conn
            .prepare("SELECT member_id FROM group_members WHERE group_id = ?1")
            .and_then(|mut s| {
                s.query_map(params![id], |r| r.get(0))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default();
        members.sort();
        Some((
            GroupState {
                id: id.to_string(),
                name,
                admin,
                version: version as u64,
                members,
                closed,
            },
            sig,
            active,
        ))
    }

    /// Creates or replaces a group from `state`. Members we do not know yet
    /// become group peers (named from `names`); `me` is never added as one.
    pub fn save_group(
        &self,
        state: &GroupState,
        sig: &[u8],
        active: bool,
        names: &[(String, String)],
        me: &str,
    ) -> rusqlite::Result<()> {
        let now = now_ms();
        let conn = self.lock();
        conn.execute(
            "INSERT INTO contacts (id, their_name, state, created_at, last_activity, kind)
             VALUES (?1, ?2, 'accepted', ?3, ?3, 'group')
             ON CONFLICT(id) DO UPDATE SET their_name = excluded.their_name",
            params![state.id, self.vault.seal_str(&state.name), now],
        )?;
        conn.execute(
            "INSERT INTO groups (id, admin, version, closed, active, sig)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET admin = excluded.admin, version = excluded.version,
                closed = excluded.closed, active = excluded.active, sig = excluded.sig",
            params![
                state.id,
                state.admin,
                state.version as i64,
                state.closed,
                active,
                sig
            ],
        )?;
        conn.execute(
            "DELETE FROM group_members WHERE group_id = ?1",
            params![state.id],
        )?;
        for m in &state.members {
            conn.execute(
                "INSERT OR IGNORE INTO group_members (group_id, member_id) VALUES (?1, ?2)",
                params![state.id, m],
            )?;
            if m != me {
                let name = names
                    .iter()
                    .find(|(k, _)| k == m)
                    .map(|(_, n)| n.as_str())
                    .unwrap_or("");
                conn.execute(
                    "INSERT OR IGNORE INTO contacts
                        (id, their_name, state, created_at, last_activity, kind)
                     VALUES (?1, ?2, 'group_peer', ?3, ?3, 'person')",
                    params![m, self.vault.seal_str(name), now],
                )?;
                if !name.is_empty() {
                    let known: Option<Vec<u8>> = conn
                        .query_row(
                            "SELECT their_name FROM contacts WHERE id = ?1",
                            params![m],
                            |r| r.get(0),
                        )
                        .unwrap_or(None);
                    if known.is_none_or(|sealed| self.vault.open_str(&sealed).is_empty()) {
                        conn.execute(
                            "UPDATE contacts SET their_name = ?2 WHERE id = ?1",
                            params![m, self.vault.seal_str(name)],
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// We left; the admin has not necessarily heard yet.
    pub fn set_group_left(&self, id: &str, left: bool) {
        let _ = self.lock().execute(
            "UPDATE groups SET left = ?2 WHERE id = ?1",
            params![id, left],
        );
    }

    /// Groups we left whose admin is `admin` and may not know it yet.
    pub fn pending_leaves_to(&self, admin: &str) -> Vec<String> {
        self.lock()
            .prepare("SELECT id FROM groups WHERE left = 1 AND admin = ?1")
            .and_then(|mut s| {
                s.query_map(params![admin], |r| r.get(0))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    pub fn set_group_active(&self, id: &str, active: bool) {
        let _ = self.lock().execute(
            "UPDATE groups SET active = ?2 WHERE id = ?1",
            params![id, active],
        );
    }

    /// Ids of the groups we are still in.
    pub fn active_groups(&self) -> Vec<String> {
        self.lock()
            .prepare("SELECT id FROM groups WHERE active = 1")
            .and_then(|mut s| {
                s.query_map([], |r| r.get(0))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    /// Groups we are in that this person is a member of.
    pub fn groups_with_member(&self, member: &str) -> Vec<String> {
        self.lock()
            .prepare(
                "SELECT g.id FROM groups g JOIN group_members m ON m.group_id = g.id
                 WHERE g.active = 1 AND m.member_id = ?1",
            )
            .and_then(|mut s| {
                s.query_map(params![member], |r| r.get(0))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    /// Keys of everyone we share an active group with.
    pub fn group_peer_ids(&self) -> Vec<String> {
        self.lock()
            .prepare(
                "SELECT DISTINCT m.member_id FROM group_members m JOIN groups g ON g.id = m.group_id
                 WHERE g.active = 1",
            )
            .and_then(|mut s| {
                s.query_map([], |r| r.get(0))
                    .map(|rows| rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    pub fn is_group_member(&self, group_id: &str, member: &str) -> bool {
        self.lock()
            .query_row(
                "SELECT 1 FROM group_members m JOIN groups g ON g.id = m.group_id
                 WHERE m.group_id = ?1 AND m.member_id = ?2 AND g.active = 1",
                params![group_id, member],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// Deletes group peers nobody shares a group with any more.
    pub fn prune_group_peers(&self) {
        let _ = self.lock().execute(
            "DELETE FROM contacts WHERE state = 'group_peer'
             AND id NOT IN (SELECT member_id FROM group_members)",
            [],
        );
    }

    pub fn delete_group(&self, id: &str) {
        self.delete_contact(id);
        self.prune_group_peers();
    }

    /// Starts tracking delivery of a group message to each member.
    pub fn add_group_acks(&self, message_id: &str, members: &[String]) {
        let conn = self.lock();
        for m in members {
            let _ = conn.execute(
                "INSERT OR IGNORE INTO group_acks (message_id, member_id) VALUES (?1, ?2)",
                params![message_id, m],
            );
        }
    }

    /// Records that a member stored a group message. True if it changed.
    pub fn set_group_ack(&self, message_id: &str, member: &str, status: i64) -> bool {
        self.lock()
            .execute(
                "UPDATE group_acks SET status = ?3
                 WHERE message_id = ?1 AND member_id = ?2 AND status < ?3",
                params![message_id, member, status],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// (members that have it, members tracked) for a group message.
    pub fn group_ack_counts(&self, message_id: &str) -> (u32, u32) {
        self.lock()
            .query_row(
                "SELECT COALESCE(SUM(status >= 2), 0), COUNT(*) FROM group_acks WHERE message_id = ?1",
                params![message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0, 0))
    }

    /// Our group messages this member has not confirmed yet, oldest first.
    pub fn group_unacked_for(&self, member: &str) -> Vec<Message> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {MSG_COLS} FROM messages
                 WHERE outgoing = 1 AND id IN
                    (SELECT message_id FROM group_acks WHERE member_id = ?1 AND status < 2) AND status >= 0
                 ORDER BY sent_at ASC, id ASC"
            ))
            .expect("valid SQL");
        stmt.query_map(params![member], |r| message_from_row(r, &self.vault))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Stops tracking a member's acks (they left or were removed).
    pub fn remove_member_acks(&self, group_id: &str, member: &str) {
        let _ = self.lock().execute(
            "DELETE FROM group_acks WHERE member_id = ?2 AND message_id IN
                (SELECT id FROM messages WHERE contact_id = ?1)",
            params![group_id, member],
        );
    }

    /// Outgoing group messages not yet confirmed by everyone, oldest first.
    pub fn group_pending_messages(&self, group_id: &str) -> Vec<Message> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {MSG_COLS} FROM messages
                 WHERE contact_id = ?1 AND outgoing = 1 AND status IN (0, 1)
                 ORDER BY sent_at ASC, id ASC"
            ))
            .expect("valid SQL");
        stmt.query_map(params![group_id], |r| message_from_row(r, &self.vault))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    // ---- drafts ----

    pub fn draft(&self, contact_id: &str) -> String {
        self.lock()
            .query_row(
                "SELECT text FROM drafts WHERE contact_id = ?1",
                params![contact_id],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .map(|sealed| self.vault.open_str(&sealed))
            .unwrap_or_default()
    }

    pub fn set_draft(&self, contact_id: &str, text: &str) {
        let conn = self.lock();
        let _ = if text.is_empty() {
            conn.execute(
                "DELETE FROM drafts WHERE contact_id = ?1",
                params![contact_id],
            )
        } else {
            conn.execute(
                "INSERT INTO drafts (contact_id, text) VALUES (?1, ?2)
                 ON CONFLICT(contact_id) DO UPDATE SET text = excluded.text",
                params![contact_id, self.vault.seal_str(text)],
            )
        };
    }
}

const MSG_COLS: &str =
    "id, contact_id, outgoing, body, sent_at, received_at, status, read, attachment, sender, extra";

fn chat_prefs_in(conn: &Connection, vault: &Vault, contact_id: &str) -> ChatPrefs {
    conn.query_row(
        "SELECT sealed FROM chat_prefs WHERE contact_id = ?1",
        params![contact_id],
        |r| r.get::<_, Vec<u8>>(0),
    )
    .ok()
    .and_then(|sealed| vault.open(&sealed))
    .and_then(|json| serde_json::from_slice(&json).ok())
    .unwrap_or_default()
}

fn contact_from_row(r: &Row<'_>, vault: &Vault) -> rusqlite::Result<Contact> {
    Ok(Contact {
        id: r.get(0)?,
        their_name: r
            .get::<_, Option<Vec<u8>>>(1)?
            .map(|sealed| vault.open_str(&sealed))
            .unwrap_or_default(),
        nickname: r
            .get::<_, Option<Vec<u8>>>(2)?
            .map(|sealed| vault.open_str(&sealed)),
        state: ContactState::parse(&r.get::<_, String>(3)?),
        last_seen: r.get(4)?,
        created_at: r.get(5)?,
        avatar: r.get(6)?,
        kind: ContactKind::parse(&r.get::<_, String>(7)?),
        pinned: r.get(8)?,
        muted: r.get(9)?,
    })
}

fn message_from_row(r: &Row<'_>, vault: &Vault) -> rusqlite::Result<Message> {
    let attachment: Option<Vec<u8>> = r.get(8)?;
    Ok(Message {
        id: r.get(0)?,
        contact_id: r.get(1)?,
        outgoing: r.get(2)?,
        body: vault.open_str(&r.get::<_, Vec<u8>>(3)?),
        sent_at: r.get(4)?,
        received_at: r.get(5)?,
        status: MessageStatus::from_i64(r.get(6)?),
        read: r.get(7)?,
        attachment: attachment
            .and_then(|sealed| vault.open(&sealed))
            .and_then(|json| serde_json::from_slice(&json).ok()),
        sender: r.get(9)?,
        extra: r
            .get::<_, Option<Vec<u8>>>(10)?
            .and_then(|sealed| vault.open(&sealed))
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default(),
    })
}

/// Renames the database and its WAL/SHM files to `<name>.corrupt-<ms>`.
fn quarantine(path: &Path) -> Option<PathBuf> {
    let ts = now_ms();
    let backup = path.with_extension(format!("corrupt-{ts}"));
    std::fs::rename(path, &backup).ok()?;
    for suffix in ["-wal", "-shm"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        let mut side_backup = backup.as_os_str().to_owned();
        side_backup.push(suffix);
        let _ = std::fs::rename(side, side_backup);
    }
    Some(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(id: &str, contact: &str, outgoing: bool, sent_at: Millis) -> Message {
        Message {
            id: id.into(),
            contact_id: contact.into(),
            outgoing,
            body: format!("body {id}"),
            sent_at,
            received_at: sent_at,
            status: MessageStatus::Queued,
            read: false,
            attachment: None,
            sender: None,
            extra: MessageExtra::default(),
        }
    }

    #[test]
    fn attachments_round_trip() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_contact("k", ContactState::Accepted, None, None)
            .unwrap();
        let mut m = msg("f1", "k", true, 1);
        m.attachment = Some(Attachment {
            name: "a.pdf".into(),
            mime: "application/pdf".into(),
            size: 10,
            sha256: "00".repeat(32),
            path: None,
            state: TransferState::Transferring,
            duration_ms: Some(5),
        });
        db.insert_message(&m).unwrap();
        assert_eq!(db.message("f1").unwrap().attachment, m.attachment);
        let done = db
            .set_transfer("f1", TransferState::Done, Some("/x/f1.pdf"))
            .unwrap();
        let a = done.attachment.unwrap();
        assert_eq!(
            (a.state, a.path.as_deref()),
            (TransferState::Done, Some("/x/f1.pdf"))
        );
        db.set_avatar("k", Some("/x/k.jpg"));
        assert_eq!(db.contact("k").unwrap().avatar.as_deref(), Some("/x/k.jpg"));
    }

    #[test]
    fn settings_round_trip_and_default() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.settings(), Settings::default());
        let s = Settings {
            display_name: "Roy".into(),
            read_receipts: false,
            ..Settings::default()
        };
        db.save_settings(&s).unwrap();
        assert_eq!(db.settings(), s);
    }

    #[test]
    fn messages_dedupe_page_and_status_only_moves_forward() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_contact("k1", ContactState::Accepted, Some("Jara"), None)
            .unwrap();
        for i in 0..5 {
            assert!(
                db.insert_message(&msg(&format!("m{i}"), "k1", true, i))
                    .unwrap()
            );
        }
        assert!(
            !db.insert_message(&msg("m0", "k1", true, 0)).unwrap(),
            "dedupe"
        );

        let page = db.messages("k1", Some(4), 2);
        assert_eq!(
            page.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["m2", "m3"]
        );

        assert!(db.advance_status("k1", "m1", MessageStatus::Read));
        assert!(
            !db.advance_status("k1", "m1", MessageStatus::Delivered),
            "no going back"
        );
        assert!(
            !db.advance_status("other", "m2", MessageStatus::Read),
            "wrong contact"
        );
        assert_eq!(db.undelivered("k1").len(), 4);
    }

    #[test]
    fn unread_read_receipts_and_cascade_delete() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_contact("k1", ContactState::Accepted, None, Some("J"))
            .unwrap();
        db.insert_message(&msg("a", "k1", false, 1)).unwrap();
        db.insert_message(&msg("b", "k1", false, 2)).unwrap();
        db.set_draft("k1", "hello");
        assert_eq!(db.chat_summaries()[0].unread, 2);

        let read = db.mark_read("k1");
        assert_eq!(read.len(), 2);
        assert_eq!(db.chat_summaries()[0].unread, 0);
        assert_eq!(db.pending_read_receipts("k1").len(), 2);
        db.mark_read_receipts_sent(&read);
        assert!(db.pending_read_receipts("k1").is_empty());

        db.delete_contact("k1");
        assert!(db.message("a").is_none());
        assert_eq!(db.draft("k1"), "");
    }

    #[test]
    fn upsert_keeps_names_when_not_given() {
        let db = Database::open_in_memory().unwrap();
        db.upsert_contact("k", ContactState::PendingIn, Some("Jara"), None)
            .unwrap();
        let c = db
            .upsert_contact("k", ContactState::Accepted, None, Some("Jaar"))
            .unwrap();
        assert_eq!(c.their_name, "Jara");
        assert_eq!(c.display_name(), "Jaar");
        assert_eq!(c.state, ContactState::Accepted);
    }

    #[test]
    fn wipe_removes_everything() {
        let db = Database::open_in_memory().unwrap();
        db.save_settings(&Settings {
            display_name: "Roy".into(),
            ..Settings::default()
        })
        .unwrap();
        db.upsert_contact("k", ContactState::Accepted, None, None)
            .unwrap();
        db.insert_message(&msg("m", "k", true, 1)).unwrap();
        db.set_draft("k", "x");
        db.wipe().unwrap();
        assert_eq!(db.settings(), Settings::default());
        assert!(db.contact("k").is_none());
        assert!(db.message("m").is_none());
        assert_eq!(db.draft("k"), "");
    }

    #[test]
    fn corrupt_file_is_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("whatsmine.db");
        std::fs::write(&path, b"this is not a sqlite database at all, just junk").unwrap();
        let (db, rec) = Database::open_with_recovery(&path, Arc::new(Vault::in_memory()));
        match rec {
            DbRecovery::Recovered { backup } => assert!(backup.exists()),
            other => panic!("expected Recovered, got {other:?}"),
        }
        db.save_settings(&Settings::default()).unwrap();
    }
}
