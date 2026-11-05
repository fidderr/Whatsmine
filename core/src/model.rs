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
    /// Kept at the top of the chat list.
    pub pinned: bool,
    /// No notifications for this chat.
    pub muted: bool,
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
    /// Waits for its moment, on this device only.
    Scheduled = -2,
    /// We stopped trying. It can be tried again, and a late receipt still counts.
    NotDelivered = -1,
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
            -2 => Self::Scheduled,
            -1 => Self::NotDelivered,
            1 => Self::Sent,
            2 => Self::Delivered,
            3 => Self::Read,
            _ => Self::Queued,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Replies, reactions, polls and the rest.
    #[serde(default)]
    pub extra: crate::rich::MessageExtra,
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
    /// Over what downloads by itself on this network: fetched when asked for.
    Waiting,
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
    /// The owner, or an admin.
    pub is_admin: bool,
    pub is_owner: bool,
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
    pub i_am_owner: bool,
    pub rules: crate::protocol::GroupRules,
    /// False once we left, were removed, or the admin closed the group:
    /// the history stays but nothing more is sent or received.
    pub active: bool,
    pub members: Vec<GroupMember>,
}

/// A chat-list row: contact plus last message and unread count.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatSummary {
    pub contact: Contact,
    pub last_message: Option<Message>,
    pub unread: u32,
    pub prefs: crate::rich::ChatPrefs,
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
    /// How the app looks: a ready-made look's key, or the id of one of theirs.
    pub app_look: String,
    /// How chats look that have no look of their own. Empty: the same as the app.
    pub chat_look: String,
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
    /// How fast against how much battery.
    pub speed_mode: crate::power::SpeedMode,
    /// Use public relays when a direct connection is impossible.
    pub use_relays: bool,
    /// How long to keep trying to deliver a message. 0 means forever.
    pub keep_trying_days: u32,
    /// Files up to this size download by themselves on Wi-Fi. 0 means none.
    pub auto_download_wifi_mb: u64,
    /// The same on mobile data.
    pub auto_download_mobile_mb: u64,
    /// "small", "normal", "large" or "huge".
    pub text_size: String,
    /// "roomy", "normal", "compact" or "big" for the chat list.
    pub density: String,
    /// Requests wait without an alert.
    pub quiet_requests: bool,
    /// Requests with one of these words go straight to the bin.
    pub request_filter: Vec<String>,
    /// Ask for the fingerprint or screen lock when the app opens.
    pub app_lock: bool,
    /// How long the app may be away before it asks again, in seconds. 0 means at once.
    pub lock_after_s: u32,
    /// No screenshots, and a blank picture in the list of recent apps.
    pub block_screenshots: bool,
    /// Asks the keyboard not to learn from what is typed.
    pub incognito_keyboard: bool,
    /// What a new chat starts with. 0 means off.
    pub disappear_default_ms: i64,
    /// Colours picked lately, the newest first.
    pub recent_colors: Vec<crate::look::Rgb>,
    /// The tabs in the order someone wants them. Empty: as they come.
    pub tab_order: Vec<String>,
    /// What swiping a chat does: "archive", "read", "pin", "mute", "hide" or "" for nothing.
    pub swipe_left: String,
    pub swipe_right: String,
    /// The reactions offered first.
    pub reactions: Vec<String>,
    /// Texts to send with one tap.
    pub quick_replies: Vec<String>,
    /// A backup every week, without being asked.
    pub auto_backup: bool,
    pub last_backup_at: Millis,
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
            app_look: "system".into(),
            chat_look: String::new(),
            locale: "en".into(),
            extra_relays: Vec::new(),
            avatar: None,
            max_file_mb: DEFAULT_MAX_FILE_MB,
            notifications: true,
            notification_previews: true,
            background: true,
            start_with_system: false,
            speed_mode: crate::power::SpeedMode::default(),
            use_relays: true,
            keep_trying_days: 7,
            auto_download_wifi_mb: 2048,
            auto_download_mobile_mb: 16,
            text_size: "normal".into(),
            density: "normal".into(),
            quiet_requests: false,
            request_filter: Vec::new(),
            app_lock: false,
            lock_after_s: 60,
            block_screenshots: false,
            incognito_keyboard: false,
            disappear_default_ms: 0,
            recent_colors: Vec::new(),
            tab_order: Vec::new(),
            swipe_left: "archive".into(),
            swipe_right: "read".into(),
            reactions: ["👍", "❤️", "😂", "😮", "😢", "🙏"]
                .map(str::to_owned)
                .to_vec(),
            quick_replies: Vec::new(),
            auto_backup: false,
            last_backup_at: 0,
        }
    }
}

