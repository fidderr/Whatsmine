//! Two devices, one person: a phone and a PC. Both hold the same person key and location secret,
//! each has its own network key and its own record in the DHT, and each talks to contacts by
//! itself. Whatever happens on one goes to the other, in order, and is sent until confirmed.
//!
//! Applying a change twice changes nothing, so nothing can go back and forth for ever: whoever
//! gets something it already has says nothing more about it.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::db::Database;
use crate::engine::Cmd;
use crate::model::{
    ContactKeys, ContactKind, ContactState, CoreEvent, Message, MessageStatus, MyInvite,
};
use crate::protocol::GroupState;
use crate::rich::{ChatPrefs, Folder};

/// How many devices can be one person.
pub const DEVICES: u8 = 2;
const DEVICE_SECRET: &str = "device";

/// What this device is.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DeviceState {
    /// Which record in the DHT is ours.
    pub slot: u8,
    pub linked: bool,
    /// We asked the other one to go, and it has not said so yet.
    pub removing: bool,
}

pub fn device_state(db: &Database) -> DeviceState {
    db.secret(DEVICE_SECRET)
        .and_then(|json| serde_json::from_slice(&json).ok())
        .unwrap_or_default()
}

pub fn save_device_state(db: &Database, state: &DeviceState) {
    if let Ok(json) = serde_json::to_vec(state) {
        let _ = db.set_secret(DEVICE_SECRET, &json);
    }
}

/// One change, for the other device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SyncEvent {
    Message(Box<Message>),
    Status {
        chat: String,
        id: String,
        status: MessageStatus,
    },
    Gone {
        chat: String,
        id: String,
    },
    Read {
        chat: String,
    },
    Contact(Box<ContactSync>),
    ContactGone {
        id: String,
    },
    Group {
        state: Box<GroupState>,
        sig: Vec<u8>,
        active: bool,
        secret: Option<[u8; 32]>,
    },
    Invites(Vec<MyInvite>),
    Location([u8; 32]),
    Name(String),
    Folders(Vec<Folder>),
    /// This device is to stop being you.
    Remove,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContactSync {
    pub id: String,
    pub state: ContactState,
    pub kind: ContactKind,
    pub their_name: String,
    pub nickname: Option<String>,
    pub pinned: bool,
    pub muted: bool,
    pub keys: ContactKeys,
    pub prefs: ChatPrefs,
}

/// What is the same on both devices. How a device got there is its own business.
fn shared_keys(mut keys: ContactKeys) -> ContactKeys {
    keys.curve = None;
    keys.network = None;
    keys.told = false;
    keys
}

fn contact_sync(db: &Database, id: &str) -> Option<ContactSync> {
    let c = db.contact(id)?;
    Some(ContactSync {
        id: c.id.clone(),
        state: c.state,
        kind: c.kind,
        their_name: c.their_name,
        nickname: c.nickname,
        pinned: c.pinned,
        muted: c.muted,
        keys: shared_keys(db.contact_keys(id).unwrap_or_default()),
        prefs: db.chat_prefs(id),
    })
}

/// What the last look saw: how each thing was, as a short hash.
#[derive(Default)]
struct Seen {
    contacts: HashMap<String, [u8; 32]>,
    unread: HashMap<String, u32>,
    groups: HashMap<String, [u8; 32]>,
    invites: [u8; 32],
    location: [u8; 32],
    name: String,
    folders: [u8; 32],
}

fn hash_of<T: Serialize>(value: &T) -> [u8; 32] {
    *blake3::hash(&serde_json::to_vec(value).unwrap_or_default()).as_bytes()
}

const ALL_STATES: [ContactState; 5] = [
    ContactState::Accepted,
    ContactState::PendingIn,
    ContactState::PendingOut,
    ContactState::Blocked,
    ContactState::GroupPeer,
];

