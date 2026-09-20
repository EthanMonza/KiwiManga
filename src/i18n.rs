//! JSON-based i18n. All bot strings live in `locales/<code>.json` and are
//! embedded into the binary. Missing key -> English. Unknown locale -> English.
//! No hardcoded user-facing strings outside this module.

use crate::error::{BotError, Result};
use std::collections::HashMap;

/// All supported locale codes, in /language display order.
pub const SUPPORTED: &[&str] = &[
    "en", "kiwi-en", "ru", "tr", "de", "da", "no", "sv", "fr", "ar", "ja", "ko", "zh", "fi", "mi",
    "es", "it",
];

pub const FALLBACK: &str = "en";

macro_rules! locale_file {
    ($code:literal) => {
        ($code, include_str!(concat!("../locales/", $code, ".json")))
    };
}

const LOCALE_FILES: &[(&str, &str)] = &[
    locale_file!("en"),
    locale_file!("kiwi-en"),
    locale_file!("ru"),
    locale_file!("tr"),
    locale_file!("de"),
    locale_file!("da"),
    locale_file!("no"),
    locale_file!("sv"),
    locale_file!("fr"),
    locale_file!("ar"),
    locale_file!("ja"),
    locale_file!("ko"),
    locale_file!("zh"),
    locale_file!("fi"),
    locale_file!("mi"),
    locale_file!("es"),
    locale_file!("it"),
];

#[derive(Debug, Clone)]
pub struct I18n {
    maps: HashMap<String, HashMap<String, String>>,
}

impl I18n {
    pub fn load() -> Result<Self> {
        let mut maps = HashMap::new();
        for (code, text) in LOCALE_FILES {
            let map: HashMap<String, String> = serde_json::from_str(text)
                .map_err(|e| BotError::I18n(format!("locales/{code}.json: {e}")))?;
            maps.insert((*code).to_string(), map);
        }
        if !maps.contains_key(FALLBACK) {
            return Err(BotError::I18n("fallback locale 'en' is missing".to_string()));
        }
        Ok(Self { maps })
    }

    /// Raw lookup with fallback: locale -> en -> key itself (never panics).
    pub fn get(&self, locale: &str, key: &str) -> String {
        if let Some(map) = self.maps.get(locale) {
            if let Some(v) = map.get(key) {
                return v.clone();
            }
        }
        if locale != FALLBACK {
            if let Some(map) = self.maps.get(FALLBACK) {
                if let Some(v) = map.get(key) {
                    return v.clone();
                }
            }
        }
        key.to_string()
    }

    /// Lookup + `{name}` placeholder substitution.
    pub fn render(&self, locale: &str, key: &str, params: &[(&str, String)]) -> String {
        let mut out = self.get(locale, key);
        for (k, v) in params {
            let ph = format!("{{{k}}}");
            out = out.replace(&ph, v);
        }
        out
    }

    /// Number of keys in a locale (used by tests / startup self-check).
    pub fn key_count(&self, locale: &str) -> usize {
        self.maps.get(locale).map(|m| m.len()).unwrap_or(0)
    }

    pub fn keys(&self, locale: &str) -> Vec<String> {
        self.maps.get(locale).map(|m| m.keys().cloned().collect()).unwrap_or_default()
    }
}

/// Normalize arbitrary user input to a supported locale code.
pub fn normalize_locale(code: &str) -> &str {
    let c = code.trim().to_lowercase();
    for s in SUPPORTED {
        if *s == c {
            return s;
        }
    }
    FALLBACK
}

pub fn is_supported(code: &str) -> bool {
    SUPPORTED.contains(&code.trim().to_lowercase().as_str())
}

/// Display name of a locale (native name; stored in en.json `lname_*` keys so
/// every locale falls back to the same recognizable names).
pub fn display_name(i18n: &I18n, code: &str) -> String {
    i18n.get(FALLBACK, &format!("lname_{code}"))
}
