//! Where someone is right now, sealed. Published in the DHT under a key that changes daily, and
//! readable only with the location secret that accepted contacts hold. Signed by the person key,
//! so one contact cannot send another to the wrong place.

use std::net::SocketAddr;

use iroh::{EndpointAddr, EndpointId, PublicKey, RelayUrl, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::model::Millis;
use crate::records::{self, Records};
use crate::vault::{open_with, seal_with};

const PURPOSE: &str = "location";
/// Enough to find someone, and it keeps the record under what the DHT takes.
const MAX_ADDRS: usize = 8;
const CONTEXT: &[u8] = b"whatsmine-location-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationRecord {
    pub network: [u8; 32],
    pub relay: Option<String>,
    pub addrs: Vec<SocketAddr>,
    /// When this was written.
    pub at: Millis,
}

impl LocationRecord {
    pub fn from_addr(addr: &EndpointAddr, at: Millis) -> Self {
        // IPv6 first: no NAT in the way, usually.
        let mut addrs: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
        addrs.sort_by_key(SocketAddr::is_ipv4);
        addrs.truncate(MAX_ADDRS);
        Self {
            network: *addr.id.as_bytes(),
            relay: addr.relay_urls().next().map(ToString::to_string),
            addrs,
            at,
        }
    }

    pub fn to_addr(&self) -> Option<EndpointAddr> {
        let id = EndpointId::from_bytes(&self.network).ok()?;
        let mut addr = EndpointAddr::new(id);
        for ip in &self.addrs {
            addr = addr.with_ip_addr(*ip);
        }
        if let Some(url) = self
            .relay
            .as_deref()
            .and_then(|u| u.parse::<RelayUrl>().ok())
        {
            addr = addr.with_relay_url(url);
        }
        Some(addr)
    }

    fn signing_bytes(&self) -> Vec<u8> {
        let mut out = CONTEXT.to_vec();
        out.extend(postcard::to_stdvec(self).unwrap_or_default());
        out
    }
}

#[derive(Serialize, Deserialize)]
struct Signed {
    record: LocationRecord,
    #[serde(with = "serde_bytes_64")]
    sig: [u8; 64],
}

mod serde_bytes_64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let bytes = <&[u8]>::deserialize(d)?;
        bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("a signature is 64 bytes"))
    }
}

fn seal_key(secret: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("whatsmine location seal v1", secret)
}

/// What goes in the DHT.
pub fn seal(secret: &[u8; 32], record: LocationRecord, person: &SecretKey) -> Vec<u8> {
    let sig = person.sign(&record.signing_bytes()).to_bytes();
    let bytes = postcard::to_stdvec(&Signed { record, sig }).expect("a record serializes");
    seal_with(&seal_key(secret), &bytes, CONTEXT)
}

/// What came out of the DHT, if it can be read and `person` really wrote it.
pub fn open(secret: &[u8; 32], sealed: &[u8], person: &PublicKey) -> Option<LocationRecord> {
    let bytes = open_with(&seal_key(secret), sealed, CONTEXT)?;
    let signed: Signed = postcard::from_bytes(&bytes).ok()?;
    person
        .verify(
            &signed.record.signing_bytes(),
            &Signature::from_bytes(&signed.sig),
        )
        .ok()?;
    Some(signed.record)
}

/// Publishes under today's key. Each device of a person has its own place: `slot`.
pub async fn publish(
    records: &Records,
    secret: &[u8; 32],
    sealed: &[u8],
    now: Millis,
    slot: u8,
) -> bool {
    let seed = records::seed(PURPOSE, secret, records::day_of(now), slot);
    records.put(&seed, sealed).await
}

/// Looks under today's key and yesterday's: someone asleep since before midnight is still there.
async fn lookup_slot(
    records: Records,
    secret: [u8; 32],
    person: PublicKey,
    now: Millis,
    slot: u8,
) -> Option<LocationRecord> {
    let today = records::day_of(now);
    for day in [today, today.saturating_sub(1)] {
        let seed = records::seed(PURPOSE, &secret, day, slot);
        if let Some(sealed) = records.get(&seed).await
            && let Some(record) = open(&secret, &sealed, &person)
        {
            return Some(record);
        }
    }
    None
}

/// Where someone is: on whichever of their devices was around last.
pub async fn lookup(
    records: &Records,
    secret: &[u8; 32],
    person: &PublicKey,
    now: Millis,
) -> Option<LocationRecord> {
    lookup_skipping(records, secret, person, now, None).await
}

