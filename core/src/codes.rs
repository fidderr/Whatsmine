//! Short codes that stand for an invitation: a name code like `roy-7kq4x2` to type, and a code like
//! `K7M4-2QX9` to read out on the phone. The invitation lies sealed in the DHT under a key the code
//! makes, so only someone who has the code can find it, and the network never sees the code.
//!
//! Anyone holding a code could also write under its key. So a name code is checked against the
//! person it claims to find: its letters come from their key and their current invitation. A code
//! to read out cannot be checked that way, but it is random, lasts minutes and works once.

use crate::identity::Invite;
use crate::vault::{open_with, seal_with};

const CONTEXT: &[u8] = b"whatsmine-code-v1";
/// Letters nobody mixes up, also over the phone.
const LETTERS: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
/// How long a code to read out works.
pub const READ_OUT_MS: i64 = 10 * 60 * 1000;

/// The part of a code that counts: without the name in front, dashes, spaces or capitals.
pub fn normalize(code: &str) -> String {
    let lower = code.trim().to_lowercase();
    let last = lower
        .rsplit_once('-')
        .map_or(lower.as_str(), |(first, last)| {
            // A code to read out has its dash in the middle and letters on both sides.
            if first.len() == 4 && last.len() == 4 {
                ""
            } else {
                last
            }
        });
    let chosen = if last.is_empty() {
        lower.as_str()
    } else {
        last
    };
    chosen
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// Whether this could be a code at all, and not a mistyped link.
pub fn looks_like_code(text: &str) -> bool {
    let code = normalize(text);
    (code.len() == 6 || code.len() == 8) && code.bytes().all(|b| LETTERS.contains(&b))
}

fn derive(purpose: &str, code: &str) -> [u8; 32] {
    blake3::derive_key(purpose, code.as_bytes())
}

/// Where the invitation of this code lies.
pub fn seed(code: &str) -> [u8; 32] {
    derive("whatsmine code seed v1", &normalize(code))
}

pub fn seal(code: &str, invite: &Invite) -> Vec<u8> {
    let plain = postcard::to_stdvec(invite).expect("an invite serializes");
    seal_with(
        &derive("whatsmine code seal v1", &normalize(code)),
        &plain,
        CONTEXT,
    )
}

/// The invitation, if it opens and, for a name code, is really theirs.
pub fn open(code: &str, sealed: &[u8]) -> Option<Invite> {
    let normal = normalize(code);
    let plain = open_with(&derive("whatsmine code seal v1", &normal), sealed, CONTEXT)?;
    let invite: Invite = postcard::from_bytes(&plain).ok()?;
    invite.key()?;
    if normal.len() == 6 && name_code(&invite) != normal {
        return None;
    }
    Some(invite)
}

/// The letters of a name code: they change when the invitation does, so a reset ends the old code.
pub fn name_code(invite: &Invite) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("whatsmine handle v2");
    hasher.update(&invite.person);
    hasher.update(&invite.drop_secret);
    let hash = hasher.finalize();
    hash.as_bytes()[..6]
        .iter()
        .map(|b| LETTERS[*b as usize % LETTERS.len()] as char)
        .collect()
}

/// How the name code is written: the name, a dash, the letters.
pub fn name_code_text(invite: &Invite) -> String {
    let code = name_code(invite);
    let first: String = invite
        .name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect::<String>()
        .to_lowercase();
    if first.is_empty() {
        code
    } else {
        format!("{first}-{code}")
    }
}

/// A new code to read out, like `K7M4-2QX9`.
pub fn new_read_out() -> String {
    let bytes: [u8; 8] = crate::vault::random();
    let letters: Vec<char> = bytes
        .iter()
        .map(|b| (LETTERS[*b as usize % LETTERS.len()] as char).to_ascii_uppercase())
        .collect();
    format!(
        "{}-{}",
        letters[..4].iter().collect::<String>(),
        letters[4..].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite(name: &str, drop: u8) -> Invite {
        let key = iroh::SecretKey::generate().public();
        Invite {
            person: *key.as_bytes(),
            name: name.into(),
            drop_secret: [drop; 32],
            box_key: [2; 32],
            met_token: None,
        }
    }

    #[test]
    fn a_code_is_the_same_however_it_is_typed() {
        assert_eq!(normalize("Roy-7KQ4X2"), "7kq4x2");
        assert_eq!(normalize(" 7kq4x2 "), "7kq4x2");
        assert_eq!(normalize("K7M4-2QX9"), "k7m42qx9");
        assert_eq!(normalize("k7m4 2qx9"), "k7m42qx9");
        assert_eq!(normalize("anna-maria-7kq4x2"), "7kq4x2");
        assert!(looks_like_code("roy-7kq4x2"));
        assert!(looks_like_code("K7M4-2QX9"));
        assert!(!looks_like_code("hello there"));
        assert!(!looks_like_code("https://example.com"));
    }

    #[test]
    fn a_name_code_finds_only_who_it_was_made_from() {
        let real = invite("Roy", 1);
        let code = name_code_text(&real);
        assert!(code.starts_with("roy-"));
        assert_eq!(open(&code, &seal(&code, &real)).unwrap(), real);
        // Someone else writing under the same code, sealed correctly: refused.
        let other = invite("Roy", 1);
        assert!(open(&code, &seal(&code, &other)).is_none());
    }

    #[test]
    fn a_reset_gives_another_code() {
        let before = invite("Roy", 1);
        let mut after = before.clone();
        after.drop_secret = [9; 32];
        assert_ne!(name_code(&before), name_code(&after));
    }

    #[test]
    fn a_code_to_read_out_opens_with_only_that_code() {
        let code = new_read_out();
        assert!(looks_like_code(&code));
        let real = invite("Roy", 1);
        let sealed = seal(&code, &real);
        assert_eq!(open(&code, &sealed).unwrap(), real);
        assert!(open("AAAA-BBBB", &sealed).is_none());
    }
}
