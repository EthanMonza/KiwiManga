//! kiwi-en must be a full peer locale (all keys), and genuinely different
//! from en (slang everywhere, not a copy with two jokes).

use kiwimanga::i18n::I18n;

#[test]
fn kiwi_has_all_keys() {
    let i18n = I18n::load().unwrap();
    let mut en_keys = i18n.keys("en");
    let mut kiwi_keys = i18n.keys("kiwi-en");
    en_keys.sort();
    kiwi_keys.sort();
    assert_eq!(kiwi_keys, en_keys);
}

#[test]
fn kiwi_is_actually_kiwi() {
    let i18n = I18n::load().unwrap();
    // Language names (lname_*) are proper nouns and stay identical by design.
    let keys: Vec<String> =
        i18n.keys("en").into_iter().filter(|k| !k.starts_with("lname_")).collect();
    let same: Vec<String> =
        keys.iter().filter(|k| i18n.get("kiwi-en", k) == i18n.get("en", k)).cloned().collect();
    let diff = keys.len() - same.len();
    let pct = diff * 100 / keys.len().max(1);
    assert!(
        pct >= 90,
        "only {diff}/{} kiwi-en strings differ from en; identical: {same:?}",
        keys.len()
    );
    // Spot-check the slang markers required by the spec.
    let all = keys.iter().map(|k| i18n.get("kiwi-en", k)).collect::<Vec<_>>().join("\n");
    for marker in ["Chur", "sweet as", "yeah nah", "tu meke", "bro", "eh"] {
        assert!(all.contains(marker), "slang marker missing: {marker}");
    }
}
