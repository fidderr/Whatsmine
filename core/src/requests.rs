//! Asking to be added. A stranger with your link only leaves a sealed note in the DHT, in one of a
//! few slots under a key only the link knows. They never reach your phone and never learn where it
//! is, and a flood of them costs you nothing: you read those slots when you look, and a reset of
//! your address makes all of it go away.

use iroh::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey as BoxPublic, StaticSecret};

use crate::identity::Invite;
use crate::model::{Millis, MyInvite, now_ms};
use crate::records;
use crate::vault::{open_with, random, seal_with};

const PURPOSE: &str = "request";
const CONTEXT: &[u8] = b"whatsmine-request-v1";
/// Slots per day per invite. Two strangers rarely take the same one, and a flood can only fill
/// these few.
pub const SLOTS: u8 = 8;
pub const MAX_GREETING: usize = 140;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub person: [u8; 32],
    pub name: String,
    pub greeting: String,
    /// How to find whoever asks, once accepted.
    pub location: [u8; 32],
    pub met_token: Option<[u8; 16]>,
    pub at: Millis,
    /// Not to be added as a contact, but to a group.
    pub join: Option<JoinAsk>,
}

/// Asking to join a group, with the token from its link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinAsk {
    pub group: String,
    pub token: [u8; 16],
}

#[derive(Serialize, Deserialize)]
struct Signed {
    request: Request,
    sig: Vec<u8>,
}

fn signing_bytes(request: &Request, box_key: &[u8; 32]) -> Vec<u8> {
    let mut out = CONTEXT.to_vec();
    out.extend_from_slice(box_key);
    out.extend(postcard::to_stdvec(request).unwrap_or_default());
    out
}

fn shared_key(
    shared: &x25519_dalek::SharedSecret,
    ephemeral: &[u8; 32],
    box_key: &[u8; 32],
) -> Option<[u8; 32]> {
    if !shared.was_contributory() {
        return None;
    }
    let mut input = shared.as_bytes().to_vec();
    input.extend_from_slice(ephemeral);
    input.extend_from_slice(box_key);
    Some(blake3::derive_key("whatsmine request box v1", &input))
}

/// The public key requests are sealed to, from the secret we keep.
pub fn box_public(secret: &[u8; 32]) -> [u8; 32] {
    BoxPublic::from(&StaticSecret::from(*secret)).to_bytes()
}

/// A sealed note for this invite's owner: only they can read it, and only `person` could sign it.
pub fn seal(invite: &Invite, request: &Request, person: &SecretKey) -> Option<Vec<u8>> {
    let sig = person
        .sign(&signing_bytes(request, &invite.box_key))
        .to_bytes()
        .to_vec();
    let bytes = postcard::to_stdvec(&Signed {
        request: request.clone(),
        sig,
    })
    .ok()?;
    let ephemeral = StaticSecret::from(random::<32>());
    let public = BoxPublic::from(&ephemeral).to_bytes();
    let shared = ephemeral.diffie_hellman(&BoxPublic::from(invite.box_key));
    let key = shared_key(&shared, &public, &invite.box_key)?;
    let mut out = public.to_vec();
    out.extend(seal_with(&key, &bytes, CONTEXT));
    Some(out)
}

/// Reads a note left for this invite. Anything wrong with it simply is no request.
pub fn open(invite: &MyInvite, sealed: &[u8]) -> Option<Request> {
    let (public, body) = sealed.split_first_chunk::<32>()?;
    let secret = StaticSecret::from(invite.box_secret);
    let box_key = box_public(&invite.box_secret);
    let shared = secret.diffie_hellman(&BoxPublic::from(*public));
    let key = shared_key(&shared, public, &box_key)?;
    let bytes = open_with(&key, body, CONTEXT)?;
    let signed: Signed = postcard::from_bytes(&bytes).ok()?;
    let sender = PublicKey::from_bytes(&signed.request.person).ok()?;
    let sig: [u8; 64] = signed.sig.as_slice().try_into().ok()?;
    sender
        .verify(
            &signing_bytes(&signed.request, &box_key),
            &Signature::from_bytes(&sig),
        )
        .ok()?;
    if signed.request.greeting.len() > MAX_GREETING * 4 || signed.request.name.len() > 256 {
        return None;
    }
    Some(signed.request)
}

