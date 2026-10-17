//! Plain data types shared by the core and the UI.

use serde::{Deserialize, Serialize};

/// Milliseconds since the Unix epoch.
pub type Millis = i64;

pub fn now_ms() -> Millis {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Where a contact relationship stands, from our side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContactState {
    /// They asked to connect; we have not answered.
    PendingIn,
    /// We asked to connect; they have not answered.
    PendingOut,
    /// Both sides agreed. Messages flow.
    Accepted,
    /// We blocked them. Their connections are refused.
    Blocked,
    /// Not a contact: someone we share a group with. We connect to them
    /// for group messages and calls, and for nothing else.
    GroupPeer,
}

impl ContactState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingIn => "pending_in",
            Self::PendingOut => "pending_out",
            Self::Accepted => "accepted",
            Self::Blocked => "blocked",
            Self::GroupPeer => "group_peer",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "pending_in" => Self::PendingIn,
            "pending_out" => Self::PendingOut,
            "accepted" => Self::Accepted,
            "group_peer" => Self::GroupPeer,
            _ => Self::Blocked,
        }
    }
}

/// A conversation is with one person or with a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContactKind {
    Person,
    Group,
}

impl ContactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Group => "group",
        }
    }

    pub fn parse(s: &str) -> Self {
        if s == "group" {
            Self::Group
        } else {
            Self::Person
        }
    }
}

/// Group ids start with this, so they can never be mistaken for a key.
pub const GROUP_ID_PREFIX: &str = "g:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    /// The contact's public key (z-base-32). This is their identity.
    pub id: String,
    /// The name they chose for themselves.
    pub their_name: String,
    /// The name we gave them. Takes precedence over `their_name`.
    pub nickname: Option<String>,
    pub state: ContactState,
    /// When their last session with us ended. `None` if never seen or hidden.
    pub last_seen: Option<Millis>,
    pub created_at: Millis,
    /// Local path of their profile picture, if they shared one.
    pub avatar: Option<String>,
    /// A person, or a group (then `id` is the group id and `their_name`
    /// the group's name).
    pub kind: ContactKind,
}

impl Contact {
    /// The name to show in the UI.
    pub fn display_name(&self) -> String {
        if let Some(n) = self.nickname.as_deref().filter(|n| !n.trim().is_empty()) {
            return n.to_string();
        }
        if !self.their_name.trim().is_empty() {
            return self.their_name.clone();
        }
        short_id(&self.id)
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Delivery state of a message we sent. Ordered: a status only moves forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MessageStatus {
    /// Saved locally, waiting for the contact to be reachable.
    Queued = 0,
    /// Written to a live connection, not yet acknowledged.
    Sent = 1,
    /// The contact's device stored it.
    Delivered = 2,
    /// The contact opened the chat.
    Read = 3,
}

impl MessageStatus {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Sent,
            2 => Self::Delivered,
            3 => Self::Read,
            _ => Self::Queued,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// UUID v7 (time-ordered), chosen by the sender. Used for dedupe.
    pub id: String,
    pub contact_id: String,
    pub outgoing: bool,
    pub body: String,
    /// Sender's clock.
    pub sent_at: Millis,
    /// Our clock, when it was stored here.
    pub received_at: Millis,
    /// Delivery status (meaningful for outgoing messages).
    pub status: MessageStatus,
    /// For incoming messages: whether we have read it.
    pub read: bool,
    /// A file (photo, video, voice note, document). `body` is then its
    /// caption, possibly empty.
    pub attachment: Option<Attachment>,
    /// In a group: who wrote an incoming message (their key).
    pub sender: Option<String>,
}

/// Where a file transfer stands, on either side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    /// Waiting for the other device or moving bytes.
    Transferring,
    /// The complete, verified file is on the receiver.
    Done,
    /// The receiver refused it (over their size limit).
    Declined,
    /// Could not be completed (for example the data was corrupt twice).
    Failed,
}

/// What kind of viewer an attachment gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    File,
}

