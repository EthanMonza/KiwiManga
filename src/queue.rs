//! Job queue on top of SQLite: enqueue with per-user caps + dedup, claim with
//! race protection, progress, cancel, resume-after-kill.

use crate::db::now_unix;
use crate::error::Result;
use crate::pack::PackMode;
use crate::sources::ChapterInfo;
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;

// ---------------------------------------------------------------- limits ---

/// Max active (pending+running) jobs per user.
pub const MAX_ACTIVE_PER_USER: i64 = 3;
/// Max chapters inside a single job.
pub const MAX_CHAPTERS_PER_JOB: usize = 200;
/// Max job-level attempts for transient failures (per-request attempts are
/// separate, see HttpClient).
pub const MAX_JOB_ATTEMPTS: i64 = 3;

// --------------------------------------------------------------- payload ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Last10,
    Range { from: String, to: String },
    Volumes(Vec<String>),
    Single { ch: String },
}

impl JobKind {
    pub fn as_db(&self) -> &'static str {
        match self {
            Self::Last10 => "last10",
            Self::Range { .. } => "range",
            Self::Volumes(_) => "volumes",
            Self::Single { .. } => "single",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobPayload {
    pub source: String,
    pub title_id: String,
    pub title_name: String,
    pub kind: JobKind,
    pub chapters: Vec<ChapterInfo>,
    pub pack_mode: String,
    pub chapter_lang: String,
    pub want_lang: String,
    /// True when chapters are NOT in the requested language (fallback used).
    pub lang_fallback: bool,
}

/// Localized job-kind label, rendered at display time (reader's locale).
pub fn render_kind(i18n: &crate::i18n::I18n, loc: &str, kind: &JobKind) -> String {
    match kind {
        JobKind::Last10 => i18n.get(loc, "kind_last10"),
        JobKind::Range { from, to } => i18n.render(
            loc,
            "kind_range",
            &[("from", from.clone()), ("to", to.clone())],
        ),
        JobKind::Volumes(v) => {
            i18n.render(loc, "kind_volumes", &[("vols", v.join(", "))])
        }
        JobKind::Single { ch } => i18n.render(loc, "kind_single", &[("ch", ch.clone())]),
    }
}

impl JobPayload {
    pub fn pack_mode_parsed(&self) -> PackMode {
        PackMode::from_db(&self.pack_mode)
    }
}

// ------------------------------------------------------------------- row ---

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct JobRow {
    pub id: i64,
    pub chat_id: i64,
    pub kind: String,
    pub payload: String,
    pub status: String,
    pub progress_done: i64,
    pub progress_total: i64,
    pub step: String,
    pub progress_msg_id: Option<i64>,
    pub error: Option<String>,
    pub attempts: i64,
}

impl JobRow {
    pub fn payload_parsed(&self) -> Result<JobPayload> {
        Ok(serde_json::from_str(&self.payload)?)
    }
}

// --------------------------------------------------------------- enqueue ---

pub enum EnqueueOutcome {
    Accepted { id: i64, position: i64 },
    Duplicate { id: i64 },
    RejectActive { active: i64, max: i64 },
    RejectTooMany { n: usize, max: usize },
}

pub async fn enqueue(pool: &SqlitePool, chat_id: i64, payload: &JobPayload) -> Result<EnqueueOutcome> {
    if payload.chapters.len() > MAX_CHAPTERS_PER_JOB {
        return Ok(EnqueueOutcome::RejectTooMany { n: payload.chapters.len(), max: MAX_CHAPTERS_PER_JOB });
    }
    if payload.chapters.is_empty() {
        return Ok(EnqueueOutcome::RejectTooMany { n: 0, max: MAX_CHAPTERS_PER_JOB });
    }
    let active = count_active(pool, chat_id).await?;
    if active >= MAX_ACTIVE_PER_USER {
        return Ok(EnqueueOutcome::RejectActive { active, max: MAX_ACTIVE_PER_USER });
    }
    let body = serde_json::to_string(payload)?;
    let now = now_unix();
    let res = sqlx::query(
        "INSERT INTO jobs (chat_id, kind, payload, status, progress_total, step, created_at, updated_at)
         VALUES (?, ?, ?, 'pending', ?, 'queued', ?, ?)",
    )
    .bind(chat_id)
    .bind(payload.kind.as_db())
    .bind(&body)
    .bind(payload.chapters.len() as i64)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await;
    match res {
        Ok(done) => {
            let id = done.last_insert_rowid();
            let position = global_position(pool, id).await?;
            Ok(EnqueueOutcome::Accepted { id, position })
        }
        Err(e) => {
            if let sqlx::Error::Database(db) = &e {
                if db.is_unique_violation() {
                    if let Some(existing) = find_duplicate(pool, chat_id, payload.kind.as_db(), &body).await? {
                        return Ok(EnqueueOutcome::Duplicate { id: existing });
                    }
                }
            }
            Err(e.into())
        }
    }
}

async fn find_duplicate(
    pool: &SqlitePool,
    chat_id: i64,
    kind: &str,
    payload: &str,
) -> Result<Option<i64>> {
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE chat_id = ? AND kind = ? AND payload = ?
         AND status IN ('pending','running') ORDER BY id LIMIT 1",
    )
    .bind(chat_id)
    .bind(kind)
    .bind(payload)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

pub async fn count_active(pool: &SqlitePool, chat_id: i64) -> Result<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE chat_id = ? AND status IN ('pending','running')",
    )
    .bind(chat_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 1-based global position of a job among pending jobs.
pub async fn global_position(pool: &SqlitePool, job_id: i64) -> Result<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM jobs WHERE status = 'pending' AND id <= ?",
    )
    .bind(job_id)
    .fetch_one(pool)
    .await?;
    Ok(n.max(1))
}