/// Where a note goes.
pub fn slot_seed(drop_secret: &[u8; 32], day: u64, slot: u8) -> [u8; 32] {
    records::seed(PURPOSE, drop_secret, day, slot)
}

/// A slot for this note, not the same one each try.
pub fn random_slot() -> u8 {
    random::<1>()[0] % SLOTS
}

impl MyInvite {
    pub fn new(label: &str, expires_at: Option<Millis>, max_uses: Option<u32>, main: bool) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            label: label.to_owned(),
            drop_secret: random(),
            box_secret: random(),
            expires_at,
            max_uses,
            uses: 0,
            created_at: now_ms(),
            revoked: false,
            main,
            met_token: None,
        }
    }

    pub fn open_for_requests(&self, now: Millis) -> bool {
        !self.revoked
            && self.expires_at.is_none_or(|at| at > now)
            && self.max_uses.is_none_or(|max| self.uses < max)
    }

    /// What goes in a link or a code.
    pub fn to_invite(&self, person: &PublicKey, name: &str) -> Invite {
        Invite {
            person: *person.as_bytes(),
            name: name.to_owned(),
            drop_secret: self.drop_secret,
            box_key: box_public(&self.box_secret),
            met_token: self.met_token,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::MAX_RECORD;

    fn setup() -> (MyInvite, Invite, SecretKey) {
        let mine = MyInvite::new("Main", None, None, true);
        let owner = SecretKey::generate();
        let invite = mine.to_invite(&owner.public(), "Roy");
        (mine, invite, SecretKey::generate())
    }

    fn request(person: &SecretKey, greeting: &str) -> Request {
        Request {
            person: *person.public().as_bytes(),
            name: "Sam".into(),
            greeting: greeting.into(),
            location: random(),
            met_token: None,
            at: 1,
            join: None,
        }
    }

    #[test]
    fn only_the_owner_reads_a_request() {
        let (mine, invite, sam) = setup();
        let r = request(&sam, "Hi, it's Sam from climbing");
        let sealed = seal(&invite, &r, &sam).unwrap();
        assert_eq!(open(&mine, &sealed), Some(r));
        assert_eq!(
            open(&MyInvite::new("Other", None, None, false), &sealed),
            None
        );
        assert!(!sealed.windows(3).any(|w| w == b"Sam"));
    }

    #[test]
    fn nobody_can_ask_in_someone_elses_name() {
        let (mine, invite, sam) = setup();
        let mallory = SecretKey::generate();
        let forged = seal(&invite, &request(&sam, "Hi"), &mallory).unwrap();
        assert_eq!(open(&mine, &forged), None);
        assert_eq!(open(&mine, b"short"), None);
        assert_eq!(open(&mine, &[0; 200]), None);
    }

    #[test]
    fn the_largest_request_fits_in_the_dht() {
        let (_, invite, sam) = setup();
        let mut r = request(&sam, &"x".repeat(MAX_GREETING));
        r.name = "y".repeat(64);
        r.met_token = Some(random());
        let sealed = seal(&invite, &r, &sam).unwrap();
        assert!(sealed.len() <= MAX_RECORD, "{} bytes", sealed.len());
    }

    #[test]
    fn an_invite_closes_when_it_is_revoked_expired_or_used_up() {
        let mut once = MyInvite::new("Anna", None, Some(1), false);
        assert!(once.open_for_requests(5));
        once.uses = 1;
        assert!(!once.open_for_requests(5));
        let soon = MyInvite::new("Club", Some(10), None, false);
        assert!(soon.open_for_requests(9));
        assert!(!soon.open_for_requests(10));
        let mut main = MyInvite::new("Main", None, None, true);
        main.revoked = true;
        assert!(!main.open_for_requests(5));
    }
}