/// Looks what changed since the last look, and notes it down for the other device.
fn look(db: &Database, seen: &mut Seen, first: bool) -> Vec<SyncEvent> {
    let mut out = Vec::new();
    let contacts = db.contacts_in(&ALL_STATES);
    let mut now: HashMap<String, [u8; 32]> = HashMap::new();
    for c in &contacts {
        let Some(sync) = contact_sync(db, &c.id) else {
            continue;
        };
        let hash = hash_of(&sync);
        if seen.contacts.get(&c.id) != Some(&hash) && !first {
            out.push(SyncEvent::Contact(Box::new(sync)));
        }
        now.insert(c.id.clone(), hash);
    }
    for id in seen.contacts.keys() {
        if !now.contains_key(id) {
            out.push(SyncEvent::ContactGone { id: id.clone() });
        }
    }
    seen.contacts = now;

    for chat in db.chat_summaries() {
        let id = chat.contact.id;
        let before = seen.unread.insert(id.clone(), chat.unread).unwrap_or(0);
        if before > 0 && chat.unread == 0 && !first {
            out.push(SyncEvent::Read { chat: id });
        }
    }

    for c in contacts.iter().filter(|c| c.kind == ContactKind::Group) {
        let Some((state, sig, active)) = db.group(&c.id) else {
            continue;
        };
        let hash = hash_of(&(&state, active));
        if seen.groups.get(&c.id) != Some(&hash) {
            if !first {
                let secret = db
                    .secret(&crate::groups::secret_name(&c.id))
                    .and_then(|bytes| bytes.try_into().ok());
                out.push(SyncEvent::Group {
                    state: Box::new(state),
                    sig,
                    active,
                    secret,
                });
            }
            seen.groups.insert(c.id.clone(), hash);
        }
    }

    let invites = db.invites();
    let hash = hash_of(&invites);
    if seen.invites != hash {
        if !first {
            out.push(SyncEvent::Invites(invites));
        }
        seen.invites = hash;
    }
    if let Ok(me) = crate::identity::Identity::load_or_create(db)
        && seen.location != me.location
    {
        if !first {
            out.push(SyncEvent::Location(me.location));
        }
        seen.location = me.location;
    }
    let name = db.settings().display_name;
    if seen.name != name {
        if !first {
            out.push(SyncEvent::Name(name.clone()));
        }
        seen.name = name;
    }
    let folders: Vec<Folder> = db
        .secret("folders")
        .and_then(|json| serde_json::from_slice(&json).ok())
        .unwrap_or_default();
    let hash = hash_of(&folders);
    if seen.folders != hash {
        if !first {
            out.push(SyncEvent::Folders(folders));
        }
        seen.folders = hash;
    }
    out
}

/// What just came from the other device: when it shows up here as a change, it does not go back.
/// Going back would be harmless if nothing else happened, but an old state on its way could undo a
/// newer one.
#[derive(Clone, Default)]
pub struct Echoes(Arc<Mutex<HashSet<[u8; 32]>>>);

impl Echoes {
    pub fn expect(&self, event: &SyncEvent) {
        let mut set = self.0.lock().unwrap_or_else(|p| p.into_inner());
        // Whatever never showed up as a change must not pile up.
        if set.len() > 4096 {
            set.clear();
        }
        set.insert(hash_of(event));
    }

    fn is_echo(&self, event: &SyncEvent) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&hash_of(event))
    }
}

/// A message as it goes to the other device: where a file lies here means nothing there.
pub fn without_path(m: &Message) -> Message {
    let mut copy = m.clone();
    if let Some(a) = &mut copy.attachment {
        a.path = None;
    }
    copy
}

