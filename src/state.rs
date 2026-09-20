//! Shared application state: config, DB pool, sources, TTL caches.
//! Handlers stay stateless — everything shared lives here.

use crate::config::Config;
use crate::http::HttpClient;
use crate::i18n::I18n;
use crate::ratelimit::SourceLimits;
use crate::sources::{AnySource, ChapterInfo, MangaDexSource, RanobeLibSource, TitleInfo, MANGADEX, RANOBELIB};
use moka::future::Cache;
use sqlx::sqlite::SqlitePool;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub type SearchCache = Cache<u32, SearchSession>;
pub type TitleCache = Cache<u32, TitleSession>;
pub type PendingCache = Cache<i64, PendingRange>;

#[derive(Debug, Clone)]
pub struct SearchSession {
    pub chat_id: i64,
    pub query: String,
    pub results: Vec<TitleInfo>,
}

#[derive(Debug, Clone)]
pub struct TitleSession {
    pub chat_id: i64,
    pub title: TitleInfo,
    /// Chapters in the actually-used language.
    pub chapters: Vec<ChapterInfo>,
    pub chapter_lang: String,
    pub want_lang: String,
    pub lang_fallback: bool,
    pub volumes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PendingRange {
    pub tid: u32,
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: SqlitePool,
    pub i18n: Arc<I18n>,
    pub http: Arc<HttpClient>,
    pub limits: SourceLimits,
    pub sources: Arc<Vec<AnySource>>,
    pub searches: SearchCache,
    pub titles: TitleCache,
    pub pending: PendingCache,
    ids: Arc<AtomicU32>,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl AppState {
    pub fn new(
        cfg: Config,
        db: SqlitePool,
        i18n: I18n,
        http: HttpClient,
        shutdown_tx: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        let limits = SourceLimits::new();
        let mut sources = Vec::new();
        if cfg.source_enabled(MANGADEX) {
            sources.push(AnySource::MangaDex(MangaDexSource::new(
                http.clone(),
                limits.mangadex.clone(),
                cfg.mangadex_api_base.clone(),
            )));
        }
        if cfg.source_enabled(RANOBELIB) {
            sources.push(AnySource::RanobeLib(RanobeLibSource::new(
                http.clone(),
                limits.ranobelib.clone(),
                cfg.ranobelib_api_base.clone(),
            )));
        }
        // Process-unique id base (time nanos), so button ids do not collide
        // with a previous process generation after a fast restart.
        let base = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(1)
            .max(1);
        Self {
            cfg: Arc::new(cfg),
            db,
            i18n: Arc::new(i18n),
            http: Arc::new(http),
            limits,
            sources: Arc::new(sources),
            searches: Cache::builder()
                .max_capacity(5_000)
                .time_to_live(Duration::from_secs(30 * 60))
                .build(),
            titles: Cache::builder()
                .max_capacity(5_000)
                .time_to_live(Duration::from_secs(30 * 60))
                .build(),
            pending: Cache::builder()
                .max_capacity(10_000)
                .time_to_live(Duration::from_secs(10 * 60))
                .build(),
            ids: Arc::new(AtomicU32::new(base)),
            shutdown_tx,
        }
    }

    pub fn next_id(&self) -> u32 {
        let id = self.ids.fetch_add(1, Ordering::Relaxed).max(1);
        if id == u32::MAX {
            self.ids.store(1, Ordering::Relaxed);
        }
        id
    }

    pub fn source(&self, name: &str) -> Option<&AnySource> {
        self.sources.iter().find(|s| s.name() == name)
    }

    pub fn subscribe_shutdown(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }
}
