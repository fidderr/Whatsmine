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

#[cfg(test)]
mod tests {
    use super::grouped_key;

    #[test]
    fn keys_group_by_four() {
        assert_eq!(grouped_key("abcdefghij"), "abcd efgh ij");
    }
}
