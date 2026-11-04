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

    fn placeholders(text: &str) -> Vec<&str> {
        let mut found: Vec<&str> = text
            .split('{')
            .skip(1)
            .filter_map(|rest| rest.split('}').next())
            .collect();
        found.sort_unstable();
        found
    }

    /// A placeholder a translation lacks would never be filled in, and one with another name
    /// would show as `{name}`.
    #[test]
    fn every_translation_has_the_placeholders_of_the_english() {
        let en = load_translations("en");
        for (code, _) in LANGUAGES {
            for (key, text) in load_translations(code) {
                assert_eq!(
                    placeholders(&text),
                    placeholders(&en[&key]),
                    "{code} {key}: {text}"
                );
            }
        }
    }

    fn rust_files(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") {
                found.push(path);
            }
        }
    }

    /// A key that does not exist shows on screen as the key.
    #[test]
    fn every_key_the_code_names_exists() {
        let en = load_translations("en");
        let groups: std::collections::HashSet<&str> =
            en.keys().filter_map(|key| key.split('.').next()).collect();
        let mut files = Vec::new();
        rust_files(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(files.len() > 20, "found {} files", files.len());
        let mut missing = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            // Inside `{t(&tr, \"key\")}` the quotes are escaped, so drop the escapes first.
            let code = code.replace("\\\"", "\"");
            // Between quotes: every second piece.
            for quoted in code.split('"').skip(1).step_by(2) {
                let looks_like_key = quoted.contains('.')
                    && !quoted.ends_with('.')
                    && !quoted.ends_with(".js")
                    && quoted.chars().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'
                    })
                    && quoted.split('.').next().is_some_and(|g| groups.contains(g));
                if looks_like_key && !en.contains_key(quoted) {
                    missing.push(format!("{}: {quoted}", file.display()));
                }
            }
        }
        assert!(missing.is_empty(), "not in en.json: {missing:#?}");
    }
}