/// A file sent or received as a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Original file name, display only.
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// SHA-256 of the content, hex. The receiver verifies it.
    pub sha256: String,
    /// The local copy, once we have the whole file (always for our own).
    pub path: Option<String>,
    pub state: TransferState,
    /// Voice notes and videos: length, if known.
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

impl Attachment {
    pub fn kind(&self) -> MediaKind {
        match self.mime.split('/').next().unwrap_or("") {
            "image" => MediaKind::Image,
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            _ => MediaKind::File,
        }
    }
}

/// One member of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMember {
    pub id: String,
    /// The name to show: our nickname for them, their own name, or the name
    /// the group's admin gave.
    pub name: String,
    pub is_me: bool,
    pub is_admin: bool,
    pub online: bool,
    /// One of our contacts (so a 1:1 chat with them exists).
    pub is_contact: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    pub id: String,
    pub name: String,
    pub admin: String,
    pub i_am_admin: bool,
    /// False once we left, were removed, or the admin closed the group:
    /// the history stays but nothing more is sent or received.
    pub active: bool,
    pub members: Vec<GroupMember>,
}

/// A chat-list row: contact plus last message and unread count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSummary {
    pub contact: Contact,
    pub last_message: Option<Message>,
    pub unread: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub display_name: String,
    pub onboarded: bool,
    /// Send read receipts (and show the contact's).
    pub read_receipts: bool,
    /// Share our last seen (and show the contact's).
    pub share_last_seen: bool,
    /// Send typing indicators (and show the contact's).
    pub typing_indicators: bool,
    /// UI theme: "system", "dark" or "light".
    pub theme: String,
    /// UI language code, for example "en".
    pub locale: String,
    /// Extra relay URLs on top of the defaults.
    pub extra_relays: Vec<String>,
    /// Our profile picture (local path), shared with contacts.
    pub avatar: Option<String>,
    /// Largest incoming file we accept, in MB. 0 = no limit.
    pub max_file_mb: u64,
    /// Show a notification for new messages, requests and calls.
    pub notifications: bool,
    /// Put the sender and the text in notifications (off: just "new message").
    pub notification_previews: bool,
    /// Keep running when the window or app is closed, so messages arrive at
    /// once (Android: a quiet background service; Windows: the tray).
    pub background: bool,
    /// Windows: start with the PC (hidden, in the tray).
    pub start_with_system: bool,
}

/// Default for [`Settings::max_file_mb`].
pub const DEFAULT_MAX_FILE_MB: u64 = 2048;

impl Default for Settings {
    fn default() -> Self {
        Self {
            display_name: String::new(),
            onboarded: false,
            read_receipts: true,
            share_last_seen: true,
            typing_indicators: true,
            theme: "system".into(),
            locale: "en".into(),
            extra_relays: Vec::new(),
            avatar: None,
            max_file_mb: DEFAULT_MAX_FILE_MB,
            notifications: true,
            notification_previews: true,
            background: true,
            start_with_system: false,
        }
    }
}

/// Events the core pushes to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreEvent {
    /// Something in the contact list changed; refresh it.
    ContactsChanged,
    MessageAdded(Message),
    MessageStatus {
        contact_id: String,
        message_id: String,
        status: MessageStatus,
    },
    Presence {
        contact_id: String,
        online: bool,
        last_seen: Option<Millis>,
    },
    Typing {
        contact_id: String,
    },
    RequestReceived {
        contact_id: String,
        name: String,
    },
    Accepted {
        contact_id: String,
    },
    Net(NetStatus),
    /// A message changed in place (attachment progress state, path).
    MessageUpdated(Message),
    /// Something happened to a voice/video call.
    Call(crate::calls::CallEvent),
    /// Bytes moved for a file, either direction. Throttled.
    Transfer {
        contact_id: String,
        message_id: String,
        done: u64,
        total: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetStatus {
    Connecting,
    Online,
    Offline,
}