/// Follows everything that happens, for as long as there is another device.
pub(crate) async fn watch(
    db: Arc<Database>,
    mut events: broadcast::Receiver<CoreEvent>,
    cmds: mpsc::UnboundedSender<Cmd>,
    linked: Arc<AtomicBool>,
    echoes: Echoes,
) {
    use broadcast::error::RecvError;
    let mut seen = Seen::default();
    let _ = look(&db, &mut seen, true);
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(_)) => CoreEvent::ContactsChanged,
            Err(RecvError::Closed) => return,
        };
        if !linked.load(Ordering::Relaxed) {
            // Whatever happened goes along with the copy when a device is linked.
            if matches!(event, CoreEvent::ContactsChanged) {
                let _ = look(&db, &mut seen, true);
            }
            continue;
        }
        let out = match &event {
            CoreEvent::MessageAdded(m) | CoreEvent::MessageUpdated(m) => {
                vec![SyncEvent::Message(Box::new(without_path(m)))]
            }
            CoreEvent::MessageStatus {
                contact_id,
                message_id,
                status,
            } => vec![SyncEvent::Status {
                chat: contact_id.clone(),
                id: message_id.clone(),
                status: *status,
            }],
            CoreEvent::MessageGone {
                contact_id,
                message_id,
            } => vec![SyncEvent::Gone {
                chat: contact_id.clone(),
                id: message_id.clone(),
            }],
            CoreEvent::ContactsChanged | CoreEvent::Accepted { .. } => look(&db, &mut seen, false),
            _ => Vec::new(),
        };
        let mut any = false;
        for event in &out {
            if !echoes.is_echo(event) {
                queue(&db, event);
                any = true;
            }
        }
        if any {
            let _ = cmds.send(Cmd::SyncOwed);
        }
    }
}

pub fn queue(db: &Database, event: &SyncEvent) {
    if let Ok(bytes) = serde_json::to_vec(event) {
        db.queue_sync(&bytes);
    }
}

// ---- linking ----

/// What it takes to become the other device, passed over a connection only the link code opens.
#[derive(Serialize, Deserialize)]
pub struct Bundle {
    pub vault_key: [u8; 32],
    pub db: Vec<u8>,
    pub slot: u8,
}

pub const LINK_ALPN: &[u8] = b"whatsmine/link/1";
const LINK_CONTEXT: &[u8] = b"whatsmine-link-v1";

fn code_prefix() -> &'static str {
    "whatsmine://link/"
}

/// What to scan or type on the other device.
pub fn link_code(secret: &[u8; 32]) -> String {
    format!(
        "{}{}",
        code_prefix(),
        data_encoding::BASE32_NOPAD.encode(secret).to_lowercase()
    )
}

pub fn parse_link_code(text: &str) -> Option<[u8; 32]> {
    let rest = text.trim().strip_prefix(code_prefix())?;
    data_encoding::BASE32_NOPAD
        .decode(rest.to_ascii_uppercase().as_bytes())
        .ok()?
        .try_into()
        .ok()
}

/// Where the device with the code leaves how to reach it.
pub fn link_seed(secret: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("whatsmine link seed v1", secret)
}

fn link_key(secret: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("whatsmine link key v1", secret)
}

pub fn seal_for_link(secret: &[u8; 32], plain: &[u8]) -> Vec<u8> {
    crate::vault::seal_with(&link_key(secret), plain, LINK_CONTEXT)
}

pub fn open_from_link(secret: &[u8; 32], sealed: &[u8]) -> Option<Vec<u8>> {
    crate::vault::open_with(&link_key(secret), sealed, LINK_CONTEXT)
}

/// Proof over this very connection that whoever is there has the code.
pub fn link_proof(secret: &[u8; 32], exporter: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("whatsmine link proof v1");
    hasher.update(secret);
    hasher.update(exporter);
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_code_comes_back_and_nothing_else_does() {
        let secret = crate::vault::random();
        assert_eq!(parse_link_code(&link_code(&secret)), Some(secret));
        assert_eq!(parse_link_code("hello"), None);
        assert_eq!(parse_link_code("whatsmine://link/aaaa"), None);
        let sealed = seal_for_link(&secret, b"everything");
        assert_eq!(
            open_from_link(&secret, &sealed).as_deref(),
            Some(b"everything".as_slice())
        );
        assert_eq!(open_from_link(&crate::vault::random(), &sealed), None);
    }

    #[test]
    fn what_differs_per_device_stays_out() {
        let keys = ContactKeys {
            curve: Some([1; 32]),
            network: Some([2; 32]),
            told: true,
            greeting: "hi".into(),
            ..ContactKeys::default()
        };
        let shared = shared_keys(keys);
        assert_eq!(shared.curve, None);
        assert_eq!(shared.network, None);
        assert!(!shared.told);
        assert_eq!(shared.greeting, "hi");
    }
}
