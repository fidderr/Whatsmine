//! Human-friendly times in the user's language and time zone.

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone};
use whatsmine_core::Millis;

use crate::app::{Translations, t};

fn local(ms: Millis) -> DateTime<Local> {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Local::now)
}

pub fn day_of(ms: Millis) -> NaiveDate {
    local(ms).date_naive()
}

/// "14:05"
pub fn clock(ms: Millis) -> String {
    local(ms).format("%H:%M").to_string()
}

/// How many calendar days ago (0 = today).
fn days_ago(ms: Millis) -> i64 {
    (Local::now().date_naive() - day_of(ms)).num_days()
}

/// "Today", "Yesterday", "Monday" (within a week) or "16-10-2026".
pub fn day_label(tr: &Translations, ms: Millis) -> String {
    match days_ago(ms) {
        0 => t(tr, "time.today"),
        1 => t(tr, "time.yesterday"),
        2..=6 => t(
            tr,
            &format!("day.{}", local(ms).weekday().num_days_from_monday()),
        ),
        _ => local(ms).format("%d-%m-%Y").to_string(),
    }
}

/// Chat list: the time for today, otherwise the day.
pub fn list_time(tr: &Translations, ms: Millis) -> String {
    if days_ago(ms) == 0 {
        clock(ms)
    } else {
        day_label(tr, ms)
    }
}

/// "last seen today at 14:05"
pub fn last_seen(tr: &Translations, ms: Millis) -> String {
    let key = match days_ago(ms) {
        0 => "chat.last_seen_today",
        1 => "chat.last_seen_yesterday",
        _ => "chat.last_seen_on",
    };
    t(tr, key)
        .replace("{time}", &clock(ms))
        .replace("{date}", &local(ms).format("%d-%m-%Y").to_string())
}

/// Splits a key into groups of four characters, easier to read and compare.
pub fn grouped_key(key: &str) -> String {
    key.chars()
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// "5.2 MB", "340 KB", "12 B" (decimal units, like phones show them).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1000 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// "0:07", "12:03", "1:02:03".
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Percentage 0..=100 for a progress pair.
pub fn percent(done: u64, total: u64) -> u64 {
    (done.min(total) * 100).checked_div(total).unwrap_or(0)
}

/// A piece of a message: plain text, or an address that can be clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Link(String),
}

/// Splits a message into text and links (http(s):// addresses, www. ones, and
/// whatsmine:// invitations). Punctuation that ends a sentence is not part of
/// the link.
pub fn pieces(body: &str) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut text = String::new();
    let mut rest = body;
    while !rest.is_empty() {
        // Each word is a candidate; whitespace stays where it was.
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if end == 0 {
            let ch = rest.chars().next().unwrap_or(' ');
            text.push(ch);
            rest = &rest[ch.len_utf8()..];
            continue;
        }
        let (word, tail) = rest.split_at(end);
        // Brackets and quotes around a link are not part of it.
        let core = word.trim_start_matches(['(', '[', '<', '"', '\'']);
        let lead = word.len() - core.len();
        let trimmed =
            core.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\'']);
        let is_link = (trimmed.len() > 8
            && (trimmed.starts_with("https://") || trimmed.starts_with("http://")))
            || (trimmed.len() > 4 && trimmed.starts_with("www."))
            || trimmed.starts_with("whatsmine://add/");
        if is_link {
            text.push_str(&word[..lead]);
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut text)));
            }
            out.push(Piece::Link(trimmed.to_string()));
            text.push_str(&core[trimmed.len()..]);
        } else {
            text.push_str(word);
        }
        rest = tail;
    }
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Piece, bytes, duration, grouped_key, percent, pieces};

    #[test]
    fn links_are_found_without_their_punctuation() {
        assert_eq!(pieces("hello"), vec![Piece::Text("hello".into())]);
        assert_eq!(
            pieces("see https://example.com/a?b=1, ok (www.example.org)."),
            vec![
                Piece::Text("see ".into()),
                Piece::Link("https://example.com/a?b=1".into()),
                Piece::Text(", ok (".into()),
                Piece::Link("www.example.org".into()),
                Piece::Text(").".into()),
            ]
        );
        assert_eq!(
            pieces("whatsmine://add/abc?n=Zo"),
            vec![Piece::Link("whatsmine://add/abc?n=Zo".into())]
        );
        // Not links.
        assert_eq!(pieces("https://").len(), 1);
        assert!(matches!(&pieces("ftp://x.y")[0], Piece::Text(_)));
        // Line breaks survive.
        let all: String = pieces("a https://x.yz\nb")
            .iter()
            .map(|p| match p {
                Piece::Text(t) | Piece::Link(t) => t.clone(),
            })
            .collect();
        assert_eq!(all, "a https://x.yz\nb");
    }

    #[test]
    fn sizes_durations_and_percentages() {
        assert_eq!(bytes(12), "12 B");
        assert_eq!(bytes(340_000), "340 KB");
        assert_eq!(bytes(5_234_000), "5.2 MB");
        assert_eq!(bytes(2_000_000_000), "2.0 GB");
        assert_eq!(duration(7_400), "0:07");
        assert_eq!(duration(723_000), "12:03");
        assert_eq!(duration(3_723_000), "1:02:03");
        assert_eq!(percent(50, 200), 25);
        assert_eq!(percent(5, 0), 0);
        assert_eq!(percent(300, 200), 100);
    }

    #[test]
    fn keys_group_by_four() {
        assert_eq!(grouped_key("abcdefghij"), "abcd efgh ij");
    }
}
