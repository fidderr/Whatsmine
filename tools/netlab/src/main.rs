//! netlab: measures what WhatsMine's connection plan rests on, on this device and this network.
//! A small program on purpose: it runs on a PC, and over adb on an emulator or a phone, with no
//! app and no data of anyone's. Every result is one line of JSON, and a summary at the end.
//!
//!   netlab net                     NAT type, IPv4 and IPv6, relays reachable and how fast
//!   netlab dht [ROUNDS] [SIZE]     DHT: write a record, read it back through another client
//!   netlab dht-put FILE [COUNT]    write records and remember them, to read much later
//!   netlab dht-check FILE          how many of those can still be read
//!   netlab listen [--no-relay]     wait for `netlab dial`, prints what to give it
//!   netlab dial TICKET [ROUNDS] [--no-relay] [--only ipv4|ipv6|relay]
//!                                  time to connect, direct or through a relay, round trips
//!   netlab self [ROUNDS]           both ends in this program: the floor of what is possible

use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use futures_lite::StreamExt;
use iroh::endpoint::{Connection, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl, TransportAddr, Watcher};
use n0_mainline::{Dht, MutableItem, SigningKey};
use serde_json::{Value, json};

const ALPN: &[u8] = b"whatsmine-netlab/1";
/// A sealed location record is at most this: what BEP44 allows for a value.
const RECORD_SIZE: usize = 1000;
const WAIT: Duration = Duration::from_secs(30);

fn line(value: Value) {
    println!("{value}");
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// The arguments that are not flags, nor the value of one.
fn plain(args: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
        } else if arg == "--only" {
            skip = true;
        } else if !arg.starts_with("--") {
            out.push(arg.as_str());
        }
    }
    out
}

/// Sorted milliseconds: the fastest, the middle one and the one nearly all are under.
fn spread(mut samples: Vec<u128>) -> Value {
    samples.sort_unstable();
    let at = |part: f64| -> Option<u128> {
        let last = samples.len().checked_sub(1)?;
        samples.get((last as f64 * part).round() as usize).copied()
    };
    json!({ "n": samples.len(), "min_ms": at(0.0), "median_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0) })
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let numbers = plain(rest);
    let number = |index: usize, default: usize| -> usize {
        numbers
            .get(index)
            .and_then(|n| n.parse().ok())
            .unwrap_or(default)
    };
    match args.first().map(String::as_str) {
        Some("net") => net().await,
        Some("dht") => dht(number(0, 10), number(1, RECORD_SIZE)).await,
        Some("dht-put") => {
            let file = numbers.first().context("dht-put FILE [COUNT]")?;
            dht_put(Path::new(file), number(1, 20)).await
        }
        Some("dht-check") => {
            let file = numbers.first().context("dht-check FILE")?;
            dht_check(Path::new(file)).await
        }
        Some("listen") => listen(flag(rest, "--no-relay")).await,
        Some("dial") => {
            let ticket = numbers.first().context("dial TICKET [ROUNDS]")?;
            dial(
                ticket,
                number(1, 5),
                flag(rest, "--no-relay"),
                option(rest, "--only"),
            )
            .await
        }
        Some("self") => self_test(number(0, 5)).await,
        _ => {
            eprintln!("{}", usage());
            bail!("what should be measured?")
        }
    }
}

fn usage() -> &'static str {
    "netlab net | dht [ROUNDS] [SIZE] | dht-put FILE [COUNT] | dht-check FILE | listen [--no-relay] | dial TICKET [ROUNDS] [--no-relay] [--only ipv4|ipv6|relay] | self [ROUNDS]"
}

async fn endpoint(no_relay: bool) -> Result<Endpoint> {
    let builder = if no_relay {
        Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled)
    } else {
        Endpoint::builder(presets::N0)
    };
    builder
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .context("binding the endpoint")
}

// ---- What kind of network is this ----