/// Events the core pushes to the UI.
#[derive(Debug, Clone, PartialEq)]
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
    /// A message was removed: it disappeared, or was taken away.
    MessageGone {
        contact_id: String,
        message_id: String,
    },
    /// Read on our other device.
    ReadElsewhere {
        contact_id: String,
    },
    /// Our other device changed something about how we are shown.
    SettingsChanged,
    /// The other device said this one is to stop being you. Nothing is left here.
    RemovedFromPerson,
    /// You asked to be reminded of this message, now.
    Reminder {
        message_id: String,
    },
    /// Nothing is held right now, and timers stop while the system sleeps: wake us in this long.
    WakeIn {
        ms: u64,
    },
    /// Bytes moved for a file, either direction. Throttled.
    Transfer {
        contact_id: String,
        message_id: String,
        done: u64,
        total: u64,
    },
}

/// How far someone is trusted to be who they say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Trust {
    /// Came through a link or a code.
    #[default]
    Link,
    /// A code was scanned face to face.
    MetInPerson,
    /// The safety words were compared.
    SafetyWords,
}

/// What we know to reach a contact by, and how they got here.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContactKeys {
    /// Finds their sealed address.
    pub location: Option<[u8; 32]>,
    /// Their address when we last knew it, for the same Wi-Fi with no internet.
    pub network: Option<[u8; 32]>,
    /// Their encryption key from the last handshake, for what comes without a connection.
    pub curve: Option<[u8; 32]>,
    /// The invite we used to ask them, until they answer.
    pub invite: Option<crate::identity::Invite>,
    /// What they wrote when asking, until answered.
    pub greeting: String,
    /// Which of our invites they came through.
    pub via: String,
    pub trust: Trust,
    /// They have our current location. False until they are told.
    pub told: bool,
    /// What they gave for passing them on.
    #[serde(default)]
    pub card: Option<String>,
    /// We asked them to let us into a group.
    #[serde(default)]
    pub join: Option<crate::requests::JoinAsk>,
}

/// One way for strangers to ask to be added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyInvite {
    pub id: String,
    pub label: String,
    pub drop_secret: [u8; 32],
    pub box_secret: [u8; 32],
    pub expires_at: Option<Millis>,
    pub max_uses: Option<u32>,
    pub uses: u32,
    pub created_at: Millis,
    pub revoked: bool,
    /// The one the name code and the plain link stand for.
    pub main: bool,
    /// Whoever comes with this was scanned in person: added both ways without asking.
    pub met_token: Option<[u8; 16]>,
}

/// Someone who asked to join a group through its link.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JoinAsker {
    pub id: String,
    pub name: String,
    pub greeting: String,
}

/// What a link to a group does, kept by whoever made it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GroupLinkState {
    pub token: [u8; 16],
    /// Someone with the link waits for an admin.
    pub approve: bool,
    pub asking: Vec<JoinAsker>,
}

/// What the files on this device take.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StorageReport {
    pub media: u64,
    pub files: u64,
    pub voice: u64,
    pub chats: Vec<(String, u64)>,
}

/// Where a message of ours is, for whoever asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryInfo {
    pub status: MessageStatus,
    /// There is a connection to them right now.
    pub connected: bool,
    /// When the next try is. `None`: a connection is there, or we stopped trying.
    pub next_try_in_ms: Option<u64>,
    /// Tries in a row that failed.
    pub failures: u32,
    /// A note was left in the DHT for them.
    pub knocked: bool,
}

/// How one connection runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerPath {
    pub contact_id: String,
    /// Directly, as opposed to through a relay.
    pub direct: bool,
    pub rtt_ms: Option<u64>,
}

/// How the connection stands, in facts, for the screen that explains it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionReport {
    pub network: crate::power::Network,
    pub level: crate::power::Level,
    /// False while resting: nothing is held.
    pub endpoint_up: bool,
    /// One of our addresses is reachable from elsewhere.
    pub reachable_direct: bool,
    pub on_relay: bool,
    pub ipv6: bool,
    /// The router gives another address per destination. `None`: not known.
    pub hard_nat: Option<bool>,
    pub peers: Vec<PeerPath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetStatus {
    Connecting,
    Online,
    Offline,
}
