//! Source adapters behind one `trait Source`.
//!
//! `async_trait` is deliberately NOT used (crate budget): async fns in traits
//! are not dyn-compatible, so runtime dispatch goes through the [`AnySource`]
//! enum. Adding a source = new module + one enum variant.

pub mod mangadex;
pub mod ranobelib;
pub mod stub;

pub use mangadex::MangaDexSource;
pub use ranobelib::RanobeLibSource;
pub use stub::StubSource;

use crate::error::{BotError, Result};
use serde::{Deserialize, Serialize};

pub const MANGADEX: &str = "mangadex";
pub const RANOBELIB: &str = "ranobelib";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChapterKind {
    Manga,
    Ranobe,
}

/// Title info: filled partially by `search`, fully by `title_card`.
#[derive(Debug, Clone)]
pub struct TitleInfo {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub alt_titles: Vec<String>,
    pub orig_lang: String,
    pub langs: Vec<String>,
    pub description: String,
    pub url: String,
    pub kind: ChapterKind,
    pub last_volume: Option<String>,
    pub last_chapter: Option<String>,
}

impl TitleInfo {
    /// All names for cross-language matching: main + alt titles.
    pub fn all_names(&self) -> Vec<&str> {
        let mut v = vec![self.title.as_str()];
        v.extend(self.alt_titles.iter().map(|s| s.as_str()));
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChapterInfo {
    pub id: String,
    pub volume: Option<String>,
    pub number: Option<String>,
    pub title: Option<String>,
    pub lang: String,
    pub pages: u32,
}

impl ChapterInfo {
    /// Short human label: "12", "12.5 — Title", "oneshot", ...
    pub fn short_label(&self) -> String {
        let num = self.number.clone().unwrap_or_else(|| "oneshot".to_string());
        match &self.title {
            Some(t) if !t.trim().is_empty() && t.trim() != num.trim() => format!("{num} — {}", t.trim()),
            _ => num,
        }
    }
}

#[derive(Debug)]
pub struct PageData {
    pub ext: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum ChapterPayload {
    Manga { pages: Vec<PageData> },
    Text { heading: String, paragraphs: Vec<String> },
}

pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;
    fn kind(&self) -> ChapterKind;

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<TitleInfo>>;
    async fn title_card(&self, id: &str) -> Result<TitleInfo>;
    async fn list_chapters(&self, id: &str, lang: &str) -> Result<Vec<ChapterInfo>>;
    async fn download_chapter(
        &self,
        title_id: &str,
        chapter: &ChapterInfo,
    ) -> Result<ChapterPayload>;
}

/// Runtime-polymorphic source handle (see module docs for why an enum).
#[derive(Debug, Clone)]
pub enum AnySource {
    MangaDex(MangaDexSource),
    RanobeLib(RanobeLibSource),
    Stub(StubSource),
}

impl AnySource {
    pub fn name(&self) -> &'static str {
        match self {
            Self::MangaDex(s) => s.name(),
            Self::RanobeLib(s) => s.name(),
            Self::Stub(s) => s.name(),
        }
    }

    pub fn kind(&self) -> ChapterKind {
        match self {
            Self::MangaDex(s) => s.kind(),
            Self::RanobeLib(s) => s.kind(),
            Self::Stub(s) => s.kind(),
        }
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<TitleInfo>> {
        match self {
            Self::MangaDex(s) => s.search(query, limit).await,
            Self::RanobeLib(s) => s.search(query, limit).await,
            Self::Stub(s) => s.search(query, limit).await,
        }
    }

    pub async fn title_card(&self, id: &str) -> Result<TitleInfo> {
        match self {
            Self::MangaDex(s) => s.title_card(id).await,
            Self::RanobeLib(s) => s.title_card(id).await,
            Self::Stub(s) => s.title_card(id).await,
        }
    }

    pub async fn list_chapters(&self, id: &str, lang: &str) -> Result<Vec<ChapterInfo>> {
        match self {
            Self::MangaDex(s) => s.list_chapters(id, lang).await,
            Self::RanobeLib(s) => s.list_chapters(id, lang).await,
            Self::Stub(s) => s.list_chapters(id, lang).await,
        }
    }

    pub async fn download_chapter(
        &self,
        title_id: &str,
        chapter: &ChapterInfo,
    ) -> Result<ChapterPayload> {
        match self {
            Self::MangaDex(s) => s.download_chapter(title_id, chapter).await,
            Self::RanobeLib(s) => s.download_chapter(title_id, chapter).await,
            Self::Stub(s) => s.download_chapter(title_id, chapter).await,
        }
    }
}

// ------------------------------------------------------------ helpers ---

/// Parse "12", "12.5", "v3" etc. for ordering; garbage -> -1 (sorts first).
pub fn parse_num(s: Option<&str>) -> f64 {
    let raw = s.unwrap_or("").trim();
    if raw.is_empty() {
        return -1.0;
    }
    // Take the leading numeric run ("v12.5x" -> "12.5").
    let mut num = String::new();
    let mut dot = false;
    for ch in raw.chars() {
        if ch.is_ascii_digit() {
            num.push(ch);
        } else if ch == '.' && !dot && !num.is_empty() {
            dot = true;
            num.push(ch);
        } else if !num.is_empty() {
            break;
        }
    }
    num.parse::<f64>().unwrap_or(-1.0)
}

/// In-place ascending sort: volume, then chapter, then id (stable).
pub fn sort_chapters(chapters: &mut Vec<ChapterInfo>) {
    chapters.sort_by(|a, b| {
        parse_num(a.volume.as_deref())
            .total_cmp(&parse_num(b.volume.as_deref()))
            .then(parse_num(a.number.as_deref()).total_cmp(&parse_num(b.number.as_deref())))
            .then(a.id.cmp(&b.id))
    });
}

/// Distinct non-empty volumes, numerically sorted.
pub fn distinct_volumes(chapters: &[ChapterInfo]) -> Vec<String> {
    let mut vols: Vec<String> = chapters.iter().filter_map(|c| c.volume.clone()).collect();
    vols.sort_by(|a, b| parse_num(Some(a)).total_cmp(&parse_num(Some(b))));
    vols.dedup();
    vols.retain(|v| !v.trim().is_empty());
    vols
}

pub fn not_found(source: &str, what: &str) -> BotError {
    BotError::NotFound(format!("{source}: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: &str, v: Option<&str>, n: Option<&str>) -> ChapterInfo {
        ChapterInfo {
            id: id.to_string(),
            volume: v.map(|s| s.to_string()),
            number: n.map(|s| s.to_string()),
            title: None,
            lang: "en".to_string(),
            pages: 10,
        }
    }

    #[test]
    fn parse_num_cases() {
        assert_eq!(parse_num(Some("12")), 12.0);
        assert_eq!(parse_num(Some("12.5")), 12.5);
        assert_eq!(parse_num(Some("v3")), 3.0);
        assert_eq!(parse_num(None), -1.0);
        assert_eq!(parse_num(Some("")), -1.0);
        assert_eq!(parse_num(Some("oneshot")), -1.0);
    }

    #[test]
    fn chapters_sort_ascending() {
        let mut v = vec![
            ch("c", Some("1"), Some("10")),
            ch("a", Some("1"), Some("2")),
            ch("b", Some("2"), Some("1")),
        ];
        sort_chapters(&mut v);
        assert_eq!(v.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["a", "c", "b"]);
    }

    #[test]
    fn volumes_distinct_sorted() {
        let v = vec![
            ch("a", Some("10"), Some("1")),
            ch("b", Some("2"), Some("1")),
            ch("c", Some("2"), Some("2")),
            ch("d", None, Some("1")),
        ];
        assert_eq!(distinct_volumes(&v), vec!["2".to_string(), "10".to_string()]);
    }
}
