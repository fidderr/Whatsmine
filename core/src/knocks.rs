//! A knock: a small sealed note in the DHT for someone whose phone is resting. "I have something
//! for you", and a short text if it fits. It lies under a key only contacts can derive, sealed,
//! and signed by whoever knocked. The other side reads these at its next check, so both phones can
//! be asleep on mobile data and a short message still gets through.

use iroh::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::model::Millis;
use crate::records::{self, MAX_RECORD};
use crate::vault::{open_with, seal_with};

const PURPOSE: &str = "knock";
const CONTEXT: &[u8] = b"whatsmine-knock-v1";
/// Slots per day per person. Who knocks tries another one each time, so two rarely collide.
pub const SLOTS: u8 = 8;
/// What can come along: a little under what a record holds, with everything around it.
pub const MAX_SEALED_TEXT: usize = 620;

/// A short text, sealed end to end inside the knock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnockText {
    pub id: [u8; 16],
    pub sent_at: Millis,
    pub kind: u8,
    pub sealed: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Knock {
    pub from: [u8; 32],
    pub text: Option<KnockText>,
    /// Messages of theirs that got here, for when no connection can tell them.
    pub delivered: Vec<[u8; 16]>,
    pub at: Millis,
}

#[derive(Serialize, Deserialize)]
struct Signed {
    knock: Knock,
    sig: Vec<u8>,
}

fn seal_key(secret: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("whatsmine knock seal v1", secret)
}

fn signing_bytes(knock: &Knock) -> Vec<u8> {
    let mut out = CONTEXT.to_vec();
    out.extend(postcard::to_stdvec(knock).unwrap_or_default());
    out
}

/// Sealed for whoever holds this location secret. `None` if it would not fit in a record.
pub fn seal(secret: &[u8; 32], knock: Knock, person: &SecretKey) -> Option<Vec<u8>> {
    let sig = person.sign(&signing_bytes(&knock)).to_bytes().to_vec();
    let bytes = postcard::to_stdvec(&Signed { knock, sig }).ok()?;
    let sealed = seal_with(&seal_key(secret), &bytes, CONTEXT);
    (sealed.len() <= MAX_RECORD).then_some(sealed)
}

/// Reads one, if it is one, and the one it says it is from really wrote it.
pub fn open(secret: &[u8; 32], sealed: &[u8]) -> Option<Knock> {
    let bytes = open_with(&seal_key(secret), sealed, CONTEXT)?;
    let signed: Signed = postcard::from_bytes(&bytes).ok()?;
    let sender = PublicKey::from_bytes(&signed.knock.from).ok()?;
    let sig: [u8; 64] = signed.sig.as_slice().try_into().ok()?;
    sender
        .verify(&signing_bytes(&signed.knock), &Signature::from_bytes(&sig))
        .ok()?;
    Some(signed.knock)
}

pub fn slot_seed(secret: &[u8; 32], day: u64, slot: u8) -> [u8; 32] {
    records::seed(PURPOSE, secret, day, slot)
}

/// Where this person's knock goes on this try.
pub fn slot_for(from: &PublicKey, attempt: u32) -> u8 {
    let start = blake3::derive_key("whatsmine knock slot v1", from.as_bytes())[0];
    ((u32::from(start) + attempt) % u32::from(SLOTS)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::random;

    fn knock(from: &SecretKey, text: Option<KnockText>) -> Knock {
        Knock {
            from: *from.public().as_bytes(),
            text,
            delivered: vec![[1; 16]],
            at: 5,
        }
    }

    #[test]
    fn only_the_secret_opens_it() {
        let (me, secret) = (SecretKey::generate(), random());
        let k = knock(&me, None);
        let sealed = seal(&secret, k.clone(), &me).unwrap();
        assert_eq!(open(&secret, &sealed), Some(k));
        assert_eq!(open(&random(), &sealed), None);
        assert_eq!(open(&secret, b"nonsense"), None);
    }

    #[test]
    fn nobody_knocks_in_someone_elses_name() {
        let (me, other, secret) = (SecretKey::generate(), SecretKey::generate(), random());
        let forged = seal(&secret, knock(&me, None), &other).unwrap();
        assert_eq!(open(&secret, &forged), None);
    }

    #[test]
    fn the_largest_text_fits_and_a_larger_one_does_not_pretend_to() {
        let (me, secret) = (SecretKey::generate(), random());
        let text = |len| KnockText {
            id: [2; 16],
            sent_at: 1,
            kind: 1,
            sealed: vec![7; len],
        };
        assert!(seal(&secret, knock(&me, Some(text(MAX_SEALED_TEXT))), &me).is_some());
        assert!(seal(&secret, knock(&me, Some(text(MAX_RECORD))), &me).is_none());
    }

    #[test]
    fn another_try_takes_another_slot() {
        let me = SecretKey::generate().public();
        let slots: std::collections::HashSet<u8> = (0..8).map(|n| slot_for(&me, n)).collect();
        assert_eq!(slots.len(), usize::from(SLOTS));
    }
}