async fn net() -> Result<()> {
    let started = Instant::now();
    let ep = endpoint(false).await?;
    let bound = ep.bound_sockets();
    let report = tokio::time::timeout(WAIT, ep.net_report().initialized())
        .await
        .context("no network report within 30 s")?;
    let nat = |varies: Option<bool>| match varies {
        Some(false) => "same address to everyone: holes can be punched",
        Some(true) => "another address per destination: holes rarely work, needs a relay",
        None => "unknown",
    };
    let relays: Vec<Value> = report
        .relay_latency
        .iter()
        .map(|(_, url, latency)| json!({ "relay": url.to_string(), "ms": latency.as_millis() }))
        .collect();
    line(json!({
        "test": "net",
        "bound": bound.iter().map(SocketAddr::to_string).collect::<Vec<_>>(),
        "udp_v4": report.udp_v4,
        "udp_v6": report.udp_v6,
        "global_v4": report.global_v4.map(|a| a.to_string()),
        "global_v6": report.global_v6.map(|a| a.to_string()),
        "nat_v4": nat(report.mapping_varies_by_dest_ipv4),
        "nat_v6": nat(report.mapping_varies_by_dest_ipv6),
        "captive_portal": report.captive_portal,
        "preferred_relay": report.preferred_relay.as_ref().map(ToString::to_string),
        "relays": relays,
        "report_after_ms": started.elapsed().as_millis(),
    }));
    ep.close().await;
    Ok(())
}

// ---- The DHT ----

fn now_s() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn new_signer() -> SigningKey {
    let mut bytes = [0u8; 32];
    // Not a secret: a throwaway key for a throwaway record, different each time.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mixed = nanos ^ u128::from(std::process::id());
    for (slot, byte) in bytes.iter_mut().zip(mixed.to_le_bytes().iter().cycle()) {
        *slot = *byte;
    }
    bytes[31] ^= 0x5a;
    SigningKey::from_bytes(&bytes)
}

async fn client(name: &str) -> Result<Dht> {
    let started = Instant::now();
    let dht = Dht::client().context("starting a DHT client")?;
    let ok = dht.bootstrapped().await.unwrap_or(false);
    line(
        json!({ "test": "dht_bootstrap", "client": name, "ok": ok, "ms": started.elapsed().as_millis() }),
    );
    Ok(dht)
}

/// The first copy any node hands back: as soon as a reader can act on it.
async fn read_first(dht: &Dht, key: &[u8; 32]) -> Option<Vec<u8>> {
    let mut stream = dht.get_mutable(key, None, None).await.ok()?;
    let item = tokio::time::timeout(WAIT, stream.next()).await.ok()??;
    Some(item.value().to_vec())
}

async fn dht(rounds: usize, size: usize) -> Result<()> {
    anyhow::ensure!(
        size <= RECORD_SIZE,
        "a record is at most {RECORD_SIZE} bytes"
    );
    let writer = client("writer").await?;
    let reader = client("reader").await?;
    let (mut puts, mut gets, mut put_ok, mut get_ok) = (Vec::new(), Vec::new(), 0, 0);
    for round in 0..rounds {
        // Each its own key: nothing cached helps.
        tokio::time::sleep(Duration::from_millis(5)).await;
        let signer = new_signer();
        let value = vec![(round % 251) as u8; size];
        let key = signer.verifying_key().to_bytes();
        let item = MutableItem::new(&signer, &value, now_s(), None);

        let started = Instant::now();
        let put = writer.put_mutable(item, None).await;
        let put_ms = started.elapsed().as_millis();
        let wrote = put.is_ok();

        let started = Instant::now();
        let got = read_first(&reader, &key).await;
        let get_ms = started.elapsed().as_millis();
        let read = got.as_deref() == Some(&value[..]);

        line(json!({
            "test": "dht", "round": round, "put_ok": wrote, "put_ms": put_ms,
            "get_ok": read, "get_ms": get_ms, "error": put.err().map(|e| e.to_string()),
        }));
        if wrote {
            put_ok += 1;
            puts.push(put_ms);
        }
        if read {
            get_ok += 1;
            gets.push(get_ms);
        }
    }
    line(json!({
        "summary": "dht", "rounds": rounds, "record_bytes": size,
        "put_ok": put_ok, "get_ok": get_ok, "put": spread(puts), "get": spread(gets),
    }));
    Ok(())
}

/// Records to read back much later: how long does the DHT keep them without a republish?
async fn dht_put(file: &Path, count: usize) -> Result<()> {
    let writer = client("writer").await?;
    let mut lines = String::new();
    let mut written = 0;
    for round in 0..count {
        tokio::time::sleep(Duration::from_millis(5)).await;
        let signer = new_signer();
        let value = vec![(round % 251) as u8; RECORD_SIZE];
        let key = signer.verifying_key().to_bytes();
        let item = MutableItem::new(&signer, &value, now_s(), None);
        if writer.put_mutable(item, None).await.is_ok() {
            written += 1;
            let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
            lines.push_str(&format!("{hex} {}\n", now_s()));
        }
    }
    tokio::fs::write(file, lines).await?;
    line(
        json!({ "test": "dht_put", "asked": count, "written": written, "file": file.display().to_string() }),
    );
    Ok(())
}

