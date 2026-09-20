//! Packing: manga -> CBZ (ZIP), ranobe -> EPUB.
//! Plus the >50MB splitter (chapter-range based parts).

use crate::error::{BotError, Result};
use crate::sources::{ChapterKind, PageData};
use std::io::{Cursor, Write};

// ------------------------------------------------------------- PackMode ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackMode {
    Merged,
    PerChapter,
}

impl PackMode {
    pub fn from_db(s: &str) -> Self {
        match s {
            "per_chapter" => Self::PerChapter,
            _ => Self::Merged,
        }
    }

    pub fn to_db(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::PerChapter => "per_chapter",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Merged => Self::PerChapter,
            Self::PerChapter => Self::Merged,
        }
    }
}

// -------------------------------------------------------------- helpers ---

/// Make a string safe for file names and URL path segments.
pub fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.' | '(' | ')') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let t = out.trim().trim_matches('.').trim();
    let cut: String = t.chars().take(80).collect();
    if cut.is_empty() {
        "untitled".to_string()
    } else {
        cut
    }
}

pub fn zero_pad_width(total: usize) -> usize {
    let w = total.max(1).to_string().len();
    w.max(3)
}

fn zip_options() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

// ------------------------------------------------------------ manga CBZ ---

/// One manga chapter -> one CBZ (zip) with zero-padded page names.
pub fn pack_manga_chapter(pages: &[PageData]) -> Result<Vec<u8>> {
    if pages.is_empty() {
        return Err(BotError::Pack("chapter has no pages".to_string()));
    }
    let width = zero_pad_width(pages.len());
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (i, page) in pages.iter().enumerate() {
        let name = format!("{:0width$}.{}", i + 1, page.ext, width = width);
        zip.start_file(name, zip_options())?;
        zip.write_all(&page.bytes)?;
    }
    Ok(zip.finish()?.into_inner())
}

/// Merged volume: all chapters' pages in ONE continuous sequence.
pub fn pack_manga_merged(chapters: &[Vec<PageData>]) -> Result<Vec<u8>> {
    let total: usize = chapters.iter().map(|c| c.len()).sum();
    if total == 0 {
        return Err(BotError::Pack("volume has no pages".to_string()));
    }
    let width = zero_pad_width(total);
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut n = 0usize;
    for pages in chapters {
        for page in pages {
            n += 1;
            let name = format!("{n:0width$}.{}", page.ext, width = width);
            zip.start_file(name, zip_options())?;
            zip.write_all(&page.bytes)?;
        }
    }
    Ok(zip.finish()?.into_inner())
}

/// Per-chapter volume: ONE archive with N separate chapter files (.cbz each).
pub fn pack_manga_per_chapter(chapters: &[(String, Vec<PageData>)]) -> Result<Vec<u8>> {
    if chapters.is_empty() {
        return Err(BotError::Pack("volume has no chapters".to_string()));
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (label, pages) in chapters {
        let inner = pack_manga_chapter(pages)?;
        let name = format!("{}.cbz", sanitize_filename(label));
        zip.start_file(name, zip_options())?;
        zip.write_all(&inner)?;
    }
    Ok(zip.finish()?.into_inner())
}

// ------------------------------------------------------------ ranobe EPUB ---

fn chapter_xhtml(title: &str, heading: &str, paragraphs: &[String]) -> Vec<u8> {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \
         \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n\
         <html xmlns=\"http://www.w3.org/1999/xhtml\">\n<head><title>",
    );
    s.push_str(&xml_escape(title));
    s.push_str("</title></head>\n<body>\n<h1>");
    s.push_str(&xml_escape(heading));
    s.push_str("</h1>\n");
    for p in paragraphs {
        s.push_str("<p>");
        s.push_str(&xml_escape(p));
        s.push_str("</p>\n");
    }
    s.push_str("</body>\n</html>\n");
    s.into_bytes()
}

fn epub_build(title: &str, sections: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let zip = epub_builder::ZipLibrary::new().map_err(|e| BotError::Epub(e.to_string()))?;
    let mut book =
        epub_builder::EpubBuilder::new(zip).map_err(|e| BotError::Epub(e.to_string()))?;
    book.metadata("title", title).map_err(|e| BotError::Epub(e.to_string()))?;
    book.metadata("author", "KiwiManga").map_err(|e| BotError::Epub(e.to_string()))?;
    for (i, (heading, xhtml)) in sections.iter().enumerate() {
        let name = format!("ch{i:03}.xhtml");
        let content =
            epub_builder::EpubContent::new(name, Cursor::new(xhtml.clone())).title(heading.clone());
        book.add_content(content).map_err(|e| BotError::Epub(e.to_string()))?;
    }
    let mut out: Vec<u8> = Vec::new();
    book.generate(&mut out).map_err(|e| BotError::Epub(e.to_string()))?;
    Ok(out)
}

/// One ranobe chapter -> one EPUB.
pub fn pack_ranobe_single(title: &str, heading: &str, paragraphs: &[String]) -> Result<Vec<u8>> {
    if paragraphs.is_empty() {
        return Err(BotError::Pack("chapter has no text".to_string()));
    }
    let xhtml = chapter_xhtml(title, heading, paragraphs);
    epub_build(title, &[(heading.to_string(), xhtml)])
}

