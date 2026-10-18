//! Identity: an Ed25519 keypair generated on this device.
//!
//! The public key is the user's address and identity. It is unique by math,
//! so nothing registers it. The secret key never leaves the device.

use std::path::Path;

use iroh::{PublicKey, SecretKey};

pub(crate) const KEY_FILE: &str = "identity.key";

pub fn load_or_create(dir: &Path) -> anyhow::Result<SecretKey> {
    let path = dir.join(KEY_FILE);
    if let Ok(bytes) = std::fs::read(&path) {
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("{} is damaged", path.display()))?;
        return Ok(SecretKey::from_bytes(&bytes));
    }
    let key = SecretKey::generate();
    std::fs::write(&path, key.to_bytes())?;
    Ok(key)
}

/// How a key is shown and shared: z-base-32, 52 lowercase characters chosen
/// to avoid look-alikes.
pub fn key_to_string(key: &PublicKey) -> String {
    key.to_z32()
}

/// Parses a key someone pasted. Accepts z-base-32 or hex, ignores spaces,
/// dashes and a `whatsmine:` prefix.
pub fn parse_key(input: &str) -> Option<PublicKey> {
    let s: String = input
        .trim()
        .trim_start_matches("whatsmine:")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_lowercase();
    if s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) {
        return s.parse().ok();
    }
    PublicKey::from_z32(&s).ok()
}

/// Someone's invitation: their key and, if the link carried it, their name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub key: PublicKey,
    pub name: Option<String>,
}

/// The link that adds you: `whatsmine://add/<key>?n=<name>`. Opening it
/// (or scanning it as a QR code) offers to add you.
pub fn invite_link(key: &str, name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        format!("whatsmine://add/{key}")
    } else {
        format!("whatsmine://add/{key}?n={}", percent_encode(name))
    }
}

/// Finds an invitation in whatever was pasted, scanned or clicked: a link,
/// `whatsmine:<key>`, a bare key, or a message that contains one of those.
pub fn parse_invite(text: &str) -> Option<Invite> {
    let from_token = |token: &str| -> Option<Invite> {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric());
        if let Some(rest) = token
            .strip_prefix("whatsmine://add/")
            .or_else(|| token.strip_prefix("whatsmine:///add/"))
        {
            let (key, query) = rest.split_once('?').unwrap_or((rest, ""));
            let name = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("n="))
                .map(percent_decode)
                .map(|n| n.trim().chars().take(64).collect::<String>())
                .filter(|n| !n.is_empty());
            return Some(Invite {
                key: parse_key(key)?,
                name,
            });
        }
        parse_key(token).map(|key| Invite { key, name: None })
    };
    // A link in a message ends at the first space; a key with spaces in it
    // (as shown on screen) is the whole text.
    text.split_whitespace()
        .find_map(|t| t.starts_with("whatsmine:").then(|| from_token(t)).flatten())
        .or_else(|| from_token(text))
        .or_else(|| text.split_whitespace().find_map(from_token))
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_persists() {
        let dir = tempfile::tempdir().unwrap();
        let a = load_or_create(dir.path()).unwrap();
        let b = load_or_create(dir.path()).unwrap();
        assert_eq!(a.public(), b.public());
    }

    #[test]
    fn invites_round_trip_and_are_found_in_text() {
        let key = SecretKey::generate().public();
        let z = key_to_string(&key);
        let link = invite_link(&z, "Zoë & co");
        assert!(link.starts_with("whatsmine://add/"));
        let got = parse_invite(&link).unwrap();
        assert_eq!(got.key, key);
        assert_eq!(got.name.as_deref(), Some("Zoë & co"));
        // Inside a message, with punctuation around it.
        let msg = format!("hey, add me: {link}.\nthanks!");
        let found = parse_invite(&msg).unwrap();
        assert_eq!((found.key, found.name.as_deref()), (key, Some("Zoë & co")));
        // Older forms still work.
        assert_eq!(parse_invite(&format!("whatsmine:{z}")).unwrap().name, None);
        assert_eq!(parse_invite(&z).unwrap().key, key);
        assert_eq!(parse_invite(&invite_link(&z, "")).unwrap().name, None);
        assert!(parse_invite("whatsmine://add/nonsense").is_none());
        assert!(parse_invite("just words").is_none());
    }

    #[test]
    fn parse_accepts_z32_hex_and_noise() {
        let key = SecretKey::generate().public();
        let z = key_to_string(&key);
        assert_eq!(z.len(), 52);
        assert_eq!(parse_key(&z), Some(key));
        assert_eq!(
            parse_key(&format!("  whatsmine:{}  ", z.to_uppercase())),
            Some(key)
        );
        let spaced: String = z
            .chars()
            .enumerate()
            .flat_map(|(i, c)| {
                if i > 0 && i % 4 == 0 {
                    vec![' ', c]
                } else {
                    vec![c]
                }
            })
            .collect();
        assert_eq!(parse_key(&spaced), Some(key));
        assert_eq!(parse_key(&key.to_string()), Some(key));
        assert_eq!(parse_key("hello"), None);
    }
}
