//! End-to-end encryption between two people: Olm, through `vodozemac`. The connection is encrypted
//! too, but this is what lets a message be held by another device, or lie in the DHT, without
//! anyone being able to read it.
//!
//! Every session state goes to disk before what it sealed goes out, so a kill at any moment cannot
//! make the two sides disagree about the ratchet.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use vodozemac::Curve25519PublicKey;
use vodozemac::olm::{Account, AccountPickle, OlmMessage, Session, SessionConfig, SessionPickle};

use crate::db::Database;

const ACCOUNT_SECRET: &str = "olm_account";

/// The small-order points of Curve25519. A key that is one of these makes a shared secret anyone
/// can compute: vodozemac refuses them for its own part, and so do we.
const WEAK_KEYS: [[u8; 32]; 7] = [
    [0; 32],
    {
        let mut k = [0u8; 32];
        k[0] = 1;
        k
    },
    [
        0xe0, 0xeb, 0x7a, 0x7c, 0x3b, 0x41, 0xb8, 0xae, 0x16, 0x56, 0xe3, 0xfa, 0xf1, 0x9f, 0xc4,
        0x6a, 0xda, 0x09, 0x8d, 0xeb, 0x9c, 0x32, 0xb1, 0xfd, 0x86, 0x62, 0x05, 0x16, 0x5f, 0x49,
        0xb8, 0x00,
    ],
    [
        0x5f, 0x9c, 0x95, 0xbc, 0xa3, 0x50, 0x8c, 0x24, 0xb1, 0xd0, 0xb1, 0x55, 0x9c, 0x83, 0xef,
        0x5b, 0x04, 0x44, 0x5c, 0xc4, 0x58, 0x1c, 0x8e, 0x86, 0xd8, 0x22, 0x4e, 0xdd, 0xd0, 0x9f,
        0x11, 0x57,
    ],
    [
        0xec, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x7f,
    ],
    [
        0xed, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x7f,
    ],
    [
        0xee, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0x7f,
    ],
];

/// Refuses a key nobody honest would send.
pub fn usable_key(bytes: &[u8; 32]) -> Option<Curve25519PublicKey> {
    if WEAK_KEYS.contains(bytes) {
        return None;
    }
    Some(Curve25519PublicKey::from_bytes(*bytes))
}

/// What was opened, or why not.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Not a starting message, and there is no session to read it with.
    NoSession,
    /// A session exists, and this does not fit it.
    DoesNotFit,
    /// A key nobody honest would send, or a message that cannot be a message.
    Invalid,
}

pub struct Sealing {
    db: Arc<Database>,
    pickle_key: [u8; 32],
    account: RefCell<Account>,
    sessions: RefCell<HashMap<String, Session>>,
}

impl Sealing {
    pub fn load(db: Arc<Database>) -> anyhow::Result<Self> {
        let pickle_key = db.pickle_key();
        let account = match db.secret(ACCOUNT_SECRET) {
            Some(bytes) => {
                let text = String::from_utf8(bytes)?;
                Account::from_pickle(AccountPickle::from_encrypted(&text, &pickle_key)?)
            }
            None => Account::new(),
        };
        let sealing = Self {
            db,
            pickle_key,
            account: RefCell::new(account),
            sessions: RefCell::new(HashMap::new()),
        };
        sealing.save_account();
        Ok(sealing)
    }

    fn save_account(&self) {
        let text = self.account.borrow().pickle().encrypt(&self.pickle_key);
        if let Err(e) = self.db.set_secret(ACCOUNT_SECRET, text.as_bytes()) {
            tracing::warn!("could not save the encryption account: {e}");
        }
    }

    fn save_session(&self, person: &str, session: &Session) {
        let text = session.pickle().encrypt(&self.pickle_key);
        self.db.set_olm_session(person, text.as_bytes());
    }

    fn with_session<T>(&self, person: &str, f: impl FnOnce(&mut Session) -> T) -> Option<T> {
        let mut sessions = self.sessions.borrow_mut();
        if !sessions.contains_key(person) {
            let bytes = self.db.olm_session(person)?;
            let text = String::from_utf8(bytes).ok()?;
            let pickle = SessionPickle::from_encrypted(&text, &self.pickle_key).ok()?;
            sessions.insert(person.to_owned(), Session::from_pickle(pickle));
        }
        sessions.get_mut(person).map(f)
    }

