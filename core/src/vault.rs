//! What is kept on disk is sealed with one key, and that key is held by the system: Android's
//! Keystore or Windows' DPAPI. A copied folder is then no stolen identity and no readable chats.
//!
//! SQLCipher would seal the whole database file, but building it needs OpenSSL for every target.
//! So the fields that say something are sealed one by one, and what is left in the clear is what
//! makes the database work: ids, times, states.

use std::path::Path;
use std::sync::OnceLock;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

const KEY_FILE: &str = "vault.key";
const NONCE: usize = 24;
const HELD_BY_SYSTEM: u8 = 1;
const IN_THE_FILE: u8 = 0;

/// How the system holds a secret for us: it comes back only on this device, for this app.
#[derive(Clone, Copy)]
pub struct KeyProtector {
    pub wrap: fn(&[u8]) -> Option<Vec<u8>>,
    pub unwrap: fn(&[u8]) -> Option<Vec<u8>>,
}

static PROTECTOR: OnceLock<KeyProtector> = OnceLock::new();

/// Called once, by the app, before the core starts. Without it the key lies in the folder, as in
/// tests and in the test peer.
pub fn set_protector(protector: KeyProtector) {
    let _ = PROTECTOR.set(protector);
}

pub struct Vault {
    key: [u8; 32],
}

/// Random bytes, or nothing works at all.
pub fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    getrandom::fill(&mut out).expect("the system gives random bytes");
    out
}

/// Seals with a key of its own: a fresh nonce, then the ciphertext.
pub fn seal_with(key: &[u8; 32], plain: &[u8], context: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = random::<NONCE>();
    let sealed = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plain,
                aad: context,
            },
        )
        .expect("sealing cannot fail for this size");
    let mut out = Vec::with_capacity(NONCE + sealed.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    out
}

pub fn open_with(key: &[u8; 32], sealed: &[u8], context: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < NONCE {
        return None;
    }
    let (nonce, body) = sealed.split_first_chunk::<NONCE>()?;
    XChaCha20Poly1305::new(key.into())
        .decrypt(
            &XNonce::from(*nonce),
            Payload {
                msg: body,
                aad: context,
            },
        )
        .ok()
}

impl Vault {
    /// The vault of this folder, made the first time. A key the system cannot give back is an
    /// error, not a reason to start over: the data may still come back.
    pub fn open_or_create(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join(KEY_FILE);
        if let Ok(bytes) = std::fs::read(&path) {
            let (&mark, held) = bytes
                .split_first()
                .ok_or_else(|| anyhow::anyhow!("{} is empty", path.display()))?;
            let key = match mark {
                HELD_BY_SYSTEM => {
                    let protector = PROTECTOR
                        .get()
                        .ok_or_else(|| anyhow::anyhow!("no way to read the key on this device"))?;
                    (protector.unwrap)(held)
                        .ok_or_else(|| anyhow::anyhow!("the system does not give the key back"))?
                }
                IN_THE_FILE => held.to_vec(),
                other => anyhow::bail!("{} is of an unknown kind {other}", path.display()),
            };
            let key: [u8; 32] = key
                .try_into()
                .map_err(|_| anyhow::anyhow!("{} is damaged", path.display()))?;
            return Ok(Self { key });
        }
        let key = random::<32>();
        let held = PROTECTOR.get().and_then(|p| (p.wrap)(&key));
        let mut file = Vec::new();
        match held {
            Some(wrapped) => {
                file.push(HELD_BY_SYSTEM);
                file.extend_from_slice(&wrapped);
            }
            None => {
                file.push(IN_THE_FILE);
                file.extend_from_slice(&key);
            }
        }
        // In one go: half a key file would lose everything sealed with it.
        let unfinished = dir.join("vault.key.tmp");
        std::fs::write(&unfinished, file)?;
        std::fs::rename(unfinished, &path)?;
        Ok(Self { key })
    }

    /// A vault for what does not outlive the process.
    pub fn in_memory() -> Self {
        Self { key: random() }
    }

    pub fn seal(&self, plain: &[u8]) -> Vec<u8> {
        seal_with(&self.key, plain, b"whatsmine-vault")
    }

    pub fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        open_with(&self.key, sealed, b"whatsmine-vault")
    }

    pub fn seal_str(&self, plain: &str) -> Vec<u8> {
        self.seal(plain.as_bytes())
    }

    /// What cannot be opened reads as empty: one damaged field is no reason to lose a chat.
    pub fn open_str(&self, sealed: &[u8]) -> String {
        self.open(sealed)
            .and_then(|plain| String::from_utf8(plain).ok())
            .unwrap_or_default()
    }

    /// A key of its own for a purpose, from this one.
    pub fn derive(&self, purpose: &str) -> [u8; 32] {
        blake3::derive_key(&format!("whatsmine vault {purpose}"), &self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_sealed_opens_and_nothing_else_does() {
        let vault = Vault::in_memory();
        let sealed = vault.seal(b"hello");
        assert_eq!(vault.open(&sealed).as_deref(), Some(&b"hello"[..]));
        assert_ne!(vault.seal(b"hello"), sealed, "a fresh nonce each time");
        assert!(!sealed.windows(5).any(|w| w == b"hello"));
        let mut damaged = sealed.clone();
        *damaged.last_mut().unwrap() ^= 1;
        assert_eq!(vault.open(&damaged), None);
        assert_eq!(Vault::in_memory().open(&sealed), None);
        assert_eq!(vault.open(b"short"), None);
    }

    #[test]
    fn the_same_folder_gives_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let sealed = Vault::open_or_create(dir.path()).unwrap().seal(b"later");
        let again = Vault::open_or_create(dir.path()).unwrap();
        assert_eq!(again.open(&sealed).as_deref(), Some(&b"later"[..]));
    }

    #[test]
    fn a_damaged_key_file_is_an_error_not_a_new_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(KEY_FILE), [IN_THE_FILE, 1, 2, 3]).unwrap();
        assert!(Vault::open_or_create(dir.path()).is_err());
        assert_eq!(
            std::fs::read(dir.path().join(KEY_FILE)).unwrap(),
            [IN_THE_FILE, 1, 2, 3]
        );
    }

    #[test]
    fn purposes_get_their_own_keys_and_context_counts() {
        let vault = Vault::in_memory();
        assert_ne!(vault.derive("a"), vault.derive("b"));
        let key = random::<32>();
        let sealed = seal_with(&key, b"x", b"one");
        assert_eq!(open_with(&key, &sealed, b"one").as_deref(), Some(&b"x"[..]));
        assert_eq!(open_with(&key, &sealed, b"two"), None);
    }
}
