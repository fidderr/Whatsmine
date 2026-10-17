//! Translations, embedded as one flat JSON file per language.

use std::collections::HashMap;

pub type TranslationMap = HashMap<String, String>;

/// Supported languages: (code, name in that language).
pub const LANGUAGES: [(&str, &str); 5] = [
    ("en", "English"),
    ("nl", "Nederlands"),
    ("es", "Español"),
    ("fr", "Français"),
    ("de", "Deutsch"),
];

pub fn load_translations(code: &str) -> TranslationMap {
    let json = match code {
        "nl" => include_str!("nl.json"),
        "es" => include_str!("es.json"),
        "fr" => include_str!("fr.json"),
        "de" => include_str!("de.json"),
        _ => include_str!("en.json"),
    };
    serde_json::from_str(json).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A broken or incomplete file would silently show raw keys in the UI.
    #[test]
    fn every_language_parses_and_matches_english_keys() {
        let en = load_translations("en");
        assert!(!en.is_empty(), "en.json failed to parse");
        for (code, _) in LANGUAGES {
            let map = load_translations(code);
            let mut missing: Vec<_> = en.keys().filter(|k| !map.contains_key(*k)).collect();
            let mut extra: Vec<_> = map.keys().filter(|k| !en.contains_key(*k)).collect();
            missing.sort();
            extra.sort();
            assert!(missing.is_empty(), "{code} missing keys: {missing:?}");
            assert!(extra.is_empty(), "{code} has keys not in en: {extra:?}");
        }
    }

    /// Every key the code uses must exist in en.json.
    #[test]
    fn keys_used_in_code_exist() {
        let en = load_translations("en");
        let src = concat!(
            include_str!("../app.rs"),
            include_str!("../fmt.rs"),
            include_str!("../components/bottom_nav.rs"),
            include_str!("../components/confirm.rs"),
            include_str!("../components/net_banner.rs"),
            include_str!("../components/attachment.rs"),
            include_str!("../components/voice.rs"),
            include_str!("../components/camera.rs"),
            include_str!("../screens/onboarding.rs"),
            include_str!("../screens/chats.rs"),
            include_str!("../screens/chat.rs"),
            include_str!("../screens/add.rs"),
            include_str!("../screens/requests.rs"),
            include_str!("../screens/contact.rs"),
            include_str!("../screens/settings.rs"),
        );
        let mut missing = Vec::new();
        for (i, _) in src.match_indices("t(&tr, ") {
            let rest = &src[i + 7..];
            let rest = rest.trim_start_matches('\\');
            if let Some(stripped) = rest.strip_prefix('"') {
                let key: String = stripped
                    .chars()
                    .take_while(|c| *c != '"' && *c != '\\')
                    .collect();
                if !en.contains_key(&key) {
                    missing.push(key);
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "keys missing from en.json: {missing:?}");
    }
}
