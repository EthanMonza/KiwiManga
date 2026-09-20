//! Reply + inline keyboards. All labels via i18n.

use crate::bot::callbacks as cb;
use crate::i18n::{display_name, I18n, SUPPORTED};
use crate::sources::TitleInfo;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, KeyboardButton, KeyboardMarkup, ReplyMarkup,
};

fn btn(text: String, data: String) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(text, data)
}

fn nav_row(
    i18n: &I18n,
    loc: &str,
    page: usize,
    total_pages: usize,
    mk: &dyn Fn(usize) -> String,
) -> Vec<InlineKeyboardButton> {
    let mut row = Vec::new();
    if page > 0 {
        row.push(btn(i18n.get(loc, "btn_prev"), mk(page - 1)));
    }
    if page + 1 < total_pages {
        row.push(btn(i18n.get(loc, "btn_next"), mk(page + 1)));
    }
    row
}

pub fn as_inline(kb: InlineKeyboardMarkup) -> ReplyMarkup {
    ReplyMarkup::InlineKeyboard(kb)
}

/// Persistent reply menu of slash commands (commands are universal, no l10n).
pub fn reply_menu() -> ReplyMarkup {
    let kb = KeyboardMarkup {
        keyboard: vec![
            vec![KeyboardButton::new("/queue"), KeyboardButton::new("/status")],
            vec![KeyboardButton::new("/settings"), KeyboardButton::new("/language")],
            vec![KeyboardButton::new("/help"), KeyboardButton::new("/cancel")],
        ],
        is_persistent: false,
        resize_keyboard: Some(true),
        one_time_keyboard: None,
        input_field_placeholder: None,
        selective: None,
    };
    ReplyMarkup::Keyboard(kb)
}

pub fn language_list(i18n: &I18n) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let mut row = Vec::new();
    for code in SUPPORTED {
        row.push(btn(display_name(i18n, code), cb::lang(code)));
        if row.len() == 2 {
            rows.push(std::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows.push(vec![btn(i18n.get("en", "btn_close"), cb::CLOSE.to_string())]);
    InlineKeyboardMarkup::new(rows)
}

pub const SEARCH_PER_PAGE: usize = 5;

pub fn search_results(
    i18n: &I18n,
    loc: &str,
    sid: u32,
    page: usize,
    results: &[TitleInfo],
) -> InlineKeyboardMarkup {
    let total_pages = (results.len() + SEARCH_PER_PAGE - 1) / SEARCH_PER_PAGE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages - 1);
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let start = page * SEARCH_PER_PAGE;
    for (k, t) in results.iter().skip(start).take(SEARCH_PER_PAGE).enumerate() {
        let n = start + k + 1;
        let label = format!("{n}. {}", short(&t.title, 40));
        rows.push(vec![btn(label, cb::pick(sid, start + k))]);
    }
    let nav = nav_row(i18n, loc, page, total_pages, &|p| cb::search_page(sid, p));
    if !nav.is_empty() {
        rows.push(nav);
    }
    rows.push(vec![btn(i18n.get(loc, "btn_close"), cb::CLOSE.to_string())]);
    InlineKeyboardMarkup::new(rows)
}

pub fn title_options(i18n: &I18n, loc: &str, tid: u32) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![btn(i18n.get(loc, "opt_last10"), cb::last10(tid))],
        vec![btn(i18n.get(loc, "opt_range"), cb::range(tid))],
        vec![btn(i18n.get(loc, "opt_volumes"), cb::volumes(tid))],
        vec![btn(i18n.get(loc, "opt_single"), cb::single(tid))],
        vec![btn(i18n.get(loc, "btn_close"), cb::CLOSE.to_string())],
    ])
}

pub const VOLS_PER_PAGE: usize = 8;

