//! Who you are, and how you can be found. These are separate things on purpose:
//!
//! - The **person key** says who you are. Contacts know it. It says nothing about where you are,
//!   so a leaked one is harmless. It comes from 12 recovery words.
//! - The **network key** is the QUIC address. Whoever you are connected to sees it. It rotates
//!   weekly and on a reset.
//! - The **location secret** finds your sealed address in the DHT, under a key that changes daily.
//!   Only accepted contacts have it. It rotates on a block and on a reset.
//! - **Invites** let a stranger leave a sealed request in the DHT. They never reach your phone,
//!   never learn your address, and cost you no battery. Each can be revoked.

use iroh::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::model::{Millis, now_ms};
use crate::vault::random;

/// The file an earlier generation kept its key in, for the clean start to remove.
pub(crate) const KEY_FILE: &str = "identity.key";
const SECRET_NAME: &str = "identity";
/// The network key changes this often.
pub const NETWORK_KEY_LIFE_MS: i64 = 7 * 24 * 3600 * 1000;
/// After a rotation the old location is still looked at for this long, for contacts who were
/// away and do not know the new one yet.
pub const OLD_LOCATION_LIFE_MS: i64 = 90 * 24 * 3600 * 1000;

/// Base of the links people share. Chat apps do not make `whatsmine://` clickable, and what comes
/// after the `#` never reaches the web server.
pub const LINK_BASE: &str = "https://fidderr.github.io/whatsmine/add#";

#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    entropy: [u8; 16],
    network: [u8; 32],
    network_since: Millis,
    pub location: [u8; 32],
    /// Earlier location secrets, and until when they are looked at.
    pub old_locations: Vec<([u8; 32], Millis)>,
}

impl Identity {
    pub fn load_or_create(db: &Database) -> anyhow::Result<Self> {
        if let Some(bytes) = db.secret(SECRET_NAME) {
            return postcard::from_bytes(&bytes)
                .map_err(|e| anyhow::anyhow!("the stored identity is damaged: {e}"));
        }
        let identity = Self::from_entropy(random());
        identity.save(db)?;
        Ok(identity)
    }

    fn from_entropy(entropy: [u8; 16]) -> Self {
        Self {
            entropy,
            network: random(),
            network_since: now_ms(),
            location: random(),
            old_locations: Vec::new(),
        }
    }

    pub fn save(&self, db: &Database) -> anyhow::Result<()> {
        db.set_secret(SECRET_NAME, &postcard::to_stdvec(self)?)?;
        Ok(())
    }

    pub fn person(&self) -> SecretKey {
        SecretKey::from_bytes(&blake3::derive_key(
            "whatsmine person key v1",
            &self.entropy,
        ))
    }

    pub fn network(&self) -> SecretKey {
        SecretKey::from_bytes(&self.network)
    }

    /// The 12 words that bring this person key back.
    pub fn recovery_words(&self) -> Vec<String> {
        bip39::Mnemonic::from_entropy(&self.entropy)
            .expect("16 bytes make 12 words")
            .words()
            .map(str::to_owned)
            .collect()
    }

    /// The same person again, from the words: new network key and location, since whoever had
    /// the old device may still have those.
    pub fn from_words(words: &str) -> anyhow::Result<Self> {
        let mnemonic: bip39::Mnemonic = words
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
            .parse()
            .map_err(|e| anyhow::anyhow!("those words do not fit: {e}"))?;
        let entropy: [u8; 16] = mnemonic
            .to_entropy()
            .try_into()
            .map_err(|_| anyhow::anyhow!("12 words are needed"))?;
        Ok(Self::from_entropy(entropy))
    }

    /// A new network key once the old one has served its time. True if it changed.
    pub fn network_due(&mut self, now: Millis) -> bool {
        if now - self.network_since < NETWORK_KEY_LIFE_MS {
            return false;
        }
        self.rotate_network(now);
        true
    }

    pub fn rotate_network(&mut self, now: Millis) {
        self.network = random();
        self.network_since = now;
    }

    /// A new location secret. The old one is still looked at for a while, never published to.
    pub fn rotate_location(&mut self, now: Millis) {
        self.old_locations
            .push((self.location, now + OLD_LOCATION_LIFE_MS));
        self.old_locations.retain(|(_, until)| *until > now);
        self.location = random();
    }
}

/// How a key is shown and shared: z-base-32, 52 lowercase characters chosen
/// to avoid look-alikes.
pub fn key_to_string(key: &PublicKey) -> String {
    key.to_z32()
}

/// Parses a person's key. Accepts z-base-32 or hex, ignores spaces and dashes.
pub fn parse_key(input: &str) -> Option<PublicKey> {
    let s: String = input
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_lowercase();
    if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return s.parse().ok();
    }
    PublicKey::from_z32(&s).ok()
}

/// Someone's invitation: who, and how to leave them a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub person: [u8; 32],
    pub name: String,
    /// Where requests are left in the DHT.
    pub drop_secret: [u8; 32],
    /// What requests are sealed to.
    pub box_key: [u8; 32],
    /// Whoever scanned this in person: added both ways without asking.
    pub met_token: Option<[u8; 16]>,
}

