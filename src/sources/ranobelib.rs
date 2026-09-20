//! RanobeLib adapter (public JSON API at api.cdnlibs.org, site_id=3).
//! Russian-only translations: chapter language is always `ru`.

use crate::error::Result;
use crate::http::HttpClient;
use crate::ratelimit::RateLimiter;
use crate::sources::{
    not_found, sort_chapters, ChapterInfo, ChapterKind, ChapterPayload, Source, TitleInfo, RANOBELIB,
};
use serde::Deserialize;
use std::sync::Arc;

pub const RANOBE_LANG: &str = "ru";
const SITE_ID: &str = "3";

#[derive(Debug, Clone)]
pub struct RanobeLibSource {
    http: HttpClient,
    limiter: Arc<RateLimiter>,
    base: String,
}

impl RanobeLibSource {
    pub fn new(http: HttpClient, limiter: Arc<RateLimiter>, base: String) -> Self {
        Self { http, limiter, base }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }
}

impl Source for RanobeLibSource {
    fn name(&self) -> &'static str {
        RANOBELIB
    }

    fn kind(&self) -> ChapterKind {
        ChapterKind::Ranobe
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<TitleInfo>> {
        let q = vec![
            ("q".to_string(), query.to_string()),
            ("site_id[]".to_string(), SITE_ID.to_string()),
            ("page".to_string(), "1".to_string()),
            ("per_page".to_string(), limit.min(20).to_string()),
        ];
        let resp: RList<RTitle> =
            self.http.get_json(&self.url("/manga"), &q, &self.limiter, RANOBELIB).await?;
        Ok(resp.data.into_iter().map(r_to_title).collect())
    }

    async fn title_card(&self, id: &str) -> Result<TitleInfo> {
        let q = vec![("fields[]".to_string(), "summary".to_string())];
        let resp: ROne<RTitle> =
            self.http.get_json(&self.url(&format!("/manga/{id}")), &q, &self.limiter, RANOBELIB).await?;
        Ok(r_to_title(resp.data))
    }

    async fn list_chapters(&self, id: &str, _lang: &str) -> Result<Vec<ChapterInfo>> {
        let empty: [(&str, &str); 0] = [];
        let resp: RList<RChapter> =
            self.http.get_json(&self.url(&format!("/manga/{id}/chapters")), &empty, &self.limiter, RANOBELIB).await?;
        let mut out: Vec<ChapterInfo> = resp
            .data
            .into_iter()
            .map(|c| ChapterInfo {
                id: format!("v{}n{}", c.volume, c.number),
                volume: Some(c.volume),
                number: Some(c.number),
                title: if c.name.trim().is_empty() { None } else { Some(c.name) },
                lang: RANOBE_LANG.to_string(),
                pages: 0,
            })
            .collect();
        sort_chapters(&mut out);
        Ok(out)
    }

    async fn download_chapter(
        &self,
        title_id: &str,
        chapter: &ChapterInfo,
    ) -> Result<ChapterPayload> {
        let volume = chapter.volume.clone().unwrap_or_default();
        let number = chapter.number.clone().unwrap_or_default();
        let q = vec![
            ("number".to_string(), number.clone()),
            ("volume".to_string(), volume.clone()),
        ];
        let resp: ROne<RChapterData> = self
            .http
            .get_json(&self.url(&format!("/manga/{title_id}/chapter")), &q, &self.limiter, RANOBELIB)
            .await?;
        let paragraphs = content_to_paragraphs(&resp.data.content);
        if paragraphs.is_empty() {
            return Err(not_found(
                RANOBELIB,
                &format!("text of chapter {number} (vol {volume})"),
            ));
        }
        let name = resp.data.name.trim();
        let heading = if name.is_empty() {
            format!("Ch. {number}")
        } else {
            format!("Ch. {number} — {name}")
        };
        Ok(ChapterPayload::Text { heading, paragraphs })
    }
}

fn r_to_title(t: RTitle) -> TitleInfo {
    let title = if t.rus_name.trim().is_empty() { t.name.clone() } else { t.rus_name.clone() };
    let mut alt = vec![t.name, t.eng_name];
    alt.retain(|s| !s.trim().is_empty() && *s != title);
    alt.sort();
    alt.dedup();
    TitleInfo {
        source: RANOBELIB.to_string(),
        source_id: t.slug_url.clone(),
        title,
        alt_titles: alt,
        orig_lang: "ru".to_string(),
        langs: vec!["ru".to_string()],
        description: t.summary.unwrap_or_default(),
        url: format!("https://ranobelib.me/{}", t.slug_url),
        kind: ChapterKind::Ranobe,
        last_volume: None,
        last_chapter: None,
    }
}

