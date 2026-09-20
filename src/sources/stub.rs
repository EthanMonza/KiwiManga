//! Placeholder adapter: keeps the `AnySource` enum open for future sources
//! and documents the integration checklist (see README "Adding a source").

use crate::error::Result;
use crate::sources::{not_found, ChapterInfo, ChapterKind, ChapterPayload, Source, TitleInfo};

#[derive(Debug, Clone, Copy, Default)]
pub struct StubSource;

impl Source for StubSource {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn kind(&self) -> ChapterKind {
        ChapterKind::Manga
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<TitleInfo>> {
        Ok(Vec::new())
    }

    async fn title_card(&self, id: &str) -> Result<TitleInfo> {
        Err(not_found("stub", id))
    }

    async fn list_chapters(&self, _id: &str, _lang: &str) -> Result<Vec<ChapterInfo>> {
        Ok(Vec::new())
    }

    async fn download_chapter(
        &self,
        _title_id: &str,
        chapter: &ChapterInfo,
    ) -> Result<ChapterPayload> {
        Err(not_found("stub", &chapter.id))
    }
}
