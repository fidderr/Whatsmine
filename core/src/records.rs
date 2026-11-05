//! Small sealed records in the BitTorrent DHT: where someone is, and requests left for them.
//! Nobody owns that network and nothing has to be set up. Whoever knows a secret can read and
//! write its records, so what is in them is sealed and, where it matters, signed.
//!
//! Measured on the real network (2026-11): 980 bytes always goes, 1000 never does, because nodes
//! count a value's encoded form. Writes took 2-7 s, reads tens of milliseconds, 100% of both.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_lite::StreamExt;
use n0_mainline::{Dht, MutableItem, SigningKey};

use crate::model::now_ms;

/// The most a record can hold. A little under what always went on the real network.
pub const MAX_RECORD: usize = 960;
/// After a first copy, more may come with a newer version: this long is worth waiting.
const NEWER_WINDOW: Duration = Duration::from_millis(400);
const READ_LIMIT: Duration = Duration::from_secs(8);

/// A DHT that lives in this process, so tests need no network.
#[derive(Clone, Debug, Default)]
pub struct LocalRecords(Arc<Mutex<LocalMap>>, Arc<Mutex<SignedMap>>);

type LocalMap = HashMap<[u8; 32], (i64, Vec<u8>)>;
type SignedMap = HashMap<([u8; 32], Option<Vec<u8>>), SignedRecord>;

impl LocalRecords {
    /// What the real one does after a couple of hours when nobody writes things again. For tests.
    #[doc(hidden)]
    pub fn forget_everything(&self) {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clear();
        self.1.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

/// A record signed by whoever owns its key. Anyone who has it can write it again, and nobody without
/// that key can change a thing about it: that is what keeps a channel alive with its author away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedRecord {
    pub key: [u8; 32],
    pub value: Vec<u8>,
    pub seq: i64,
    pub signature: [u8; 64],
    pub salt: Option<Vec<u8>>,
}

impl SignedRecord {
    pub fn sign(secret: &[u8; 32], value: &[u8], seq: i64, salt: Option<&[u8]>) -> Self {
        let signer = SigningKey::from_bytes(secret);
        let item = MutableItem::new(&signer, value, seq, salt);
        Self::from_item(&item)
    }

    fn from_item(item: &MutableItem) -> Self {
        Self {
            key: *item.key(),
            value: item.value().to_vec(),
            seq: item.seq(),
            signature: *item.signature(),
            salt: item.salt().map(<[u8]>::to_vec),
        }
    }

    fn to_item(&self) -> MutableItem {
        MutableItem::new_signed_unchecked(
            self.key,
            self.signature,
            &self.value,
            self.seq,
            self.salt.as_deref(),
        )
    }

    /// As it is stored here, to write it again later.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(112 + self.value.len());
        out.extend_from_slice(&self.key);
        out.extend_from_slice(&self.signature);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&(self.salt.as_ref().map_or(0, Vec::len) as u32).to_be_bytes());
        if let Some(salt) = &self.salt {
            out.extend_from_slice(salt);
        }
        out.extend_from_slice(&self.value);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let key: [u8; 32] = bytes.get(..32)?.try_into().ok()?;
        let signature: [u8; 64] = bytes.get(32..96)?.try_into().ok()?;
        let seq = i64::from_be_bytes(bytes.get(96..104)?.try_into().ok()?);
        let salt_len = u32::from_be_bytes(bytes.get(104..108)?.try_into().ok()?) as usize;
        let value_at = 108usize.checked_add(salt_len)?;
        let salt = bytes.get(108..value_at)?;
        Some(Self {
            key,
            signature,
            seq,
            salt: (salt_len > 0).then(|| salt.to_vec()),
            value: bytes.get(value_at..)?.to_vec(),
        })
    }
}

#[derive(Clone)]
pub enum Records {
    Dht(Dht),
    Local(LocalRecords),
}

fn public_key(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

impl Records {
    /// Joins the DHT. It is kept only while there is an endpoint: a client keeps its routing table
    /// fresh, and that is radio traffic.
    pub fn join() -> anyhow::Result<Self> {
        Ok(Self::Dht(Dht::client()?))
    }

    /// Writes under the key `seed` makes. False if nothing took it: try again later.
    pub async fn put(&self, seed: &[u8; 32], value: &[u8]) -> bool {
        if value.len() > MAX_RECORD {
            tracing::warn!("a record of {} bytes does not fit", value.len());
            return false;
        }
        let version = now_ms();
        match self {
            Self::Dht(dht) => {
                let signer = SigningKey::from_bytes(seed);
                let item = MutableItem::new(&signer, value, version, None);
                match dht.put_mutable(item, None).await {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::debug!("a DHT write failed: {e}");
                        false
                    }
                }
            }
            Self::Local(local) => {
                let mut map = local.0.lock().unwrap_or_else(|p| p.into_inner());
                map.insert(public_key(seed), (version, value.to_vec()));
                true
            }
        }
    }

    /// Writes a signed record, ours or someone else's. False if nothing took it.
    pub async fn put_signed(&self, record: &SignedRecord) -> bool {
        if record.value.len() > MAX_RECORD {
            return false;
        }
        match self {
            Self::Dht(dht) => match dht.put_mutable(record.to_item(), None).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::debug!("a DHT write failed: {e}");
                    false
                }
            },
            Self::Local(local) => {
                let mut map = local.1.lock().unwrap_or_else(|p| p.into_inner());
                let at = (record.key, record.salt.clone());
                // As real nodes do: never back to something older.
                if map.get(&at).is_none_or(|have| have.seq <= record.seq) {
                    map.insert(at, record.clone());
                }
                true
            }
        }
    }

    /// The newest copy of a signed record, with what it takes to write it again.
    pub async fn get_signed(&self, key: &[u8; 32], salt: Option<&[u8]>) -> Option<SignedRecord> {
        match self {
            Self::Dht(dht) => {
                let mut stream = dht.get_mutable(key, salt, None).await.ok()?;
                let first = tokio::time::timeout(READ_LIMIT, stream.next())
                    .await
                    .ok()??;
                let mut newest = first;
                let more = async {
                    while let Some(item) = stream.next().await {
                        if item.seq() > newest.seq() {
                            newest = item;
                        }
                    }
                };
                let _ = tokio::time::timeout(NEWER_WINDOW, more).await;
                Some(SignedRecord::from_item(&newest))
            }
            Self::Local(local) => {
                let map = local.1.lock().unwrap_or_else(|p| p.into_inner());
                map.get(&(*key, salt.map(<[u8]>::to_vec))).cloned()
            }
        }
    }

    /// The newest copy of what is under the key `seed` makes.
    pub async fn get(&self, seed: &[u8; 32]) -> Option<Vec<u8>> {
        let key = public_key(seed);
        match self {
            Self::Dht(dht) => {
                let mut stream = dht.get_mutable(&key, None, None).await.ok()?;
                let first = tokio::time::timeout(READ_LIMIT, stream.next())
                    .await
                    .ok()??;
                let mut newest = (first.seq(), first.value().to_vec());
                let more = async {
                    while let Some(item) = stream.next().await {
                        if item.seq() > newest.0 {
                            newest = (item.seq(), item.value().to_vec());
                        }
                    }
                };
                let _ = tokio::time::timeout(NEWER_WINDOW, more).await;
                Some(newest.1)
            }
            Self::Local(local) => {
                let map = local.0.lock().unwrap_or_else(|p| p.into_inner());
                map.get(&key).map(|(_, value)| value.clone())
            }
        }
    }
}