// ------------------------------------------------------------- wire DTOs ---

#[derive(Debug, Deserialize)]
struct RList<T> {
    #[serde(default)]
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct ROne<T> {
    data: T,
}

#[derive(Debug, Default, Deserialize)]
struct RTitle {
    #[serde(default)]
    name: String,
    #[serde(default)]
    rus_name: String,
    #[serde(default)]
    eng_name: String,
    #[serde(default)]
    slug_url: String,
    #[serde(default)]
    summary: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RChapter {
    #[serde(default)]
    volume: String,
    #[serde(default)]
    number: String,
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct RChapterData {
    #[serde(default)]
    name: String,
    #[serde(default)]
    content: serde_json::Value,
}

// ------------------------------------------------------- content parsing ---

/// The `content` field is either an HTML string or a tiptap-like JSON doc.
pub fn content_to_paragraphs(content: &serde_json::Value) -> Vec<String> {
    match content {
        serde_json::Value::String(html) => paragraphs_from_html(html),
        serde_json::Value::Object(_) => paragraphs_from_json(content),
        _ => Vec::new(),
    }
}

pub fn paragraphs_from_json(v: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    let arr = v.get("content").and_then(|c| c.as_array());
    let Some(arr) = arr else {
        return out;
    };
    for p in arr {
        // Concatenate ALL inline runs (bold/italic splits lose text otherwise).
        let mut buf = String::new();
        if let Some(a) = p.get("content").and_then(|c| c.as_array()) {
            for n in a {
                if let Some(t) = n.get("text").and_then(|t| t.as_str()) {
                    buf.push_str(t);
                }
            }
        }
        let t = buf.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    out
}

/// Extract `<p>…</p>` blocks without the `regex` crate (crate budget).
pub fn paragraphs_from_html(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    loop {
        let Some(open) = find_p_open(rest) else { break };
        rest = &rest[open..];
        let Some(gt) = rest.find('>') else { break };
        let inner = &rest[gt + 1..];
        let Some(end) = inner.find("</p>") else { break };
        let text = decode_entities(&strip_tags(&inner[..end]));
        let t = text.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
        rest = &inner[end + 4..];
    }
    if out.is_empty() {
        // No <p> tags at all: degrade to stripped lines.
        let flat = decode_entities(&strip_tags(html));
        for line in flat.lines() {
            let l = line.trim();
            if !l.is_empty() {
                out.push(l.to_string());
            }
        }
    }
    out
}

/// Byte offset of `<p`, `<P` (+ space/`>`) in `s`, if any.
fn find_p_open(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'<' && (bytes[i + 1] == b'p' || bytes[i + 1] == b'P') {
            let c = bytes[i + 2];
            if c == b'>' || c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' || c == b'/' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

pub fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ => {
                if !in_tag {
                    out.push(ch);
                }
            }
        }
    }
    out
}

pub fn decode_entities(s: &str) -> String {
    // &amp; LAST to avoid double-decoding "&amp;lt;" into "<".
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn html_paragraphs() {
        let html = "<div><p>Hello <b>world</b>!</p><p>Second &amp; third.</p></div>";
        assert_eq!(paragraphs_from_html(html), vec!["Hello world!".to_string(), "Second & third.".to_string()]);
    }

    #[test]
    fn html_without_p_falls_back() {
        assert_eq!(paragraphs_from_html("Just<br/>text"), vec!["Justtext".to_string()]);
    }

    #[test]
    fn json_paragraphs() {
        let v = json!({"content": [
            {"content": [{"text": "First"}]},
            {"content": [{"text": "  "}]},
            {"content": [{"text": "Second"}]},
        ]});
        assert_eq!(paragraphs_from_json(&v), vec!["First".to_string(), "Second".to_string()]);
    }

    #[test]
    fn strip_and_entities() {
        assert_eq!(strip_tags("<a href=\"x\">Hi</a>"), "Hi");
        assert_eq!(decode_entities("a &lt;b&gt; &amp; c"), "a <b> & c");
    }
}