impl Invite {
    pub fn key(&self) -> Option<PublicKey> {
        PublicKey::from_bytes(&self.person).ok()
    }

    /// The name to show, if it has one.
    pub fn name(&self) -> Option<&str> {
        let name = self.name.trim();
        (!name.is_empty()).then_some(name)
    }

    fn encoded(&self) -> String {
        let bytes = postcard::to_stdvec(self).expect("an invite serializes");
        data_encoding::BASE32_NOPAD.encode(&bytes).to_lowercase()
    }

    /// The link to share anywhere: opens the app, or a page that says how to get it.
    pub fn link(&self) -> String {
        format!("{LINK_BASE}{}", self.encoded())
    }

    /// What goes in a QR code: smaller than the link, and it is scanned by the app anyway.
    pub fn qr_text(&self) -> String {
        format!("whatsmine://add/{}", self.encoded())
    }
}

/// Finds an invitation in whatever was pasted, scanned or clicked: a link, or a message that
/// contains one.
pub fn parse_invite(text: &str) -> Option<Invite> {
    for token in text.split_whitespace() {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric());
        let Some(rest) = token
            .strip_prefix("whatsmine://add/")
            .or_else(|| token.split_once("/add#").map(|(_, rest)| rest))
        else {
            continue;
        };
        let upper = rest.to_ascii_uppercase();
        if let Ok(bytes) = data_encoding::BASE32_NOPAD.decode(upper.as_bytes())
            && let Ok(invite) = postcard::from_bytes::<Invite>(&bytes)
            && invite.key().is_some()
        {
            return Some(invite);
        }
    }
    None
}

/// Words anyone can read out, from some bytes: two bytes make one word.
pub fn words_for(bytes: &[u8]) -> Vec<String> {
    let list = bip39::Language::English.word_list();
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| list[usize::from(u16::from_be_bytes(*pair)) % list.len()].to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite() -> Invite {
        Invite {
            person: SecretKey::generate().public().as_bytes().to_owned(),
            name: "Roy".into(),
            drop_secret: random(),
            box_key: random(),
            met_token: None,
        }
    }

    #[test]
    fn links_and_qr_codes_come_back_as_they_went() {
        let i = invite();
        assert_eq!(parse_invite(&i.link()), Some(i.clone()));
        assert_eq!(parse_invite(&i.qr_text()), Some(i.clone()));
        let text = format!("Add me: {} thanks!", i.link());
        assert_eq!(parse_invite(&text), Some(i.clone()));
        assert_eq!(parse_invite("nothing here"), None);
        assert_eq!(parse_invite("whatsmine://add/notbase32!!"), None);
        assert_eq!(i.name(), Some("Roy"));
    }

    #[test]
    fn a_met_token_makes_it_through_a_qr_code() {
        let mut i = invite();
        i.met_token = Some(random());
        assert_eq!(parse_invite(&i.qr_text()).unwrap().met_token, i.met_token);
    }

    #[test]
    fn the_words_bring_back_the_person_and_nothing_else() {
        let first = Identity::from_entropy(random());
        let words = first.recovery_words();
        assert_eq!(words.len(), 12);
        let back = Identity::from_words(&words.join("  ").to_uppercase()).unwrap();
        assert_eq!(back.person().public(), first.person().public());
        assert_ne!(back.network, first.network);
        assert_ne!(back.location, first.location);
        assert!(Identity::from_words("not the right words").is_err());
    }

    #[test]
    fn the_network_key_rotates_when_it_is_time() {
        let mut me = Identity::from_entropy(random());
        let (network, person) = (me.network, me.person().public());
        let since = me.network_since;
        assert!(!me.network_due(since + NETWORK_KEY_LIFE_MS - 1));
        assert_eq!(me.network, network);
        assert!(me.network_due(since + NETWORK_KEY_LIFE_MS));
        assert_ne!(me.network, network);
        assert_eq!(me.person().public(), person);
    }

    #[test]
    fn an_old_location_is_kept_for_a_while_then_goes() {
        let mut me = Identity::from_entropy(random());
        let first = me.location;
        me.rotate_location(1000);
        assert_ne!(me.location, first);
        assert_eq!(me.old_locations, vec![(first, 1000 + OLD_LOCATION_LIFE_MS)]);
        me.rotate_location(1000 + OLD_LOCATION_LIFE_MS + 1);
        assert_eq!(me.old_locations.len(), 1);
        assert_ne!(me.old_locations[0].0, first);
    }

    #[test]
    fn the_identity_survives_the_database() {
        let db = Database::open_in_memory().unwrap();
        let first = Identity::load_or_create(&db).unwrap();
        let again = Identity::load_or_create(&db).unwrap();
        assert_eq!(first.person().public(), again.person().public());
        assert_eq!(first.location, again.location);
    }

    #[test]
    fn handles_are_short_stable_and_show_the_name() {
        let key = SecretKey::generate().public();
        let _ = &key;
        assert_eq!(words_for(&[0, 1, 2, 3]).len(), 2);
    }
}