/// The day a moment falls in: record keys change with it.
pub fn day_of(now_ms: i64) -> u64 {
    (now_ms / (24 * 3600 * 1000)) as u64
}

/// The key of a record, from a secret, a purpose and a day. Nobody without the secret can tell
/// which records belong together, and from day to day they do not look related.
pub fn seed(purpose: &str, secret: &[u8; 32], day: u64, slot: u8) -> [u8; 32] {
    let mut input = Vec::with_capacity(41);
    input.extend_from_slice(secret);
    input.extend_from_slice(&day.to_be_bytes());
    input.push(slot);
    blake3::derive_key(&format!("whatsmine record {purpose} v1"), &input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_records_keep_the_last_write() {
        let records = Records::Local(LocalRecords::default());
        let seed = seed("test", &[1; 32], 7, 0);
        assert_eq!(records.get(&seed).await, None);
        assert!(records.put(&seed, b"one").await);
        assert!(records.put(&seed, b"two").await);
        assert_eq!(records.get(&seed).await.as_deref(), Some(&b"two"[..]));
    }

    #[tokio::test]
    async fn what_does_not_fit_is_refused() {
        let records = Records::Local(LocalRecords::default());
        let seed = seed("test", &[1; 32], 7, 0);
        assert!(!records.put(&seed, &vec![0; MAX_RECORD + 1]).await);
        assert!(records.put(&seed, &vec![0; MAX_RECORD]).await);
    }

    #[test]
    fn keys_differ_by_secret_purpose_day_and_slot() {
        let base = seed("a", &[1; 32], 1, 0);
        assert_eq!(base, seed("a", &[1; 32], 1, 0));
        assert_ne!(base, seed("b", &[1; 32], 1, 0));
        assert_ne!(base, seed("a", &[2; 32], 1, 0));
        assert_ne!(base, seed("a", &[1; 32], 2, 0));
        assert_ne!(base, seed("a", &[1; 32], 1, 1));
    }

    #[tokio::test]
    async fn a_signed_record_can_be_written_again_by_anyone_and_changed_by_nobody() {
        let local = LocalRecords::default();
        let records = Records::Local(local.clone());
        let secret = [5u8; 32];
        let record = SignedRecord::sign(&secret, b"post", 3, Some(b"p1"));
        assert_eq!(
            SignedRecord::from_bytes(&record.to_bytes()),
            Some(record.clone())
        );
        assert!(records.put_signed(&record).await);
        let got = records
            .get_signed(&record.key, Some(b"p1"))
            .await
            .expect("there");
        assert_eq!(got, record);
        assert_eq!(records.get_signed(&record.key, Some(b"p2")).await, None);
        assert_eq!(records.get_signed(&record.key, None).await, None);
        // Gone after a while, until someone who has it writes it again.
        local.forget_everything();
        assert_eq!(records.get_signed(&record.key, Some(b"p1")).await, None);
        assert!(records.put_signed(&got).await);
        assert!(records.get_signed(&record.key, Some(b"p1")).await.is_some());
        // Something older does not replace something newer.
        let newer = SignedRecord::sign(&secret, b"newer", 4, Some(b"p1"));
        assert!(records.put_signed(&newer).await);
        assert!(records.put_signed(&record).await);
        assert_eq!(
            records
                .get_signed(&record.key, Some(b"p1"))
                .await
                .unwrap()
                .value,
            b"newer"
        );
        // Bytes that make no record.
        assert_eq!(SignedRecord::from_bytes(&[1, 2, 3]), None);
        let mut long_salt = record.to_bytes();
        long_salt[104..108].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(SignedRecord::from_bytes(&long_salt), None);
    }

    #[test]
    fn a_day_is_a_day() {
        assert_eq!(day_of(0), 0);
        assert_eq!(day_of(24 * 3600 * 1000 - 1), 0);
        assert_eq!(day_of(24 * 3600 * 1000), 1);
    }
}