pub fn volumes_page(
    i18n: &I18n,
    loc: &str,
    tid: u32,
    page: usize,
    volumes: &[String],
) -> InlineKeyboardMarkup {
    let total_pages = (volumes.len() + VOLS_PER_PAGE - 1) / VOLS_PER_PAGE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages - 1);
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let start = page * VOLS_PER_PAGE;
    let mut row = Vec::new();
    for (k, v) in volumes.iter().skip(start).take(VOLS_PER_PAGE).enumerate() {
        let label = i18n.render(loc, "volume_label", &[("v", v.clone())]);
        row.push(btn(label, cb::vol_pick(tid, start + k)));
        if row.len() == 2 {
            rows.push(std::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    let nav = nav_row(i18n, loc, page, total_pages, &|p| cb::vol_page(tid, p));
    if !nav.is_empty() {
        rows.push(nav);
    }
    rows.push(vec![
        btn(i18n.get(loc, "btn_back"), cb::back_options(tid)),
        btn(i18n.get(loc, "btn_close"), cb::CLOSE.to_string()),
    ]);
    InlineKeyboardMarkup::new(rows)
}

pub const CHS_PER_PAGE: usize = 8;

pub fn chapters_page(
    i18n: &I18n,
    loc: &str,
    tid: u32,
    page: usize,
    labels: &[String],
) -> InlineKeyboardMarkup {
    let total_pages = (labels.len() + CHS_PER_PAGE - 1) / CHS_PER_PAGE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages - 1);
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    let start = page * CHS_PER_PAGE;
    for (k, label) in labels.iter().skip(start).take(CHS_PER_PAGE).enumerate() {
        rows.push(vec![btn(short(label, 60), cb::ch_pick(tid, start + k))]);
    }
    let nav = nav_row(i18n, loc, page, total_pages, &|p| cb::ch_page(tid, p));
    if !nav.is_empty() {
        rows.push(nav);
    }
    rows.push(vec![
        btn(i18n.get(loc, "btn_back"), cb::back_options(tid)),
        btn(i18n.get(loc, "btn_close"), cb::CLOSE.to_string()),
    ]);
    InlineKeyboardMarkup::new(rows)
}

pub fn cancel_job(i18n: &I18n, loc: &str, job_id: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![btn(
        i18n.get(loc, "btn_cancel_job"),
        cb::cancel_job(job_id),
    )]])
}

pub fn cancel_pick(i18n: &I18n, loc: &str, jobs: &[(i64, String)]) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for (id, title) in jobs {
        rows.push(vec![btn(format!("#{id} {}", short(title, 40)), cb::cancel_job(*id))]);
    }
    rows.push(vec![btn(i18n.get(loc, "btn_close"), cb::CLOSE.to_string())]);
    InlineKeyboardMarkup::new(rows)
}

/// Chapter languages offered in /settings (+ auto). MangaDex language codes.
pub const CHAPTER_LANGS: &[&str] =
    &["en", "ru", "ja", "ko", "zh", "es", "fr", "de", "it", "ar", "tr"];

pub fn settings(i18n: &I18n, loc: &str, merged: bool, chapter_lang: Option<&str>) -> InlineKeyboardMarkup {
    let tick = |on: bool| if on { "✅ " } else { "" }.to_string();
    let merged_label = format!("{}{}", tick(merged), i18n.get(loc, "pack_merged_name"));
    let split_label = format!("{}{}", tick(!merged), i18n.get(loc, "pack_per_chapter_name"));
    let mut rows = vec![
        vec![btn(merged_label, cb::pack_mode(true))],
        vec![btn(split_label, cb::pack_mode(false))],
    ];
    // Chapter language grid: auto + 11 codes, 3 per row.
    let mut row = Vec::new();
    row.push(btn(
        format!("{}{}", tick(chapter_lang.is_none()), i18n.get(loc, "settings_chlang_auto_name")),
        cb::chapter_lang("auto"),
    ));
    rows.push(std::mem::take(&mut row));
    for code in CHAPTER_LANGS {
        let on = chapter_lang == Some(*code);
        row.push(btn(format!("{}{code}", tick(on)), cb::chapter_lang(code)));
        if row.len() == 3 {
            rows.push(std::mem::take(&mut row));
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows.push(vec![btn(i18n.get(loc, "btn_close"), cb::CLOSE.to_string())]);
    InlineKeyboardMarkup::new(rows)
}

fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}