/// The same, except for one device: for finding our own other one.
pub async fn lookup_skipping(
    records: &Records,
    secret: &[u8; 32],
    person: &PublicKey,
    now: Millis,
    skip: Option<u8>,
) -> Option<LocationRecord> {
    let mut looking = tokio::task::JoinSet::new();
    for slot in 0..crate::sibling::DEVICES {
        if Some(slot) != skip {
            looking.spawn(lookup_slot(records.clone(), *secret, *person, now, slot));
        }
    }
    let mut best: Option<LocationRecord> = None;
    let newest = |found: Option<LocationRecord>, best: &mut Option<LocationRecord>| {
        if let Some(found) = found
            && best.as_ref().is_none_or(|b| b.at < found.at)
        {
            *best = Some(found);
        }
    };
    // Most people have one device: an empty place must not hold up the one that is filled.
    while let Some(done) = looking.join_next().await {
        if let Ok(found) = done {
            newest(found, &mut best);
            if best.is_some() {
                break;
            }
        }
    }
    if best.is_some() {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(300), async {
            while let Some(done) = looking.join_next().await {
                if let Ok(found) = done {
                    newest(found, &mut best);
                }
            }
        })
        .await;
    }
    looking.abort_all();
    best
}

/// The secret a group's members find each other with, one per member.
pub fn group_secret(group_secret: &[u8; 32], member: &PublicKey) -> [u8; 32] {
    let mut input = group_secret.to_vec();
    input.extend_from_slice(member.as_bytes());
    blake3::derive_key("whatsmine group location v1", &input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{LocalRecords, MAX_RECORD};
    use crate::vault::random;

    fn record(key: &SecretKey, addrs: usize) -> LocationRecord {
        LocationRecord {
            network: *key.public().as_bytes(),
            relay: Some("https://euc1-1.relay.n0.iroh.link./".into()),
            addrs: (0..addrs)
                .map(|i| {
                    format!("[2a02:a461:643f:0:54d2:9605:baf1:{i:x}]:57934")
                        .parse()
                        .unwrap()
                })
                .collect(),
            at: 1_793_803_000_000,
        }
    }

    #[test]
    fn only_the_secret_opens_it_and_only_the_person_could_write_it() {
        let (person, secret) = (SecretKey::generate(), random());
        let r = record(&person, 2);
        let sealed = seal(&secret, r.clone(), &person);
        assert_eq!(open(&secret, &sealed, &person.public()), Some(r.clone()));
        assert_eq!(open(&random(), &sealed, &person.public()), None);
        // A contact has the secret, so can write a record, but cannot sign as someone else.
        let other = SecretKey::generate();
        let forged = seal(&secret, r, &other);
        assert_eq!(open(&secret, &forged, &person.public()), None);
    }

    #[test]
    fn the_biggest_record_fits_in_the_dht() {
        let person = SecretKey::generate();
        let sealed = seal(&random(), record(&person, MAX_ADDRS), &person);
        assert!(sealed.len() <= MAX_RECORD, "{} bytes", sealed.len());
    }

    #[test]
    fn it_comes_back_as_an_address_ipv6_first() {
        let person = SecretKey::generate();
        let addr = EndpointAddr::new(person.public())
            .with_ip_addr("1.2.3.4:5".parse().unwrap())
            .with_ip_addr("[2001:db8::1]:6".parse().unwrap())
            .with_relay_url("https://relay.example./".parse().unwrap());
        let r = LocationRecord::from_addr(&addr, 9);
        assert!(r.addrs[0].is_ipv6());
        assert_eq!(r.to_addr(), Some(addr));
    }

    #[tokio::test]
    async fn yesterday_is_still_found_and_the_day_before_is_not() {
        let records = Records::Local(LocalRecords::default());
        let (person, secret) = (SecretKey::generate(), random());
        let day = 24 * 3600 * 1000;
        let now = 100 * day;
        let sealed = seal(&secret, record(&person, 1), &person);
        assert!(publish(&records, &secret, &sealed, now, 0).await);
        let public = person.public();
        let found = |at| lookup(&records, &secret, &public, at);
        assert!(found(now).await.is_some());
        assert!(found(now + day).await.is_some());
        assert!(found(now + 2 * day).await.is_none());
    }

    #[tokio::test]
    async fn the_device_around_last_is_the_one_found() {
        let records = Records::Local(LocalRecords::default());
        let (person, secret) = (SecretKey::generate(), random());
        let now = 100 * 24 * 3600 * 1000;
        let mut older = record(&person, 1);
        older.at = 5;
        let mut newer = record(&person, 2);
        newer.at = 6;
        publish(
            &records,
            &secret,
            &seal(&secret, older.clone(), &person),
            now,
            0,
        )
        .await;
        publish(
            &records,
            &secret,
            &seal(&secret, newer.clone(), &person),
            now,
            1,
        )
        .await;
        let public = person.public();
        assert_eq!(lookup(&records, &secret, &public, now).await, Some(newer));
        // Our own other device: not ourselves.
        assert_eq!(
            lookup_skipping(&records, &secret, &public, now, Some(1)).await,
            Some(older)
        );
    }

    #[test]
    fn each_member_of_a_group_has_their_own_secret() {
        let (a, b) = (
            SecretKey::generate().public(),
            SecretKey::generate().public(),
        );
        let secret = random();
        assert_ne!(group_secret(&secret, &a), group_secret(&secret, &b));
        assert_ne!(group_secret(&secret, &a), group_secret(&random(), &a));
    }
}
