//! MangaDex adapter (official API v5, no auth needed for catalogue reads).
//! Polite global rate limit + identifying User-Agent per their rules.

use crate::error::{BotError, Result};
use crate::http::HttpClient;
use crate::ratelimit::RateLimiter;
use crate::sources::{
    not_found, sort_chapters, ChapterInfo, ChapterKind, ChapterPayload, PageData, Source, TitleInfo,
    MANGADEX,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

const FEED_LIMIT: usize = 500;
const MAX_FEED_PAGES: usize = 10; // 5000 chapters safety cap

#[derive(Debug, Clone)]
pub struct MangaDexSource {
    http: HttpClient,
    limiter: Arc<RateLimiter>,
    base: String,
}

impl MangaDexSource {
    pub fn new(http: HttpClient, limiter: Arc<RateLimiter>, base: String) -> Self {
        Self { http, limiter, base }
    }

    fn ratings() -> Vec<(String, String)> {
        ["safe", "suggestive", "erotica", "pornographic"]
            .into_iter()
            .map(|r| ("contentRating[]".to_string(), r.to_string()))
            .collect()
    }
}

impl Source for MangaDexSource {
    fn name(&self) -> &'static str {
        MANGADEX
    }

    fn kind(&self) -> ChapterKind {
        ChapterKind::Manga
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<TitleInfo>> {
        let url = format!("{}/manga", self.base);
        let mut q: Vec<(String, String)> = vec![
            ("title".to_string(), query.to_string()),
            ("limit".to_string(), limit.min(25).to_string()),
            ("offset".to_string(), "0".to_string()),
            ("order[relevance]".to_string(), "desc".to_string()),
        ];
        q.extend(Self::ratings());
        let resp: MdList<MdManga> =
            self.http.get_json(&url, &q, &self.limiter, MANGADEX).await?;
        Ok(resp.data.into_iter().map(md_to_title).collect())
    }

    async fn title_card(&self, id: &str) -> Result<TitleInfo> {
        let url = format!("{}/manga/{id}", self.base);
        let q: Vec<(String, String)> = Self::ratings();
        let resp: MdOne<MdManga> = self.http.get_json(&url, &q, &self.limiter, MANGADEX).await?;
        Ok(md_to_title(resp.data))
    }

    async fn list_chapters(&self, id: &str, lang: &str) -> Result<Vec<ChapterInfo>> {
        let url = format!("{}/manga/{id}/feed", self.base);
        let mut out = Vec::new();
        for page in 0..MAX_FEED_PAGES {
            let mut q: Vec<(String, String)> = vec![
                ("limit".to_string(), FEED_LIMIT.to_string()),
                ("offset".to_string(), (page * FEED_LIMIT).to_string()),
                ("translatedLanguage[]".to_string(), lang.to_string()),
                ("order[volume]".to_string(), "asc".to_string()),
                ("order[chapter]".to_string(), "asc".to_string()),
            ];
            q.extend(Self::ratings());
            let resp: MdList<MdChapter> =
                self.http.get_json(&url, &q, &self.limiter, MANGADEX).await?;
            if resp.data.is_empty() {
                break;
            }
            let got = resp.data.len();
            for c in resp.data {
                // Skip external/unavailable chapters (no pages to download).
                if c.attributes.pages == 0 || c.attributes.external_url.is_some() {
                    continue;
                }
                out.push(ChapterInfo {
                    id: c.id,
                    volume: c.attributes.volume,
                    number: c.attributes.chapter,
                    title: c.attributes.title,
                    lang: c.attributes.translated_language,
                    pages: c.attributes.pages,
                });
            }
            if got < FEED_LIMIT {
                break;
            }
        }
        sort_chapters(&mut out);
        Ok(out)
    }

    async fn download_chapter(
        &self,
        _title_id: &str,
        chapter: &ChapterInfo,
    ) -> Result<ChapterPayload> {
        let url = format!("{}/at-home/server/{}", self.base, chapter.id);
        let empty: [(&str, &str); 0] = [];
        let resp: AtHome = self.http.get_json(&url, &empty, &self.limiter, MANGADEX).await?;
        if resp.chapter.data.is_empty() {
            return Err(not_found(MANGADEX, &format!("pages of chapter {}", chapter.id)));
        }
        let mut pages = Vec::with_capacity(resp.chapter.data.len());
        for file in &resp.chapter.data {
            let img = format!("{}/data/{}/{}", resp.base_url, resp.chapter.hash, file);
            let bytes = self.http.get_bytes(&img, &self.limiter, MANGADEX).await.map_err(|e| {
                match e {
                    BotError::NotFound(_) => {
                        BotError::NotFound(format!("{MANGADEX}: page {file} of chapter {}", chapter.id))
                    }
                    other => other,
                }
            })?;
            pages.push(PageData { ext: ext_of(file).to_string(), bytes });
        }
        Ok(ChapterPayload::Manga { pages })
    }
}

fn ext_of(file: &str) -> &str {
    file.rsplit('.').next().filter(|e| e.len() <= 5 && !e.is_empty()).unwrap_or("jpg")
}

fn pick_localized(map: &HashMap<String, String>, prefer: &[&str]) -> String {
    for p in prefer {
        if let Some(v) = map.get(*p) {
            if !v.trim().is_empty() {
                return v.clone();
            }
        }
    }
    map.values().find(|v| !v.trim().is_empty()).cloned().unwrap_or_default()
}

fn md_to_title(m: MdManga) -> TitleInfo {
    let a = m.attributes;
    let title = pick_localized(&a.title, &["en", &a.original_language.clone().unwrap_or_default()]);
    let title = if title.is_empty() { m.id.clone() } else { title };
    let mut alt: Vec<String> = Vec::new();
    for map in &a.alt_titles {
        alt.extend(map.values().cloned());
    }
    alt.retain(|s| !s.trim().is_empty() && *s != title);
    alt.sort();
    alt.dedup();
    TitleInfo {
        source: MANGADEX.to_string(),
        source_id: m.id.clone(),
        title,
        alt_titles: alt,
        orig_lang: a.original_language.unwrap_or_else(|| "ja".to_string()),
        langs: a.available_translated_languages.unwrap_or_default(),
        description: pick_localized(&a.description, &["en"]),
        url: format!("https://mangadex.org/title/{}", m.id),
        kind: ChapterKind::Manga,
        last_volume: a.last_volume,
        last_chapter: a.last_chapter,
    }
}

// ------------------------------------------------------------- wire DTOs ---

#[derive(Debug, Deserialize)]
struct MdList<T> {
    #[serde(default)]
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct MdOne<T> {
    data: T,
}

#[derive(Debug, Default, Deserialize)]
struct MdManga {
    id: String,
    attributes: MdMangaAttr,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MdMangaAttr {
    #[serde(default)]
    title: HashMap<String, String>,
    #[serde(default)]
    alt_titles: Vec<HashMap<String, String>>,
    #[serde(default)]
    description: HashMap<String, String>,
    #[serde(default)]
    original_language: Option<String>,
    #[serde(default)]
    available_translated_languages: Option<Vec<String>>,
    #[serde(default)]
    last_volume: Option<String>,
    #[serde(default)]
    last_chapter: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct MdChapter {
    id: String,
    attributes: MdChapterAttr,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MdChapterAttr {
    #[serde(default)]
    volume: Option<String>,
    #[serde(default)]
    chapter: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    translated_language: String,
    #[serde(default)]
    pages: u32,
    #[serde(default)]
    external_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AtHome {
    base_url: String,
    chapter: AtHomeChapter,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AtHomeChapter {
    hash: String,
    #[serde(default)]
    data: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_detection() {
        assert_eq!(ext_of("x123.jpg"), "jpg");
        assert_eq!(ext_of("noext"), "jpg");
        assert_eq!(ext_of("a.toolongext"), "jpg");
    }

    #[test]
    fn localized_pick() {
        let mut m = HashMap::new();
        m.insert("ja".to_string(), "ワンピース".to_string());
        m.insert("en".to_string(), "One Piece".to_string());
        assert_eq!(pick_localized(&m, &["en"]), "One Piece");
        let any = pick_localized(&m, &["ru"]);
        assert!(any == "One Piece" || any == "ワンピース", "got {any}");
    }
}
