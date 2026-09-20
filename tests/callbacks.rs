//! Callback codec: roundtrip + strict 64-byte Telegram limit, incl. extremes.

use kiwimanga::bot::callbacks::{self as cb, Callback};

fn assert_fits(s: &str) {
    assert!(s.len() <= 64, "callback too long ({}B): {s}", s.len());
}

#[test]
fn all_constructors_fit_even_at_extremes() {
    let big = usize::MAX;
    let payloads = vec![
        cb::search_page(u32::MAX, big),
        cb::pick(u32::MAX, big),
        cb::last10(u32::MAX),
        cb::range(u32::MAX),
        cb::volumes(u32::MAX),
        cb::single(u32::MAX),
        cb::back_options(u32::MAX),
        cb::vol_page(u32::MAX, big),
        cb::vol_pick(u32::MAX, big),
        cb::ch_page(u32::MAX, big),
        cb::ch_pick(u32::MAX, big),
        cb::cancel_job(i64::MAX),
        cb::cancel_job(i64::MIN),
        cb::pack_mode(true),
        cb::pack_mode(false),
        cb::chapter_lang("auto"),
        cb::chapter_lang("en"),
        cb::lang("kiwi-en"),
        cb::CLOSE.to_string(),
    ];
    for s in &payloads {
        assert_fits(s);
    }
}

#[test]
fn roundtrip_all_variants() {
    let cases: Vec<(String, Callback)> = vec![
        (cb::search_page(11, 2), Callback::SearchPage { sid: 11, page: 2 }),
        (cb::pick(11, 4), Callback::Pick { sid: 11, idx: 4 }),
        (cb::last10(9), Callback::Last10 { tid: 9 }),
        (cb::range(9), Callback::Range { tid: 9 }),
        (cb::volumes(9), Callback::Volumes { tid: 9 }),
        (cb::single(9), Callback::Single { tid: 9 }),
        (cb::back_options(9), Callback::BackOptions { tid: 9 }),
        (cb::vol_page(9, 1), Callback::VolPage { tid: 9, page: 1 }),
        (cb::vol_pick(9, 6), Callback::VolPick { tid: 9, idx: 6 }),
        (cb::ch_page(9, 3), Callback::ChPage { tid: 9, page: 3 }),
        (cb::ch_pick(9, 120), Callback::ChPick { tid: 9, idx: 120 }),
        (cb::cancel_job(777), Callback::CancelJob { job_id: 777 }),
        (cb::pack_mode(true), Callback::PackMode { merged: true }),
        (cb::pack_mode(false), Callback::PackMode { merged: false }),
        (
            cb::chapter_lang("ru"),
            Callback::ChapterLang { code: "ru".to_string() },
        ),
        (cb::lang("mi"), Callback::Lang { code: "mi".to_string() }),
        (cb::CLOSE.to_string(), Callback::Close),
    ];
    for (s, want) in cases {
        assert_eq!(cb::parse(&s), Some(want), "payload: {s}");
    }
}

#[test]
fn pagination_sequences_stay_short() {
    // Simulate deep pagination on a huge title list.
    for page in [0usize, 1, 50, 9999] {
        for sid in [1u32, 42424242] {
            assert_fits(&cb::search_page(sid, page));
            assert_fits(&cb::ch_page(sid, page));
            assert_fits(&cb::vol_page(sid, page));
        }
    }
}

#[test]
fn garbage_rejected() {
    let long = "x".repeat(65);
    for s in [
        "",
        "nope",
        "sp:",
        "sp:1",
        "sp:x:y",
        "pk:1:2:3",
        "cx:notanumber",
        "stpk:maybe",
        "m10:1:2",
        "cls:extra",
        long.as_str(),
    ] {
        assert_eq!(cb::parse(s), None, "payload: {s}");
    }
}
