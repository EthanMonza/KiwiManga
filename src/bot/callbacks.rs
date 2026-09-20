//! Compact callback_data codec (Telegram limit: 64 bytes).
//! Only short numeric ids travel in buttons; everything else lives in TTL caches.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callback {
    SearchPage { sid: u32, page: usize },
    Pick { sid: u32, idx: usize },
    Last10 { tid: u32 },
    Range { tid: u32 },
    Volumes { tid: u32 },
    Single { tid: u32 },
    BackOptions { tid: u32 },
    VolPage { tid: u32, page: usize },
    VolPick { tid: u32, idx: usize },
    ChPage { tid: u32, page: usize },
    ChPick { tid: u32, idx: usize },
    CancelJob { job_id: i64 },
    PackMode { merged: bool },
    ChapterLang { code: String },
    Lang { code: String },
    Close,
}

pub const CLOSE: &str = "cls";

pub fn search_page(sid: u32, page: usize) -> String {
    format!("sp:{sid}:{page}")
}
pub fn pick(sid: u32, idx: usize) -> String {
    format!("pk:{sid}:{idx}")
}
pub fn last10(tid: u32) -> String {
    format!("m10:{tid}")
}
pub fn range(tid: u32) -> String {
    format!("mrg:{tid}")
}
pub fn volumes(tid: u32) -> String {
    format!("mvl:{tid}")
}
pub fn single(tid: u32) -> String {
    format!("mch:{tid}")
}
pub fn back_options(tid: u32) -> String {
    format!("bk:{tid}")
}
pub fn vol_page(tid: u32, page: usize) -> String {
    format!("vp:{tid}:{page}")
}
pub fn vol_pick(tid: u32, idx: usize) -> String {
    format!("vk:{tid}:{idx}")
}
pub fn ch_page(tid: u32, page: usize) -> String {
    format!("cp:{tid}:{page}")
}
pub fn ch_pick(tid: u32, idx: usize) -> String {
    format!("ck:{tid}:{idx}")
}
pub fn cancel_job(job_id: i64) -> String {
    format!("cx:{job_id}")
}
pub fn pack_mode(merged: bool) -> String {
    format!("stpk:{}", if merged { "m" } else { "p" })
}
pub fn chapter_lang(code: &str) -> String {
    format!("stcl:{code}")
}
pub fn lang(code: &str) -> String {
    format!("lg:{code}")
}

fn parse_u32(s: &str) -> Option<u32> {
    s.parse::<u32>().ok()
}

fn parse_usize(s: &str) -> Option<usize> {
    s.parse::<usize>().ok()
}

fn parse_i64(s: &str) -> Option<i64> {
    s.parse::<i64>().ok()
}

pub fn parse(data: &str) -> Option<Callback> {
    let mut it = data.split(':');
    let head = it.next()?;
    let a = it.next();
    let b = it.next();
    if it.next().is_some() {
        return None; // more than 3 segments
    }
    match head {
        "sp" => Some(Callback::SearchPage { sid: parse_u32(a?)?, page: parse_usize(b?)? }),
        "pk" => Some(Callback::Pick { sid: parse_u32(a?)?, idx: parse_usize(b?)? }),
        "m10" if b.is_none() => Some(Callback::Last10 { tid: parse_u32(a?)? }),
        "mrg" if b.is_none() => Some(Callback::Range { tid: parse_u32(a?)? }),
        "mvl" if b.is_none() => Some(Callback::Volumes { tid: parse_u32(a?)? }),
        "mch" if b.is_none() => Some(Callback::Single { tid: parse_u32(a?)? }),
        "bk" if b.is_none() => Some(Callback::BackOptions { tid: parse_u32(a?)? }),
        "vp" => Some(Callback::VolPage { tid: parse_u32(a?)?, page: parse_usize(b?)? }),
        "vk" => Some(Callback::VolPick { tid: parse_u32(a?)?, idx: parse_usize(b?)? }),
        "cp" => Some(Callback::ChPage { tid: parse_u32(a?)?, page: parse_usize(b?)? }),
        "ck" => Some(Callback::ChPick { tid: parse_u32(a?)?, idx: parse_usize(b?)? }),
        "cx" if b.is_none() => Some(Callback::CancelJob { job_id: parse_i64(a?)? }),
        "stpk" if b.is_none() => match a? {
            "m" => Some(Callback::PackMode { merged: true }),
            "p" => Some(Callback::PackMode { merged: false }),
            _ => None,
        },
        "stcl" if b.is_none() => Some(Callback::ChapterLang { code: a?.to_string() }),
        "lg" if b.is_none() => Some(Callback::Lang { code: a?.to_string() }),
        "cls" if a.is_none() => Some(Callback::Close),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let cases = vec![
            (search_page(1, 2), Callback::SearchPage { sid: 1, page: 2 }),
            (pick(99, 7), Callback::Pick { sid: 99, idx: 7 }),
            (last10(5), Callback::Last10 { tid: 5 }),
            (range(5), Callback::Range { tid: 5 }),
            (volumes(5), Callback::Volumes { tid: 5 }),
            (single(5), Callback::Single { tid: 5 }),
            (back_options(5), Callback::BackOptions { tid: 5 }),
            (vol_page(5, 3), Callback::VolPage { tid: 5, page: 3 }),
            (vol_pick(5, 3), Callback::VolPick { tid: 5, idx: 3 }),
            (ch_page(5, 3), Callback::ChPage { tid: 5, page: 3 }),
            (ch_pick(5, 3), Callback::ChPick { tid: 5, idx: 3 }),
            (cancel_job(12345), Callback::CancelJob { job_id: 12345 }),
            (pack_mode(true), Callback::PackMode { merged: true }),
            (pack_mode(false), Callback::PackMode { merged: false }),
            (chapter_lang("auto"), Callback::ChapterLang { code: "auto".to_string() }),
            (lang("kiwi-en"), Callback::Lang { code: "kiwi-en".to_string() }),
            (CLOSE.to_string(), Callback::Close),
        ];
        for (s, want) in cases {
            assert_eq!(parse(&s), Some(want), "payload {s}");
            assert!(s.len() <= 64, "payload too long: {s}");
        }
    }

    #[test]
    fn max_values_still_fit_64_bytes() {
        for s in [
            search_page(u32::MAX, usize::MAX),
            ch_pick(u32::MAX, usize::MAX),
            cancel_job(i64::MAX),
        ] {
            assert!(s.len() <= 64, "payload too long: {s}");
        }
    }

    #[test]
    fn garbage_rejected() {
        for s in ["", "xx", "sp:1", "sp:a:b", "sp:1:2:3", "cx:", "stpk:x"] {
            assert_eq!(parse(s), None, "payload {s}");
        }
        // "lg:" splits into ["lg", ""] — empty code parses as Some("").
        // Empty codes are rejected later by locale validation; keep codec total.
        assert_eq!(parse("lg:"), Some(Callback::Lang { code: String::new() }));
    }
}
