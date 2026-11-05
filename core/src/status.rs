//! Status: posts that last a day, sealed per person, for the people someone chooses.
//!
//! There is nowhere for a post to wait, so it reaches someone only if both of them are around within
//! the day. It goes out when a connection is there anyway: when someone opens the app, on a check
//! the power rules allow. Posting to fifty people does not wake a phone fifty times.

use serde::{Deserialize, Serialize};

use crate::model::Millis;

/// How long a post lasts.
pub const LASTS: Millis = 24 * 3600 * 1000;
pub const MAX_TEXT: usize = 700;
/// A photo goes in parts that fit in a frame.
pub const PHOTO_PART: usize = 64 * 1024;
pub const MOST_PARTS: u16 = 48;
/// How many posts of one person are kept. Whoever posts more loses their oldest.
pub const MOST_FROM_ONE: usize = 30;
/// How much of a post goes along with a reply.
pub const QUOTE_CHARS: usize = 80;

/// What is behind the text. A fixed list: from someone else this is a number, never a colour or any
/// other thing that ends up in a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    #[default]
    Sea,
    Dusk,
    Night,
    Forest,
    Sand,
    Plum,
}

impl Backdrop {
    pub const ALL: [Self; 6] = [
        Self::Sea,
        Self::Dusk,
        Self::Night,
        Self::Forest,
        Self::Sand,
        Self::Plum,
    ];

    pub fn byte(self) -> u8 {
        match self {
            Self::Sea => 0,
            Self::Dusk => 1,
            Self::Night => 2,
            Self::Forest => 3,
            Self::Sand => 4,
            Self::Plum => 5,
        }
    }

    pub fn from_byte(byte: u8) -> Self {
        match byte {
            1 => Self::Dusk,
            2 => Self::Night,
            3 => Self::Forest,
            4 => Self::Sand,
            5 => Self::Plum,
            _ => Self::Sea,
        }
    }

    /// As it is called in the stylesheet.
    pub fn word(self) -> &'static str {
        match self {
            Self::Sea => "sea",
            Self::Dusk => "dusk",
            Self::Night => "night",
            Self::Forest => "forest",
            Self::Sand => "sand",
            Self::Plum => "plum",
        }
    }
}

/// Who sees what someone posts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Audience {
    #[default]
    All,
    Only(Vec<String>),
    Except(Vec<String>),
}

impl Audience {
    pub fn includes(&self, id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Only(ids) => ids.iter().any(|i| i == id),
            Self::Except(ids) => !ids.iter().any(|i| i == id),
        }
    }
}

/// A post, someone's or our own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    pub id: String,
    pub author: String,
    pub text: String,
    pub backdrop: Backdrop,
    pub at: Millis,
    pub until: Millis,
    /// How many parts its photo has, if it has one.
    pub parts: u16,
    /// We looked at it.
    pub seen: bool,
    /// We told them we looked at it.
    pub seen_told: bool,
}

impl Post {
    pub fn has_photo(&self) -> bool {
        self.parts > 0
    }
}

/// What is left of someone's text.
pub fn clean_text(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(MAX_TEXT)
        .collect()
}

/// When a post from someone else ends, or `None` when it is over already. Goes by what they say, but
/// a wrong clock on their side can never make it last longer than a day from now.
pub fn ends_at(now: Millis, at: Millis, lasts: Millis) -> Option<Millis> {
    let lasts = lasts.clamp(0, LASTS);
    let claimed = at.min(now).saturating_add(lasts);
    (claimed > now).then_some(claimed)
}

/// The bit of a post that goes along with a reply.
pub fn quote_of(post: &Post) -> String {
    post.text.chars().take(QUOTE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_sees_it() {
        assert!(Audience::All.includes("a"));
        let only = Audience::Only(vec!["a".into()]);
        assert!(only.includes("a") && !only.includes("b"));
        let except = Audience::Except(vec!["a".into()]);
        assert!(!except.includes("a") && except.includes("b"));
    }

    #[test]
    fn a_wrong_clock_does_not_make_it_last() {
        const NOW: Millis = 1_000_000_000_000;
        assert_eq!(ends_at(NOW, NOW, LASTS), Some(NOW + LASTS));
        // Their clock is a year ahead: still a day.
        assert_eq!(ends_at(NOW, NOW + 365 * LASTS, LASTS), Some(NOW + LASTS));
        // They claim it lasts a month: still a day.
        assert_eq!(ends_at(NOW, NOW, 30 * LASTS), Some(NOW + LASTS));
        // Late: what is left of the day.
        assert_eq!(ends_at(NOW, NOW - LASTS + 5, LASTS), Some(NOW + 5));
        // Over already, or nonsense.
        assert_eq!(ends_at(NOW, NOW - LASTS, LASTS), None);
        assert_eq!(ends_at(NOW, 0, LASTS), None);
        assert_eq!(ends_at(NOW, NOW, -5), None);
        assert_eq!(ends_at(NOW, Millis::MIN, Millis::MAX), None);
    }

    #[test]
    fn text_is_cleaned_and_cut() {
        assert_eq!(clean_text("  hi\u{7}\nthere  "), "hi\nthere");
        assert_eq!(
            clean_text(&"x".repeat(MAX_TEXT + 5)).chars().count(),
            MAX_TEXT
        );
    }

    #[test]
    fn a_backdrop_from_someone_else_is_one_of_the_list() {
        for byte in 0..=255u8 {
            let word = Backdrop::from_byte(byte).word();
            assert!(Backdrop::ALL.iter().any(|b| b.word() == word));
        }
        for backdrop in Backdrop::ALL {
            assert_eq!(Backdrop::from_byte(backdrop.byte()), backdrop);
        }
    }
}
