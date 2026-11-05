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

/// How long something lasted, next to when it was: "45 s", "12 min", "1 h 5 min". A clock-like
/// "12:00" right after a time of day reads as a second time.
pub fn length(tr: &Translations, ms: u64) -> String {
    length_in(
        [
            &t(tr, "length.seconds"),
            &t(tr, "length.minutes"),
            &t(tr, "length.hours"),
        ],
        ms,
    )
}

/// [`length`] with the three ways of saying it given: seconds, minutes, hours and minutes.
fn length_in(say: [&str; 3], ms: u64) -> String {
    let s = (ms + 500) / 1000;
    if s < 60 {
        return say[0].replace("{n}", &s.to_string());
    }
    let m = (s + 30) / 60;
    if m < 60 {
        return say[1].replace("{n}", &m.to_string());
    }
    say[2]
        .replace("{h}", &(m / 60).to_string())
        .replace("{m}", &(m % 60).to_string())
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

/// A moment, in full enough words for a list of what is planned.
pub fn when(tr: &Translations, ms: Millis) -> String {
    format!("{} {}", day_label(tr, ms), clock(ms))
}

/// How long until something, in the largest unit that says enough.
pub fn wait_time(tr: &Translations, ms: u64) -> String {
    let minutes = ms / 60_000;
    if ms < 60_000 {
        crate::app::t(tr, "time.seconds").replace("{n}", &(ms / 1000).max(1).to_string())
    } else if minutes < 60 {
        crate::app::t(tr, "time.minutes").replace("{n}", &minutes.to_string())
    } else {
        crate::app::t(tr, "time.hours").replace("{n}", &(minutes / 60).to_string())
    }
}

/// A bit of text with the marks people type around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Styled {
    Plain(String),
    Bold(String),
    Italic(String),
    Strike(String),
    Code(String),
    Spoiler(String),
}

/// Finds `*bold*`, `_italic_`, `~gone~`, `` `code` `` and `||spoiler||`. A mark only counts at the
/// edge of a word, so 2*3*4 and snake_case_names stay as they are.
pub fn styled(text: &str) -> Vec<Styled> {
    type Make = fn(String) -> Styled;
    let marks: [(&str, Make); 5] = [
        ("||", Styled::Spoiler),
        ("`", Styled::Code),
        ("*", Styled::Bold),
        ("_", Styled::Italic),
        ("~", Styled::Strike),
    ];
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    'outer: while !rest.is_empty() {
        let before = plain.chars().last();
        let at_edge = before.is_none_or(|c| !c.is_alphanumeric());
        if at_edge {
            for (mark, make) in &marks {
                if let Some(after) = rest.strip_prefix(mark)
                    && !after.starts_with(char::is_whitespace)
                    && let Some(end) = after.find(mark)
                    && end > 0
                    && !after[..end].ends_with(char::is_whitespace)
                    && !after[..end].contains('\n')
                    && after[end + mark.len()..]
                        .chars()
                        .next()
                        .is_none_or(|c| !c.is_alphanumeric())
                {
                    if !plain.is_empty() {
                        out.push(Styled::Plain(std::mem::take(&mut plain)));
                    }
                    out.push(make(after[..end].to_owned()));
                    rest = &after[end + mark.len()..];
                    continue 'outer;
                }
            }
        }
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            plain.push(c);
        }
        rest = chars.as_str();
    }
    if !plain.is_empty() {
        out.push(Styled::Plain(plain));
    }
    out
}

/// The text without the marks: for a line in a list or a quote.
pub fn plain(text: &str) -> String {
    styled(text)
        .into_iter()
        .map(|piece| match piece {
            Styled::Plain(t)
            | Styled::Bold(t)
            | Styled::Italic(t)
            | Styled::Strike(t)
            | Styled::Code(t) => t,
            // A secret stays one in a list.
            Styled::Spoiler(t) => "•".repeat(t.chars().count().min(8)),
        })
        .collect()
}

/// Splits off who is named with an @: text, and whether it is a name.
pub fn with_mentions(text: &str, names: &[String]) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((at, name)) = names
        .iter()
        .filter(|n| !n.is_empty())
        .filter_map(|n| rest.find(&format!("@{n}")).map(|i| (i, n)))
        .min_by_key(|(i, _)| *i)
    {
        if at > 0 {
            out.push((rest[..at].to_owned(), false));
        }
        let end = at + 1 + name.len();
        out.push((rest[at..end].to_owned(), true));
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        out.push((rest.to_owned(), false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Piece, Styled, bytes, duration, grouped_key, length_in, percent, pieces, styled,
        with_mentions,
    };

    #[test]
    fn how_long_reads_as_a_length() {
        let say = ["{n} s", "{n} min", "{h} h {m} min"];
        assert_eq!(length_in(say, 0), "0 s");
        assert_eq!(length_in(say, 45_400), "45 s");
        assert_eq!(length_in(say, 59_600), "1 min");
        assert_eq!(length_in(say, 12 * 60_000 + 10_000), "12 min");
        assert_eq!(length_in(say, 25 * 60_000), "25 min");
        assert_eq!(length_in(say, 65 * 60_000), "1 h 5 min");
    }

    #[test]
    fn marks_only_count_at_the_edge_of_a_word() {
        assert_eq!(
            styled("a *big* one"),
            vec![
                Styled::Plain("a ".into()),
                Styled::Bold("big".into()),
                Styled::Plain(" one".into())
            ]
        );
        assert_eq!(styled("2*3*4"), vec![Styled::Plain("2*3*4".into())]);
        assert_eq!(
            styled("snake_case_name"),
            vec![Styled::Plain("snake_case_name".into())]
        );
        assert_eq!(styled("* no *"), vec![Styled::Plain("* no *".into())]);
        assert_eq!(styled("||secret||"), vec![Styled::Spoiler("secret".into())]);
        assert_eq!(styled("`a*b*`"), vec![Styled::Code("a*b*".into())]);
        assert_eq!(super::plain("a *big* one ||no||"), "a big one ••");
    }

    #[test]
    fn names_are_found_in_a_text() {
        let names = vec!["Roy".to_owned()];
        assert_eq!(
            with_mentions("hi @Roy, ok", &names),
            vec![
                ("hi ".to_owned(), false),
                ("@Roy".to_owned(), true),
                (", ok".to_owned(), false)
            ]
        );
        assert_eq!(
            with_mentions("nobody", &names),
            vec![("nobody".to_owned(), false)]
        );
    }

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
