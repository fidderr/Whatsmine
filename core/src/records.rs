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
pub struct LocalRecords(Arc<Mutex<LocalMap>>);

type LocalMap = HashMap<[u8; 32], (i64, Vec<u8>)>;

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

    #[test]
    fn a_day_is_a_day() {
        assert_eq!(day_of(0), 0);
        assert_eq!(day_of(24 * 3600 * 1000 - 1), 0);
        assert_eq!(day_of(24 * 3600 * 1000), 1);
    }
}
