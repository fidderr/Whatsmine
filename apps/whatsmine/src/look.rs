//! What a person can choose about how things look.

use whatsmine_core::Settings;
use whatsmine_core::rich::ChatPrefs;

/// Accent colours: the key, the colour on dark, the colour on light, the text on it.
pub const ACCENTS: &[(&str, &str, &str)] = &[
    ("", "#7cb0ff", "blue"),
    ("teal", "#3fc1a6", "teal"),
    ("violet", "#a78bfa", "violet"),
    ("pink", "#ec6fae", "pink"),
    ("amber", "#f0a93e", "amber"),
    ("white", "#f2f3f7", "white"),
];

/// Overrides the theme's accent for this person's choice.
pub fn accent_style(key: &str) -> String {
    match ACCENTS.iter().find(|(k, _, _)| *k == key) {
        Some((k, color, _)) if !k.is_empty() => format!(
            "--primary:{color};--primary-hover:{color};--read-tick:{color};--on-primary:#0b1526;"
        ),
        _ => String::new(),
    }
}

/// The root text size in pixels, which everything else scales with.
pub fn root_font_px(size: &str) -> u32 {
    match size {
        "small" => 14,
        "large" => 18,
        "huge" => 21,
        _ => 16,
    }
}

/// Wallpapers: the key, what shows behind the messages.
pub const WALLPAPERS: &[(&str, &str)] = &[
    ("", ""),
    (
        "dots",
        "background-color:var(--surface2);background-image:radial-gradient(var(--line) 1px,transparent 1.5px);background-size:14px 14px;",
    ),
    (
        "waves",
        "background-color:var(--surface2);background-image:repeating-radial-gradient(circle at 0 100%,transparent 0 7px,var(--line) 7px 8px);",
    ),
    (
        "dusk",
        "background-color:#3f3556;background-image:linear-gradient(160deg,#3f3556,#ee9b6c);",
    ),
    (
        "sea",
        "background-color:#1f5a4d;background-image:linear-gradient(160deg,#1f5a4d,#8fc3e8);",
    ),
];

/// What a chat looks like: its own look, or else what all chats have.
pub struct ChatStyle {
    /// For the part with the messages.
    pub messages: String,
    /// For the chat as a whole.
    pub class: String,
}

pub fn chat_style(settings: &Settings, prefs: &ChatPrefs) -> ChatStyle {
    let wallpaper = if prefs.wallpaper.is_empty() {
        settings.wallpaper.as_str()
    } else {
        prefs.wallpaper.as_str()
    };
    let color = if prefs.bubble_color.is_empty() {
        settings.bubble_color.as_str()
    } else {
        prefs.bubble_color.as_str()
    };
    let dim = if prefs.wallpaper.is_empty() {
        settings.wallpaper_dim
    } else {
        prefs.wallpaper_dim
    };
    let mut messages = WALLPAPERS
        .iter()
        .find(|(k, _)| *k == wallpaper)
        .map(|(_, css)| (*css).to_owned())
        .unwrap_or_default();
    if !messages.is_empty() && dim > 0 {
        messages.push_str(&format!(
            "box-shadow:inset 0 0 0 100vmax rgba(0,0,0,{:.2});",
            f32::from(dim.min(90)) / 100.0
        ));
    }
    if let Some((_, swatch, _)) = ACCENTS
        .iter()
        .find(|(k, _, _)| !k.is_empty() && *k == color)
    {
        messages.push_str(&format!(
            "--bubble-out:{swatch};--bubble-out-text:#0b1526;--bubble-out-meta:#0b1526;"
        ));
    }
    let mut class = String::new();
    if settings.bubble_square {
        class.push_str(" shape-square");
    }
    if settings.flat_layout {
        class.push_str(" flat");
    }
    ChatStyle { messages, class }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_with_its_own_look_keeps_it() {
        let settings = Settings {
            wallpaper: "dots".into(),
            ..Settings::default()
        };
        let own = ChatPrefs {
            wallpaper: "sea".into(),
            ..ChatPrefs::default()
        };
        assert!(chat_style(&settings, &own).messages.contains("#1f5a4d"));
        assert!(
            chat_style(&settings, &ChatPrefs::default())
                .messages
                .contains("radial-gradient")
        );
        assert!(accent_style("").is_empty());
        assert!(accent_style("teal").contains("#3fc1a6"));
        assert_eq!(root_font_px("normal"), 16);
    }
}
