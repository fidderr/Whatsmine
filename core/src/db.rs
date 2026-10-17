//! Local storage. Everything lives in one SQLite file on this device.
//!
//! Patterns ported from AlarmFree: `PRAGMA user_version` migrations, WAL, and
//! open-with-recovery (a corrupt file is quarantined, not deleted).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::model::{
    Attachment, ChatSummary, Contact, ContactKind, ContactState, Message, MessageStatus, Millis,
    Settings, TransferState, now_ms,
};
use crate::protocol::GroupState;

const CURRENT_VERSION: u32 = 3;

/// v2 → v3: groups. A group is a row in `contacts` (kind 'group') so chats,
/// messages, drafts and unread counts work unchanged; the rest lives here.
const MIGRATE_V3: &str = "
ALTER TABLE contacts ADD COLUMN kind TEXT NOT NULL DEFAULT 'person';
ALTER TABLE messages ADD COLUMN sender TEXT;
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
";

/// v1 → v2: attachments (JSON) on messages, profile pictures on contacts.
const MIGRATE_V2: &str = "
ALTER TABLE messages ADD COLUMN attachment TEXT;
ALTER TABLE contacts ADD COLUMN avatar TEXT;
";

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS settings (
    id   INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS contacts (
    id             TEXT PRIMARY KEY,
    their_name     TEXT NOT NULL DEFAULT '',
    nickname       TEXT,
    state          TEXT NOT NULL,
    last_seen      INTEGER,
    created_at     INTEGER NOT NULL,
    last_activity  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id                 TEXT PRIMARY KEY,
    contact_id         TEXT NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
    outgoing           INTEGER NOT NULL,
    body               TEXT NOT NULL,
    sent_at            INTEGER NOT NULL,
    received_at        INTEGER NOT NULL,
    status             INTEGER NOT NULL DEFAULT 0,
    read               INTEGER NOT NULL DEFAULT 0,
    read_receipt_sent  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS messages_by_contact ON messages(contact_id, sent_at);
CREATE TABLE IF NOT EXISTS drafts (
    contact_id TEXT PRIMARY KEY REFERENCES contacts(id) ON DELETE CASCADE,
    text       TEXT NOT NULL
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
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let version: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(SCHEMA_V1)?;
        }
        if version < 2 {
            conn.execute_batch(MIGRATE_V2)?;
        }
        if version < 3 {
            conn.execute_batch(MIGRATE_V3)?;
        }
        conn.execute_batch(&format!("PRAGMA user_version = {CURRENT_VERSION}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Opens `path`, recovering from corruption instead of failing.
    pub fn open_with_recovery(path: &Path) -> (Self, DbRecovery) {
        if let Ok(db) = Self::open_healthy(path) {
            return (db, DbRecovery::Healthy);
        }
        if let Some(backup) = quarantine(path)
            && let Ok(db) = Self::open_healthy(path)
        {
            return (db, DbRecovery::Recovered { backup });
        }
        let db = Self::open_in_memory().expect("in-memory SQLite must open");
        (db, DbRecovery::Volatile)
    }

    fn open_healthy(path: &Path) -> rusqlite::Result<Self> {
        let db = Self::open(path)?;
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
            "DELETE FROM drafts; DELETE FROM group_acks; DELETE FROM messages; DELETE FROM group_members; DELETE FROM groups; DELETE FROM contacts;
             DELETE FROM settings; PRAGMA wal_checkpoint(TRUNCATE); VACUUM;",
        )
    }

    // ---- settings ----

    pub fn settings(&self) -> Settings {
        self.lock()
            .query_row("SELECT data FROM settings WHERE id = 1", [], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, s: &Settings) -> rusqlite::Result<()> {
        let json = serde_json::to_string(s).expect("settings serialize");
        self.lock().execute(
            "INSERT INTO settings (id, data) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            params![json],
        )?;
        Ok(())
    }

    // ---- contacts ----

    pub fn contact(&self, id: &str) -> Option<Contact> {
        self.lock()
            .query_row(
                "SELECT id, their_name, nickname, state, last_seen, created_at, avatar, kind
                 FROM contacts WHERE id = ?1",
                params![id],
                contact_from_row,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn contacts_in(&self, states: &[ContactState]) -> Vec<Contact> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, their_name, nickname, state, last_seen, created_at, avatar, kind
                 FROM contacts ORDER BY last_activity DESC",
            )
            .expect("valid SQL");
        stmt.query_map([], contact_from_row)
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
        {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO contacts (id, their_name, nickname, state, created_at, last_activity)
                 VALUES (?1, COALESCE(?2, ''), ?3, ?4, ?5, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                    state = excluded.state,
                    their_name = COALESCE(?2, contacts.their_name),
                    nickname = COALESCE(?3, contacts.nickname),
                    last_activity = excluded.last_activity",
                params![id, their_name, nickname, state.as_str(), now],
            )?;
        }
        Ok(self.contact(id).expect("just upserted"))
    }

    pub fn set_their_name(&self, id: &str, name: &str) -> bool {
        self.lock()
            .execute(
                "UPDATE contacts SET their_name = ?2 WHERE id = ?1 AND their_name != ?2",
                params![id, name],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn set_nickname(&self, id: &str, nickname: Option<&str>) {
        let _ = self.lock().execute(
            "UPDATE contacts SET nickname = ?2 WHERE id = ?1",
            params![id, nickname],
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

    pub fn clear_chat(&self, id: &str) {
        let _ = self
            .lock()
            .execute("DELETE FROM messages WHERE contact_id = ?1", params![id]);
    }

    pub fn chat_summaries(&self) -> Vec<ChatSummary> {
        let contacts = self.contacts_in(&[ContactState::Accepted, ContactState::PendingOut]);
        let conn = self.lock();
        contacts
            .into_iter()
            .map(|contact| {
                let last_message = conn
                    .query_row(
                        &format!(
                            "SELECT {MSG_COLS} FROM messages WHERE contact_id = ?1
                             ORDER BY sent_at DESC, id DESC LIMIT 1"
                        ),
                        params![contact.id],
                        message_from_row,
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
                ChatSummary {
                    contact,
                    last_message,
                    unread,
                }
            })
            .collect()
    }

    // ---- messages ----

    /// Inserts a message. Returns false if a message with this id already exists.
    pub fn insert_message(&self, m: &Message) -> rusqlite::Result<bool> {
        let conn = self.lock();
        let n = conn.execute(
            "INSERT OR IGNORE INTO messages
             (id, contact_id, outgoing, body, sent_at, received_at, status, read, attachment, sender)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                m.id,
                m.contact_id,
                m.outgoing,
                m.body,
                m.sent_at,
                m.received_at,
                m.status as i64,
                m.read,
                m.attachment
                    .as_ref()
                    .map(|a| serde_json::to_string(a).expect("attachment serializes")),
                m.sender
            ],
        )?;
        if n > 0 {
            conn.execute(
                "UPDATE contacts SET last_activity = ?2 WHERE id = ?1",
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
                message_from_row,
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
                 WHERE contact_id = ?1 AND sent_at < ?2
                 ORDER BY sent_at DESC, id DESC LIMIT ?3"
            ))
            .expect("valid SQL");
        let mut out: Vec<Message> = stmt
            .query_map(
                params![contact_id, before.unwrap_or(i64::MAX), limit],
                message_from_row,
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
                 WHERE contact_id = ?1 AND outgoing = 1 AND status < 2
                 ORDER BY sent_at ASC, id ASC"
            ))
            .expect("valid SQL");
        stmt.query_map(params![contact_id], message_from_row)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Moves an outgoing message's status forward (never backward).
    /// Returns true if it changed.
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
        let _ = self
            .lock()
            .execute("DELETE FROM messages WHERE id = ?1", params![id]);
    }

    /// Replaces a message's attachment metadata.
    pub fn set_attachment(&self, id: &str, a: &Attachment) {
        let json = serde_json::to_string(a).expect("attachment serializes");
        let _ = self.lock().execute(
            "UPDATE messages SET attachment = ?2 WHERE id = ?1",
            params![id, json],
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
        let name: String = conn
            .query_row(
                "SELECT their_name FROM contacts WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok()?;
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
            params![state.id, state.name, now],
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
                    params![m, name, now],
                )?;
                if !name.is_empty() {
                    conn.execute(
                        "UPDATE contacts SET their_name = ?2 WHERE id = ?1 AND their_name = ''",
                        params![m, name],
                    )?;
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
                    (SELECT message_id FROM group_acks WHERE member_id = ?1 AND status < 2)
                 ORDER BY sent_at ASC, id ASC"
            ))
            .expect("valid SQL");
        stmt.query_map(params![member], message_from_row)
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
                 WHERE contact_id = ?1 AND outgoing = 1 AND status < 2
                 ORDER BY sent_at ASC, id ASC"
            ))
            .expect("valid SQL");
        stmt.query_map(params![group_id], message_from_row)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    // ---- drafts ----

    pub fn draft(&self, contact_id: &str) -> String {
        self.lock()
            .query_row(
                "SELECT text FROM drafts WHERE contact_id = ?1",
                params![contact_id],
                |r| r.get(0),
            )
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
                params![contact_id, text],
            )
        };
    }
}

const MSG_COLS: &str =
    "id, contact_id, outgoing, body, sent_at, received_at, status, read, attachment, sender";

fn contact_from_row(r: &Row<'_>) -> rusqlite::Result<Contact> {
    Ok(Contact {
        id: r.get(0)?,
        their_name: r.get(1)?,
        nickname: r.get(2)?,
        state: ContactState::parse(&r.get::<_, String>(3)?),
        last_seen: r.get(4)?,
        created_at: r.get(5)?,
        avatar: r.get(6)?,
        kind: ContactKind::parse(&r.get::<_, String>(7)?),
    })
}

fn message_from_row(r: &Row<'_>) -> rusqlite::Result<Message> {
    let attachment: Option<String> = r.get(8)?;
    Ok(Message {
        id: r.get(0)?,
        contact_id: r.get(1)?,
        outgoing: r.get(2)?,
        body: r.get(3)?,
        sent_at: r.get(4)?,
        received_at: r.get(5)?,
        status: MessageStatus::from_i64(r.get(6)?),
        read: r.get(7)?,
        attachment: attachment.and_then(|s| serde_json::from_str(&s).ok()),
        sender: r.get(9)?,
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
        }
    }

    #[test]
    fn attachments_round_trip_and_migration_upgrades_a_v1_file() {
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

        // A database written by the previous version (schema v1) upgrades in place.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch("PRAGMA user_version = 1").unwrap();
            conn.execute(
                "INSERT INTO contacts (id, state, created_at, last_activity) VALUES ('c', 'accepted', 1, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages (id, contact_id, outgoing, body, sent_at, received_at) VALUES ('m', 'c', 1, 'hoi', 1, 1)",
                [],
            )
            .unwrap();
        }
        let db = Database::open(&path).unwrap();
        let old = db.message("m").unwrap();
        assert_eq!((old.body.as_str(), old.attachment), ("hoi", None));
        assert_eq!(db.contact("c").unwrap().avatar, None);
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
        let (db, rec) = Database::open_with_recovery(&path);
        match rec {
            DbRecovery::Recovered { backup } => assert!(backup.exists()),
            other => panic!("expected Recovered, got {other:?}"),
        }
        db.save_settings(&Settings::default()).unwrap();
    }
}