// ----------------------------------------------------------------- claim ---

/// Claim the oldest pending job. Race-safe: the UPDATE only succeeds for one
/// worker (rows_affected check).
pub async fn claim_next(pool: &SqlitePool) -> Result<Option<JobRow>> {
    let cand: Option<JobRow> = sqlx::query_as::<_, JobRow>(
        "SELECT id, chat_id, kind, payload, status, progress_done, progress_total,
                step, progress_msg_id, error, attempts
         FROM jobs WHERE status = 'pending' ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some(job) = cand else {
        return Ok(None);
    };
    let now = now_unix();
    let res = sqlx::query(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1,
                         started_at = ?, updated_at = ?
         WHERE id = ? AND status = 'pending'",
    )
    .bind(now)
    .bind(now)
    .bind(job.id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 1 {
        get(pool, job.id).await.map(Some)
    } else {
        Ok(None) // lost the race, another worker took it
    }
}

/// Startup / graceful-shutdown recovery: jobs stuck in `running` (kill -9,
/// redeploy, crash) go back to `pending`.
pub async fn reset_running_to_pending(pool: &SqlitePool) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE jobs SET status = 'pending', step = 'queued', updated_at = ?
         WHERE status = 'running'",
    )
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

// ------------------------------------------------------------------ misc ---

pub async fn get(pool: &SqlitePool, id: i64) -> Result<JobRow> {
    let row = sqlx::query_as::<_, JobRow>(
        "SELECT id, chat_id, kind, payload, status, progress_done, progress_total,
                step, progress_msg_id, error, attempts
         FROM jobs WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn set_progress(
    pool: &SqlitePool,
    id: i64,
    done: i64,
    total: i64,
    step: &str,
    msg_id: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "UPDATE jobs SET progress_done = ?, progress_total = ?, step = ?,
                         progress_msg_id = COALESCE(?, progress_msg_id), updated_at = ?
         WHERE id = ?",
    )
    .bind(done)
    .bind(total)
    .bind(step)
    .bind(msg_id)
    .bind(now_unix())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_done(pool: &SqlitePool, id: i64) -> Result<()> {
    finish(pool, id, "done", None).await
}

pub async fn mark_failed(pool: &SqlitePool, id: i64, err: &str) -> Result<()> {
    finish(pool, id, "failed", Some(err)).await
}

async fn finish(pool: &SqlitePool, id: i64, status: &str, err: Option<&str>) -> Result<()> {
    sqlx::query(
        "UPDATE jobs SET status = ?, step = ?, error = ?, finished_at = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(status)
    .bind(status)
    .bind(err)
    .bind(now_unix())
    .bind(now_unix())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Put a transiently-failed job back to pending for another attempt.
pub async fn requeue(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query(
        "UPDATE jobs SET status = 'pending', step = 'queued',
                         progress_done = 0, error = NULL, updated_at = ?
         WHERE id = ?",
    )
    .bind(now_unix())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Request cancellation. Returns the previous status if the job was active.
pub async fn request_cancel(pool: &SqlitePool, id: i64) -> Result<Option<String>> {
    let job = get(pool, id).await?;
    if job.status == "pending" || job.status == "running" {
        sqlx::query("UPDATE jobs SET status = 'canceled', step = 'canceled', updated_at = ? WHERE id = ?")
            .bind(now_unix())
            .bind(id)
            .execute(pool)
            .await?;
        Ok(Some(job.status))
    } else {
        Ok(None)
    }
}

pub async fn list_active(pool: &SqlitePool, chat_id: i64) -> Result<Vec<JobRow>> {
    let rows = sqlx::query_as::<_, JobRow>(
        "SELECT id, chat_id, kind, payload, status, progress_done, progress_total,
                step, progress_msg_id, error, attempts
         FROM jobs WHERE chat_id = ? AND status IN ('pending','running') ORDER BY id",
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Newest active job of a user (for bare /cancel).
pub async fn newest_active(pool: &SqlitePool, chat_id: i64) -> Result<Option<JobRow>> {
    let row = sqlx::query_as::<_, JobRow>(
        "SELECT id, chat_id, kind, payload, status, progress_done, progress_total,
                step, progress_msg_id, error, attempts
         FROM jobs WHERE chat_id = ? AND status IN ('pending','running')
         ORDER BY id DESC LIMIT 1",
    )
    .bind(chat_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