async fn dht_check(file: &Path) -> Result<()> {
    let reader = client("reader").await?;
    let text = tokio::fs::read_to_string(file).await?;
    let (mut found, mut total, mut ages) = (0, 0, Vec::new());
    for row in text.lines() {
        let Some((hex, when)) = row.split_once(' ') else {
            continue;
        };
        let mut key = [0u8; 32];
        for (slot, pair) in key.iter_mut().zip(hex.as_bytes().chunks(2)) {
            *slot = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
        }
        total += 1;
        if read_first(&reader, &key).await.is_some() {
            found += 1;
        }
        ages.push(now_s() - when.parse::<i64>().unwrap_or(0));
    }
    line(json!({
        "summary": "dht_check", "written": total, "still_there": found,
        "age_minutes": ages.first().map(|a| a / 60),
    }));
    Ok(())
}

// ---- Two ends ----

fn ticket(addr: &EndpointAddr) -> String {
    let mut parts = vec![addr.id.to_string()];
    parts.extend(addr.addrs.iter().filter_map(|a| match a {
        TransportAddr::Relay(url) => Some(format!("relay={url}")),
        TransportAddr::Ip(ip) => Some(format!("ip={ip}")),
        _ => None,
    }));
    parts.join(",")
}

fn parse_ticket(text: &str, only: Option<&str>) -> Result<EndpointAddr> {
    let mut parts = text.split(',');
    let id = EndpointId::from_str(parts.next().context("empty ticket")?)?;
    let mut addr = EndpointAddr::new(id);
    for part in parts {
        if let Some(url) = part.strip_prefix("relay=") {
            if matches!(only, None | Some("relay")) {
                addr = addr.with_relay_url(RelayUrl::from_str(url)?);
            }
        } else if let Some(ip) = part.strip_prefix("ip=") {
            let ip: SocketAddr = ip.parse()?;
            let wanted = match only {
                None => true,
                Some("ipv4") => ip.is_ipv4(),
                Some("ipv6") => ip.is_ipv6(),
                Some(_) => false,
            };
            if wanted {
                addr = addr.with_ip_addr(ip);
            }
        }
    }
    anyhow::ensure!(
        !addr.is_empty(),
        "no address of the kind asked for in the ticket"
    );
    Ok(addr)
}

/// Echo what comes in, for as long as the other end wants.
async fn serve(ep: Endpoint) {
    while let Some(incoming) = ep.accept().await {
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else {
                return;
            };
            while let Ok((mut send, mut recv)) = connection.accept_bi().await {
                let mut buffer = [0u8; 64];
                while let Ok(Some(n)) = recv.read(&mut buffer).await {
                    if send.write_all(&buffer[..n]).await.is_err() {
                        break;
                    }
                }
                let _ = send.finish();
            }
        });
    }
}

async fn listen(no_relay: bool) -> Result<()> {
    let ep = endpoint(no_relay).await?;
    if !no_relay {
        // With a relay, or the ticket has no way in from another network.
        let _ = tokio::time::timeout(WAIT, ep.online()).await;
    }
    // A moment for the addresses to settle.
    tokio::time::sleep(Duration::from_secs(2)).await;
    line(json!({ "test": "listen", "ticket": ticket(&ep.addr()) }));
    serve(ep).await;
    Ok(())
}

/// Until the selected path is a direct one, for at most `limit`.
async fn wait_direct(connection: &Connection, limit: Duration) -> Option<Duration> {
    let started = Instant::now();
    let mut paths = connection.paths_stream();
    tokio::time::timeout(limit, async {
        while let Some(list) = paths.next().await {
            if list.iter().any(|path| path.is_selected() && path.is_ip()) {
                return started.elapsed();
            }
        }
        // The connection closed.
        limit
    })
    .await
    .ok()
}

async fn round_trips(connection: &Connection, count: u32) -> Result<Vec<u128>> {
    let (mut send, mut recv) = connection.open_bi().await?;
    let mut out = Vec::new();
    let mut back = [0u8; 16];
    for _ in 0..count {
        let started = Instant::now();
        send.write_all(&[7u8; 16]).await?;
        recv.read_exact(&mut back).await?;
        out.push(started.elapsed().as_micros() / 1000);
    }
    send.finish()?;
    Ok(out)
}