/// Merged ranobe volume: ONE epub, every chapter a TOC section.
pub fn pack_ranobe_volume(title: &str, chapters: &[(String, Vec<String>)]) -> Result<Vec<u8>> {
    if chapters.is_empty() {
        return Err(BotError::Pack("volume has no chapters".to_string()));
    }
    let mut sections = Vec::with_capacity(chapters.len());
    for (heading, paragraphs) in chapters {
        sections.push((heading.clone(), chapter_xhtml(title, heading, paragraphs)));
    }
    epub_build(title, &sections)
}

/// Per-chapter ranobe volume: ONE zip with N separate .epub chapter files.
pub fn pack_ranobe_per_chapter(
    title: &str,
    chapters: &[(String, Vec<String>)],
) -> Result<Vec<u8>> {
    if chapters.is_empty() {
        return Err(BotError::Pack("volume has no chapters".to_string()));
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (heading, paragraphs) in chapters {
        let inner = pack_ranobe_single(title, heading, paragraphs)?;
        let name = format!("{}.epub", sanitize_filename(heading));
        zip.start_file(name, zip_options())?;
        zip.write_all(&inner)?;
    }
    Ok(zip.finish()?.into_inner())
}

// -------------------------------------------------------------- splitter ---

/// Plan chapter-range parts so every part stays under `max_bytes`.
///
/// Returns groups of chapter indices. A chapter that alone exceeds the cap is
/// returned as its own group and ALSO reported via `oversize` (caller fails
/// the job honestly — such a chapter cannot be delivered via Bot API).
pub fn plan_parts(sizes: &[u64], max_bytes: u64) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut parts: Vec<Vec<usize>> = Vec::new();
    let mut oversize: Vec<usize> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut cur_bytes: u64 = 0;
    for (i, &sz) in sizes.iter().enumerate() {
        if sz > max_bytes {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
                cur_bytes = 0;
            }
            oversize.push(i);
            parts.push(vec![i]);
            continue;
        }
        if !cur.is_empty() && cur_bytes + sz > max_bytes {
            parts.push(std::mem::take(&mut cur));
            cur_bytes = 0;
        }
        cur.push(i);
        cur_bytes += sz;
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    (parts, oversize)
}

pub fn kind_label(kind: ChapterKind) -> &'static str {
    match kind {
        ChapterKind::Manga => "manga",
        ChapterKind::Ranobe => "ranobe",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(byte: u8, len: usize, ext: &str) -> PageData {
        PageData { ext: ext.to_string(), bytes: vec![byte; len] }
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_filename("Berserk: Vol. 1/2?"), "Berserk_ Vol. 1_2_");
        assert_eq!(sanitize_filename("..."), "untitled");
        assert_eq!(sanitize_filename("  One Piece (ワンピース) "), "One Piece (ワンピース)");
    }

    #[test]
    fn pad_width() {
        assert_eq!(zero_pad_width(0), 3);
        assert_eq!(zero_pad_width(9), 3);
        assert_eq!(zero_pad_width(120), 3);
        assert_eq!(zero_pad_width(1234), 4);
    }

    #[test]
    fn merged_order_is_continuous() {
        let ch1 = vec![page(1, 10, "jpg"), page(2, 10, "png")];
        let ch2 = vec![page(3, 10, "jpg")];
        let bytes = pack_manga_merged(&[ch1, ch2]).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(zip.len(), 3);
        assert_eq!(zip.by_index(0).unwrap().name(), "001.jpg");
        assert_eq!(zip.by_index(1).unwrap().name(), "002.png");
        assert_eq!(zip.by_index(2).unwrap().name(), "003.jpg");
    }

    #[test]
    fn per_chapter_has_n_files() {
        let chs = vec![
            ("ch 1".to_string(), vec![page(1, 5, "jpg")]),
            ("ch 2".to_string(), vec![page(2, 5, "jpg")]),
        ];
        let bytes = pack_manga_per_chapter(&chs).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        assert_eq!(zip.len(), 2);
        assert_eq!(zip.by_index(0).unwrap().name(), "ch 1.cbz");
    }

    #[test]
    fn splitter_cases() {
        // Everything fits -> one part.
        let (parts, over) = plan_parts(&[10, 20, 30], 100);
        assert_eq!(parts, vec![vec![0, 1, 2]]);
        assert!(over.is_empty());
        // Must split.
        let (parts, over) = plan_parts(&[40, 40, 40], 50);
        assert_eq!(parts.len(), 3);
        assert!(over.is_empty());
        // Oversize chapter is flagged.
        let (parts, over) = plan_parts(&[10, 999, 10], 50);
        assert_eq!(over, vec![1]);
        assert_eq!(parts.len(), 3);
        // Empty input.
        let (parts, over) = plan_parts(&[], 50);
        assert!(parts.is_empty() && over.is_empty());
    }

    #[test]
    fn ranobe_epub_builds() {
        let bytes = pack_ranobe_single(
            "Test Novel",
            "Ch. 1 — Beginning",
            &["Para one.".to_string(), "Para two.".to_string()],
        )
        .unwrap();
        assert!(bytes.len() > 1000);
        assert_eq!(&bytes[0..2], b"PK"); // epub is a zip
    }
}
