//! SQLite access (sqlx, runtime-checked queries only — no DATABASE_URL needed
//! at compile time). WAL mode on file DBs.

use crate::error::{BotError, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use std::time::Duration;

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn connect(url: &str) -> Result<SqlitePool> {
    connect_with(url, 5).await
}

pub async fn connect_with(url: &str, max_connections: u32) -> Result<SqlitePool> {
    let mut opts = SqliteConnectOptions::from_str(url)
        .map_err(|e| BotError::Config(format!("bad DATABASE_URL: {e}")))?;
    opts = opts.create_if_missing(true).busy_timeout(Duration::from_secs(5));
    ensure_parent_dir(url)?;
    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .connect_with(opts)
        .await?;
    // Best-effort pragmas (WAL is a noop on :memory:, which is fine).
    for pragma in [
        "PRAGMA journal_mode=WAL",
        "PRAGMA synchronous=NORMAL",
        "PRAGMA foreign_keys=ON",
    ] {
        if let Err(e) = sqlx::query(pragma).execute(&pool).await {
            tracing::warn!("pragma {pragma} failed: {e}");
        }
    }
    run_migrations(&pool).await?;
    Ok(pool)
}

/// Create the parent directory of a file-backed sqlite URL (noop for :memory:).
fn ensure_parent_dir(url: &str) -> Result<()> {
    let mut path = url.strip_prefix("sqlite:").unwrap_or(url);
    if let Some(q) = path.find('?') {
        path = &path[..q];
    }
    if path == ":memory:" || path.is_empty() {
        return Ok(());
    }
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

pub async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

// ---------------------------------------------------------------- users ---

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserRow {
    pub chat_id: i64,
    pub locale: Option<String>,
    pub auto_locale: String,
    pub pack_mode: String,
    pub chapter_lang: Option<String>,
}

impl UserRow {
    /// Manual /language choice wins over auto-detection.
    pub fn effective_locale(&self) -> String {
        self.locale.clone().unwrap_or_else(|| self.auto_locale.clone())
    }

    /// /settings chapter-language override wins over UI locale.
    pub fn effective_chapter_lang(&self) -> String {
        self.chapter_lang.clone().unwrap_or_else(|| self.effective_locale())
    }
}

pub async fn get_or_create_user(
    pool: &SqlitePool,
    chat_id: i64,
    pack_default: &str,
) -> Result<UserRow> {
    let now = now_unix();
    sqlx::query(
        "INSERT INTO users (chat_id, pack_mode, created_at, updated_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(chat_id) DO NOTHING",
    )
    .bind(chat_id)
    .bind(pack_default)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    get_user(pool, chat_id).await
}

pub async fn get_user(pool: &SqlitePool, chat_id: i64) -> Result<UserRow> {
    sqlx::query_as::<_, UserRow>(
        "SELECT chat_id, locale, auto_locale, pack_mode, chapter_lang FROM users WHERE chat_id = ?",
    )
    .bind(chat_id)
    .fetch_one(pool)
    .await
    .map_err(BotError::Db)
}

pub async fn set_locale(pool: &SqlitePool, chat_id: i64, locale: &str) -> Result<()> {
    sqlx::query("UPDATE users SET locale = ?, updated_at = ? WHERE chat_id = ?")
        .bind(locale)
        .bind(now_unix())
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_auto_locale(pool: &SqlitePool, chat_id: i64, locale: &str) -> Result<()> {
    sqlx::query("UPDATE users SET auto_locale = ?, updated_at = ? WHERE chat_id = ?")
        .bind(locale)
        .bind(now_unix())
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_pack_mode(pool: &SqlitePool, chat_id: i64, mode: &str) -> Result<()> {
    sqlx::query("UPDATE users SET pack_mode = ?, updated_at = ? WHERE chat_id = ?")
        .bind(mode)
        .bind(now_unix())
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_chapter_lang(pool: &SqlitePool, chat_id: i64, lang: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE users SET chapter_lang = ?, updated_at = ? WHERE chat_id = ?")
        .bind(lang)
        .bind(now_unix())
        .bind(chat_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ------------------------------------------------------- chapters cache ---

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CacheRow {
    pub source: String,
    pub title_id: String,
    pub chapter_id: String,
    pub lang: String,
    pub kind: String,
    pub path: String,
    pub bytes: i64,
}

pub async fn cache_get(
    pool: &SqlitePool,
    source: &str,
    title_id: &str,
    chapter_id: &str,
    lang: &str,
) -> Result<Option<CacheRow>> {
    let row = sqlx::query_as::<_, CacheRow>(
        "SELECT source, title_id, chapter_id, lang, kind, path, bytes FROM chapters_cache
         WHERE source = ? AND title_id = ? AND chapter_id = ? AND lang = ?",
    )
    .bind(source)
    .bind(title_id)
    .bind(chapter_id)
    .bind(lang)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

#[allow(clippy::too_many_arguments)] // mirrors the chapters_cache row shape 1:1
pub async fn cache_put(
    pool: &SqlitePool,
    source: &str,
    title_id: &str,
    chapter_id: &str,
    lang: &str,
    kind: &str,
    path: &str,
    bytes: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO chapters_cache
           (source, title_id, chapter_id, lang, kind, path, bytes, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(source, title_id, chapter_id, lang)
         DO UPDATE SET kind = excluded.kind, path = excluded.path,
                       bytes = excluded.bytes, created_at = excluded.created_at",
    )
    .bind(source)
    .bind(title_id)
    .bind(chapter_id)
    .bind(lang)
    .bind(kind)
    .bind(path)
    .bind(bytes)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn cache_delete(
    pool: &SqlitePool,
    source: &str,
    title_id: &str,
    chapter_id: &str,
    lang: &str,
) -> Result<()> {
    sqlx::query(
        "DELETE FROM chapters_cache
         WHERE source = ? AND title_id = ? AND chapter_id = ? AND lang = ?",
    )
    .bind(source)
    .bind(title_id)
    .bind(chapter_id)
    .bind(lang)
    .execute(pool)
    .await?;
    Ok(())
}