/// One connection from a cold start: a new endpoint, as an app that just woke up has.
async fn one_dial(addr: &EndpointAddr, no_relay: bool, round: usize) -> Result<Value> {
    let bind_started = Instant::now();
    let ep = endpoint(no_relay).await?;
    let bind_ms = bind_started.elapsed().as_millis();
    let started = Instant::now();
    let connection = tokio::time::timeout(WAIT, ep.connect(addr.clone(), ALPN))
        .await
        .context("no connection within 30 s")??;
    let connect_ms = started.elapsed().as_millis();
    let direct = wait_direct(&connection, Duration::from_secs(15)).await;
    let trips = round_trips(&connection, 10).await?;
    let path = connection
        .paths()
        .iter()
        .find(|path| path.is_selected())
        .map(|path| (path.is_ip(), path.rtt().as_millis()));
    let result = json!({
        "test": "dial", "round": round, "bind_ms": bind_ms, "connect_ms": connect_ms,
        "direct_after_ms": direct.map(|d| d.as_millis()),
        "ends_direct": path.map(|p| p.0), "path_rtt_ms": path.map(|p| p.1),
        "round_trip": spread(trips),
    });
    connection.close(0u32.into(), b"done");
    ep.close().await;
    Ok(result)
}

async fn dial(text: &str, rounds: usize, no_relay: bool, only: Option<&str>) -> Result<()> {
    let addr = parse_ticket(text, only)?;
    let (mut connects, mut direct, mut failed) = (Vec::new(), 0, 0);
    for round in 0..rounds {
        match one_dial(&addr, no_relay, round).await {
            Ok(result) => {
                if let Some(ms) = result["connect_ms"].as_u64() {
                    connects.push(u128::from(ms));
                }
                if result["ends_direct"] == json!(true) {
                    direct += 1;
                }
                line(result);
            }
            Err(e) => {
                failed += 1;
                line(json!({ "test": "dial", "round": round, "error": format!("{e:#}") }));
            }
        }
    }
    line(json!({
        "summary": "dial", "rounds": rounds, "failed": failed, "ended_direct": direct,
        "no_relay": no_relay, "only": only, "connect": spread(connects),
    }));
    Ok(())
}

async fn self_test(rounds: usize) -> Result<()> {
    let listener = endpoint(true).await?;
    // The first addresses take a moment.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let text = ticket(&listener.addr());
    tokio::spawn(serve(listener));
    dial(&text, rounds, true, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_of_nothing_says_nothing() {
        let out = spread(Vec::new());
        assert_eq!(out["n"], 0);
        assert!(out["median_ms"].is_null());
    }

    #[test]
    fn spread_takes_the_middle_and_the_high_end() {
        let out = spread((1..=100).rev().collect());
        assert_eq!(out["min_ms"], 1);
        assert_eq!(out["median_ms"], 51);
        assert_eq!(out["p95_ms"], 95);
        assert_eq!(out["max_ms"], 100);
    }

    #[test]
    fn only_the_numbers_are_plain_arguments() {
        let args: Vec<String> = ["abc", "--only", "ipv6", "5", "--no-relay"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(plain(&args), vec!["abc", "5"]);
        assert_eq!(option(&args, "--only"), Some("ipv6"));
        assert!(flag(&args, "--no-relay"));
    }

    #[test]
    fn a_ticket_comes_back_as_it_went_and_can_be_narrowed() {
        let id = EndpointId::from_str(
            "ae58ff8833241ac82d6ff7611046ed67b5072d142c588d0063e942d9a75502b6",
        )
        .unwrap();
        let addr = EndpointAddr::new(id)
            .with_ip_addr("1.2.3.4:5".parse().unwrap())
            .with_ip_addr("[2001:db8::1]:6".parse().unwrap())
            .with_relay_url(RelayUrl::from_str("https://relay.example./").unwrap());
        let text = ticket(&addr);
        assert_eq!(parse_ticket(&text, None).unwrap(), addr);
        assert_eq!(parse_ticket(&text, Some("ipv4")).unwrap().addrs.len(), 1);
        assert_eq!(parse_ticket(&text, Some("ipv6")).unwrap().addrs.len(), 1);
        assert_eq!(parse_ticket(&text, Some("relay")).unwrap().addrs.len(), 1);
        assert!(parse_ticket(&format!("{id}"), None).is_err());
    }

    #[test]
    fn two_new_keys_differ() {
        let one = new_signer().verifying_key().to_bytes();
        std::thread::sleep(Duration::from_millis(2));
        assert_ne!(one, new_signer().verifying_key().to_bytes());
    }
}
