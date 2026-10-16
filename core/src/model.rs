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
}

impl ContactState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PendingIn => "pending_in",
            Self::PendingOut => "pending_out",
            Self::Accepted => "accepted",
            Self::Blocked => "blocked",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "pending_in" => Self::PendingIn,
            "pending_out" => Self::PendingOut,
            "accepted" => Self::Accepted,
            _ => Self::Blocked,
        }
    }
}

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
}

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetStatus {
    Connecting,
    Online,
    Offline,
}