    /// What someone needs to start a session with us: a long-lived key and a fresh one-time key.
    pub fn keys_to_give(&self) -> ([u8; 32], [u8; 32]) {
        let mut account = self.account.borrow_mut();
        let created = account.generate_one_time_keys(1);
        let one_time = created
            .created
            .first()
            .copied()
            .expect("one key was asked for");
        account.mark_keys_as_published();
        let curve = account.curve25519_key().to_bytes();
        drop(account);
        self.save_account();
        (curve, one_time.to_bytes())
    }

    /// Our long-lived key on this device.
    pub fn my_curve(&self) -> [u8; 32] {
        self.account.borrow().curve25519_key().to_bytes()
    }

    pub fn has_session(&self, person: &str) -> bool {
        self.with_session(person, |_| ()).is_some()
    }

    /// Starts a session from the keys they gave. Replaces one that has stopped working.
    pub fn start_session(&self, person: &str, curve: &[u8; 32], one_time: &[u8; 32]) -> bool {
        let (Some(curve), Some(one_time)) = (usable_key(curve), usable_key(one_time)) else {
            tracing::warn!("{person} sent a key that cannot be used");
            return false;
        };
        let created = self.account.borrow().create_outbound_session(
            SessionConfig::version_1(),
            curve,
            one_time,
        );
        let session = match created {
            Ok(session) => session,
            Err(e) => {
                tracing::warn!("no session with {person}: {e}");
                return false;
            }
        };
        self.save_session(person, &session);
        self.sessions
            .borrow_mut()
            .insert(person.to_owned(), session);
        true
    }

    pub fn forget_session(&self, person: &str) {
        self.sessions.borrow_mut().remove(person);
        self.db.drop_olm_session(person);
    }

    /// Seals for this person, if there is a session.
    pub fn seal(&self, person: &str, plain: &[u8]) -> Option<(u8, Vec<u8>)> {
        let message = self
            .with_session(person, |session| session.encrypt(plain))?
            .ok()?;
        self.with_session(person, |session| self.save_session(person, session));
        let (kind, bytes) = message.to_parts();
        Some((kind as u8, bytes))
    }

