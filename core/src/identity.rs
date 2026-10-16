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
