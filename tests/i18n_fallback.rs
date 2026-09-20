//! i18n fallback: unknown locale -> en; params render; unknown key echoes.

use kiwimanga::i18n::I18n;

#[test]
fn loads_all_locales() {
    let i18n = I18n::load().unwrap();
    assert_eq!(i18n.key_count("en"), 107);
    assert_eq!(i18n.key_count("kiwi-en"), 107);
    assert_eq!(i18n.key_count("mi"), 107);
}

#[test]
fn unknown_locale_falls_back_to_en() {
    let i18n = I18n::load().unwrap();
    assert_eq!(i18n.get("xx", "btn_close"), i18n.get("en", "btn_close"));
    assert_eq!(i18n.get("", "btn_close"), i18n.get("en", "btn_close"));
}

#[test]
fn unknown_key_echoes() {
    let i18n = I18n::load().unwrap();
    assert_eq!(i18n.get("ru", "no_such_key"), "no_such_key");
}

#[test]
fn render_replaces_params() {
    let i18n = I18n::load().unwrap();
    let s = i18n.render(
        "ru",
        "job_accepted",
        &[("id", "7".to_string()), ("n", "2".to_string())],
    );
    assert!(s.contains("#7"), "id missing: {s}");
    assert!(s.contains("<b>2</b>"), "position missing: {s}");
    assert!(!s.contains("{id}") && !s.contains("{n}"), "raw placeholders left: {s}");
}

#[test]
fn every_locale_has_every_en_key() {
    let i18n = I18n::load().unwrap();
    let mut en_keys = i18n.keys("en");
    en_keys.sort();
    for loc in kiwimanga::i18n::SUPPORTED {
        let mut keys = i18n.keys(loc);
        keys.sort();
        assert_eq!(keys, en_keys, "key set mismatch in {loc}");
    }
}

/// Extract `{name}` placeholders (test-only mini-parser).
fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('}') {
            out.push(rest[..close].to_string());
            rest = &rest[close + 1..];
        } else {
            break;
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn placeholders_match_en_in_every_locale() {
    let i18n = I18n::load().unwrap();
    for key in i18n.keys("en") {
        let want = placeholders(&i18n.get("en", &key));
        for loc in kiwimanga::i18n::SUPPORTED {
            let got = placeholders(&i18n.get(loc, &key));
            assert_eq!(got, want, "placeholder mismatch {loc}::{key}");
        }
    }
}