    /// Opens what this person sealed. A starting message can make a session; `curve` is their
    /// long-lived key, proven at the handshake.
    pub fn open(
        &self,
        person: &str,
        curve: &[u8; 32],
        kind: u8,
        bytes: &[u8],
    ) -> Result<Vec<u8>, OpenError> {
        let message =
            OlmMessage::from_parts(kind as usize, bytes).map_err(|_| OpenError::Invalid)?;
        let known = self.with_session(person, |session| session.decrypt(&message));
        match (known, &message) {
            (Some(Ok(plain)), _) => {
                self.with_session(person, |session| self.save_session(person, session));
                Ok(plain)
            }
            (known, OlmMessage::PreKey(prekey)) => {
                let curve = usable_key(curve).ok_or(OpenError::Invalid)?;
                let created = self.account.borrow_mut().create_inbound_session(
                    SessionConfig::version_1(),
                    curve,
                    prekey,
                );
                match created {
                    Ok(result) => {
                        self.save_account();
                        self.save_session(person, &result.session);
                        self.sessions
                            .borrow_mut()
                            .insert(person.to_owned(), result.session);
                        Ok(result.plaintext)
                    }
                    Err(e) => {
                        tracing::debug!("no session from {person}'s message: {e}");
                        if known.is_some() {
                            Err(OpenError::DoesNotFit)
                        } else {
                            Err(OpenError::NoSession)
                        }
                    }
                }
            }
            (Some(Err(_)), _) => Err(OpenError::DoesNotFit),
            (None, _) => Err(OpenError::NoSession),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party() -> (Sealing, Arc<Database>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        (Sealing::load(db.clone()).unwrap(), db)
    }

    fn pair() -> (Sealing, Sealing, [u8; 32], [u8; 32]) {
        let (a, _) = party();
        let (b, _) = party();
        let (a_curve, _) = a.keys_to_give();
        let (b_curve, b_one_time) = b.keys_to_give();
        assert!(a.start_session("b", &b_curve, &b_one_time));
        (a, b, a_curve, b_curve)
    }

    #[test]
    fn a_session_works_both_ways_after_the_first_message() {
        let (a, b, a_curve, b_curve) = pair();
        let (kind, first) = a.seal("b", b"hello").unwrap();
        assert_eq!(b.open("a", &a_curve, kind, &first).unwrap(), b"hello");
        let (kind, reply) = b.seal("a", b"hi").unwrap();
        assert_eq!(a.open("b", &b_curve, kind, &reply).unwrap(), b"hi");
        let (kind, again) = a.seal("b", b"again").unwrap();
        assert_eq!(b.open("a", &a_curve, kind, &again).unwrap(), b"again");
    }

    #[test]
    fn what_is_sealed_does_not_show_and_cannot_be_opened_by_others() {
        let (a, b, a_curve, _) = pair();
        let (kind, sealed) = a.seal("b", b"secret text").unwrap();
        assert!(!sealed.windows(6).any(|w| w == b"secret"));
        let (other, _) = party();
        assert!(other.open("a", &a_curve, kind, &sealed).is_err());
        assert!(b.open("a", &a_curve, kind, &sealed).is_ok());
    }

    #[test]
    fn a_session_survives_a_restart() {
        let (a, _) = party();
        let (b, b_db) = party();
        let (a_curve, _) = a.keys_to_give();
        let (b_curve, b_one_time) = b.keys_to_give();
        a.start_session("b", &b_curve, &b_one_time);
        let (kind, first) = a.seal("b", b"one").unwrap();
        b.open("a", &a_curve, kind, &first).unwrap();
        drop(b);
        let b = Sealing::load(b_db).unwrap();
        assert!(b.has_session("a"));
        let (kind, second) = a.seal("b", b"two").unwrap();
        assert_eq!(b.open("a", &a_curve, kind, &second).unwrap(), b"two");
    }

    #[test]
    fn the_same_message_twice_does_not_open_twice() {
        let (a, b, a_curve, _) = pair();
        let (kind, first) = a.seal("b", b"one").unwrap();
        b.open("a", &a_curve, kind, &first).unwrap();
        assert!(b.seal("a", b"reply").is_some());
        let (kind, second) = a.seal("b", b"two").unwrap();
        b.open("a", &a_curve, kind, &second).unwrap();
        assert!(b.open("a", &a_curve, kind, &second).is_err());
    }

    #[test]
    fn keys_nobody_honest_would_send_are_refused() {
        let (a, _) = party();
        let (curve, one_time) = party().0.keys_to_give();
        for weak in WEAK_KEYS {
            assert!(usable_key(&weak).is_none());
            assert!(!a.start_session("x", &weak, &one_time));
            assert!(!a.start_session("x", &curve, &weak));
        }
        assert!(a.start_session("x", &curve, &one_time));
    }

    #[test]
    fn nothing_to_read_with_says_so() {
        let (a, _) = party();
        let (b, _) = party();
        let (a_curve, _) = a.keys_to_give();
        let (b_curve, b_one_time) = b.keys_to_give();
        a.start_session("b", &b_curve, &b_one_time);
        // A normal message from a session the other side never saw: first a reply on a session.
        let (kind, first) = a.seal("b", b"one").unwrap();
        assert_eq!(kind, 0, "a first message starts the session");
        b.open("a", &a_curve, kind, &first).unwrap();
        let (kind, reply) = b.seal("a", b"two").unwrap();
        a.open("b", &b_curve, kind, &reply).unwrap();
        let (kind, normal) = a.seal("b", b"three").unwrap();
        assert_eq!(kind, 1);
        let (fresh, _) = party();
        assert_eq!(
            fresh.open("a", &a_curve, kind, &normal),
            Err(OpenError::NoSession)
        );
        assert_eq!(
            fresh.open("a", &a_curve, 9, &normal),
            Err(OpenError::Invalid)
        );
    }
}
