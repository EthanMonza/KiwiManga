//! Background workers: claim jobs, download (with disk cache), pack, split on
//! the 50MB cap, upload sequentially. Progress edits are throttled to 1/2.5s.

use crate::bot::{keyboards as kb, send};
use crate::db;
use crate::error::{BotError, Result};
use crate::pack::{self, PackMode};
use crate::queue::{self, JobKind, JobPayload, JobRow, MAX_JOB_ATTEMPTS};
use crate::sources::{ChapterInfo, ChapterKind, ChapterPayload};
use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile, MessageId, ParseMode};

/// Progress edits must not be more frequent than this (Telegram flood control).
pub const EDIT_THROTTLE: Duration = Duration::from_millis(2500);
/// Refuse new jobs below this much free space.
const MIN_FREE_BYTES: u64 = 200 * 1024 * 1024;
/// Headroom under the Bot API cap when planning parts (zip/epub overhead).
const PLAN_HEADROOM_BYTES: u64 = 1024 * 1024;
const MAX_SEND_ATTEMPTS: u32 = 10;

pub fn spawn_workers(state: AppState, n: usize) -> Vec<tokio::task::JoinHandle<()>> {
    (0..n)
        .map(|i| {
            let s = state.clone();
            tokio::spawn(async move { worker_loop(i, s).await })
        })
        .collect()
}

fn is_shutdown(state: &AppState) -> bool {
    *state.subscribe_shutdown().borrow()
}

async fn worker_loop(id: usize, state: AppState) {
    let bot = Bot::new(state.cfg.bot_token.clone());
    let mut shutdown = state.subscribe_shutdown();
    tracing::info!("worker {id} started");
    loop {
        if *shutdown.borrow() {
            break;
        }
        match queue::claim_next(&state.db).await {
            Ok(Some(job)) => {
                if let Err(e) = process_job(&state, &bot, &job).await {
                    tracing::error!("worker {id}: job #{} failed hard: {e}", job.id);
                }
            }
            Ok(None) => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    _ = shutdown.changed() => { break; }
                }
            }
            Err(e) => {
                tracing::error!("worker {id}: claim failed: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    tracing::info!("worker {id} stopped");
}

// ------------------------------------------------------------ job flow ---

struct Progress<'a> {
    bot: &'a Bot,
    chat: ChatId,
    msg: MessageId,
    cancel_markup: teloxide::types::ReplyMarkup,
    last_edit: Instant,
}

impl<'a> Progress<'a> {
    /// Throttled edit (keeps the Cancel button). Errors are logged, never fatal.
    async fn edit(&mut self, text: &str) {
        if self.last_edit.elapsed() < EDIT_THROTTLE {
            return;
        }
        self.last_edit = Instant::now();
        if let Err(e) = send::edit_html(self.bot, self.chat, self.msg, text, Some(self.cancel_markup.clone())).await {
            tracing::debug!("progress edit failed: {e}");
        }
    }

    /// Final edit (drops buttons). Errors are logged, never fatal.
    async fn finish(&self, text: &str) {
        if let Err(e) = send::edit_html(self.bot, self.chat, self.msg, text, None).await {
            tracing::debug!("final edit failed: {e}");
        }
    }
}

async fn process_job(state: &AppState, bot: &Bot, job: &JobRow) -> Result<()> {
    let payload: JobPayload = match job.payload_parsed() {
        Ok(p) => p,
        Err(e) => {
            queue::mark_failed(&state.db, job.id, &format!("bad payload: {e}")).await?;
            return Ok(());
        }
    };
    if payload.chapters.is_empty() {
        queue::mark_failed(&state.db, job.id, "empty chapter list").await?;
        return Ok(());
    }
    let chat = ChatId(job.chat_id);
    let user =
        db::get_or_create_user(&state.db, job.chat_id, &state.cfg.volume_pack_default).await?;
    let loc = user.effective_locale();
    let i18n = &state.i18n;

    let total = payload.chapters.len() as i64;
    let start_text = i18n.render(
        &loc,
        "progress_start",
        &[
            ("id", job.id.to_string()),
            ("title", send::escape(&payload.title_name)),
            ("total", total.to_string()),
        ],
    );
    let cancel_markup =
        teloxide::types::ReplyMarkup::InlineKeyboard(kb::cancel_job(i18n, &loc, job.id));
    let progress_msg = send::send_html(bot, chat, &start_text, Some(cancel_markup.clone())).await.map_err(BotError::Telegram)?;
    let mut progress = Progress {
        bot,
        chat,
        msg: progress_msg.id,
        cancel_markup,
        last_edit: Instant::now() - EDIT_THROTTLE,
    };
    queue::set_progress(&state.db, job.id, 0, total, "download", Some(progress_msg.id.0 as i64)).await?;

    // Disk-space gate (df missing -> warn + proceed: never block on the check itself).
    match free_bytes(Path::new(&state.cfg.data_dir)) {
        Some(free) if free < MIN_FREE_BYTES => {
            let text = i18n.get(&loc, "err_no_space");
            progress.finish(&text).await;
            queue::mark_failed(&state.db, job.id, "no disk space").await?;
            return Ok(());
        }
        Some(free) => tracing::debug!("free disk: {free} bytes"),
        None => tracing::warn!("cannot determine free disk space; proceeding"),
    }

    if state.source(&payload.source).is_none() {
        let text = i18n.render(&loc, "err_source_down", &[("source", send::escape(&payload.source))]);
        progress.finish(&text).await;
        queue::mark_failed(&state.db, job.id, "source disabled").await?;
        return Ok(());
    }

    let tmp = Path::new(&state.cfg.data_dir).join("tmp").join(format!("job_{}", job.id));
    if let Err(e) = std::fs::create_dir_all(&tmp) {
        return fail(state, job, &progress, &loc, BotError::Io(e)).await;
    }

    // ---- 1. download every chapter into the disk cache (streamed, low RAM) ---
    let mut metas: Vec<CachedChapter> = Vec::with_capacity(payload.chapters.len());
    for (i, ch) in payload.chapters.iter().enumerate() {
        if is_shutdown(state) {
            cleanup_tmp(&tmp);
            queue::requeue(&state.db, job.id).await?;
            tracing::info!("job #{} requeued (shutdown)", job.id);
            return Ok(());
        }
        if is_canceled(state, job.id).await? {
            cleanup_tmp(&tmp);
            progress.finish(&i18n.get(&loc, "err_canceled")).await;
            return Ok(());
        }
        let meta = match ensure_cached(state, &payload, ch).await {
            Ok(m) => m,
            Err(e) => {
                cleanup_tmp(&tmp);
                return fail_or_retry(state, bot, job, &payload, &mut progress, &loc, e).await;
            }
        };
        metas.push(meta);
        queue::set_progress(&state.db, job.id, (i + 1) as i64, total, "download", None).await?;
        let pct = ((i + 1) * 100 / payload.chapters.len().max(1)) as i64;
        let text = i18n.render(
            &loc,
            "progress_download",
            &[
                ("done", (i + 1).to_string()),
                ("total", total.to_string()),
                ("pct", pct.to_string()),
            ],
        );
        progress.edit(&text).await;
    }

    // ---- 2. plan parts, pack, verify sizes, split down if needed -------------
    queue::set_progress(&state.db, job.id, total, total, "pack", None).await?;
    progress.edit(&i18n.get(&loc, "progress_pack")).await;
    let cap = state.cfg.max_file_bytes();
    let plan_cap = cap.saturating_sub(PLAN_HEADROOM_BYTES).max(1024 * 1024);
    let sizes: Vec<u64> = metas.iter().map(|m| m.bytes).collect();
    let (initial, _) = pack::plan_parts(&sizes, plan_cap);
    let pack_mode = payload.pack_mode_parsed();
    let single = matches!(payload.kind, JobKind::Single { .. });

    // Phase 1: pack each part, verify, split down on overflow; spill to tmp.
    let mut work = if initial.is_empty() { vec![(0..metas.len()).collect::<Vec<_>>()] } else { initial };
    let mut seq = 0usize;
    let mut i = 0usize;
    while i < work.len() {
        if is_shutdown(state) {
            cleanup_tmp(&tmp);
            queue::requeue(&state.db, job.id).await?;
            return Ok(());
        }
        if is_canceled(state, job.id).await? {
            cleanup_tmp(&tmp);
            progress.finish(&i18n.get(&loc, "err_canceled")).await;
            return Ok(());
        }
        let indices = work[i].clone();
        let bytes = match pack_part_blocking(state, &payload, pack_mode, single, &metas, &indices).await {
            Ok(b) => b,
            Err(e) => {
                cleanup_tmp(&tmp);
                return fail(state, job, &progress, &loc, e).await;
            }
        };
        if bytes.len() as u64 > cap {
            if indices.len() == 1 {
                cleanup_tmp(&tmp);
                let label = indices
                    .first()
                    .and_then(|k| metas.get(*k))
                    .map(|m| m.info.number.clone().unwrap_or_else(|| "?".to_string()))
                    .unwrap_or_else(|| "?".to_string());
                tracing::warn!("job #{}: chapter {label} exceeds cap alone", job.id);
                let text = i18n.render(&loc, "err_too_big_single", &[("mb", state.cfg.max_file_mb.to_string())]);
                progress.finish(&text).await;
                queue::mark_failed(&state.db, job.id, "single chapter over cap").await?;
                return Ok(());
            }
            let mid = indices.len() / 2;
            let right = indices[mid..].to_vec();
            work[i] = indices[..mid].to_vec();
            work.insert(i + 1, right);
            continue;
        }
        let path = tmp.join(format!("p{seq:04}.bin"));
        seq += 1;
        if let Err(e) = std::fs::write(&path, &bytes) {
            cleanup_tmp(&tmp);
            return fail(state, job, &progress, &loc, BotError::Io(e)).await;
        }
        drop(bytes);
        i += 1;
    }
    let total_parts = seq;
    if total_parts == 0 {
        cleanup_tmp(&tmp);
        return fail(state, job, &progress, &loc, BotError::Pack("nothing to pack".to_string())).await;
    }

    // ---- 3. upload sequentially ----------------------------------------------
    queue::set_progress(&state.db, job.id, total, total, "upload", None).await?;
    progress.edit(&i18n.get(&loc, "progress_upload")).await;
    let kind_label = queue::render_kind(i18n, &loc, &payload.kind);
    for k in 0..total_parts {
        if is_shutdown(state) {
            cleanup_tmp(&tmp);
            queue::requeue(&state.db, job.id).await?;
            return Ok(());
        }
        if is_canceled(state, job.id).await? {
            cleanup_tmp(&tmp);
            progress.finish(&i18n.get(&loc, "err_canceled")).await;
            return Ok(());
        }
        let tmp_path = tmp.join(format!("p{k:04}.bin"));
        let final_path = tmp.join(final_filename(&payload, pack_mode, single, &kind_label, k, total_parts));
        if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
            cleanup_tmp(&tmp);
            return fail(state, job, &progress, &loc, BotError::Io(e)).await;
        }
        let part = if total_parts == 1 {
            String::new()
        } else {
            i18n.render(&loc, "done_part", &[("k", (k + 1).to_string()), ("n", total_parts.to_string())])
        };
        let mut caption = i18n.render(
            &loc,
            "done_caption",
            &[
                ("title", send::escape(&payload.title_name)),
                ("kind", send::escape(&kind_label)),
                ("lang", send::escape(&payload.chapter_lang)),
                ("source", send::escape(&payload.source)),
                ("part", send::escape(&part)),
            ],
        );
        caption = caption.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
        if let Err(e) = send_file(bot, chat, &final_path, &caption).await {
            cleanup_tmp(&tmp);
            return fail_or_retry(state, bot, job, &payload, &mut progress, &loc, e).await;
        }
        let _ = std::fs::remove_file(&final_path);
        let text = i18n.render(
            &loc,
            "progress_download",
            &[
                ("done", (k + 1).to_string()),
                ("total", total_parts.to_string()),
                ("pct", ((k + 1) * 100 / total_parts).to_string()),
            ],
        );
        progress.edit(&text).await;
    }

    cleanup_tmp(&tmp);
    queue::mark_done(&state.db, job.id).await?;
    let done = format!("{} <b>{}</b>", i18n.get(&loc, "step_done"), send::escape(&payload.title_name));
    progress.finish(&done).await;
    Ok(())
}

async fn is_canceled(state: &AppState, job_id: i64) -> Result<bool> {
    Ok(queue::get(&state.db, job_id).await?.status == "canceled")
}

fn cleanup_tmp(tmp: &Path) {
    if let Err(e) = std::fs::remove_dir_all(tmp) {
        tracing::debug!("tmp cleanup failed: {e}");
    }
}

/// Permanent failure: message + mark failed.
async fn fail(
    state: &AppState,
    job: &JobRow,
    progress: &Progress<'_>,
    loc: &str,
    err: BotError,
) -> Result<()> {
    fail_inner(state, job, progress, loc, &err).await
}

/// Transient failure: requeue while attempts remain, else permanent failure.
async fn fail_or_retry(
    state: &AppState,
    _bot: &Bot,
    job: &JobRow,
    _payload: &JobPayload,
    progress: &mut Progress<'_>,
    loc: &str,
    err: BotError,
) -> Result<()> {
    let fresh = queue::get(&state.db, job.id).await?;
    if fresh.status == "canceled" {
        progress.finish(&state.i18n.get(loc, "err_canceled")).await;
        return Ok(());
    }
    if err.is_transient() && fresh.attempts < MAX_JOB_ATTEMPTS {
        tracing::warn!("job #{} transient failure (attempt {}): {err}", job.id, fresh.attempts);
        queue::requeue(&state.db, job.id).await?;
        let text = state.i18n.render(
            loc,
            "job_retry",
            &[("a", (fresh.attempts + 1).to_string()), ("m", MAX_JOB_ATTEMPTS.to_string())],
        );
        progress.edit(&text).await;
        return Ok(());
    }
    fail_inner(state, job, progress, loc, &err).await
}

async fn fail_inner(
    state: &AppState,
    job: &JobRow,
    progress: &Progress<'_>,
    loc: &str,
    err: &BotError,
) -> Result<()> {
    let text = match err {
        BotError::SourceUnreachable(src) => {
            let name = src.split([' ', '(']).next().unwrap_or(src).to_string();
            state.i18n.render(loc, "err_source_down", &[("source", send::escape(&name))])
        }
        BotError::NotFound(_) => state.i18n.get(loc, "err_not_found"),
        other => {
            let short: String = other.to_string().chars().take(200).collect();
            state.i18n.render(loc, "err_generic", &[("err", send::escape(&short))])
        }
    };
    progress.finish(&text).await;
    queue::mark_failed(&state.db, job.id, &err.to_string()).await?;
    Ok(())
}

fn final_filename(
    payload: &JobPayload,
    mode: PackMode,
    single: bool,
    kind_label: &str,
    k: usize,
    total_parts: usize,
) -> String {
    let ext = match (payload_kind(payload), mode, single) {
        (ChapterKind::Manga, _, true) => "cbz",
        (ChapterKind::Manga, PackMode::Merged, false) => "cbz",
        (ChapterKind::Manga, PackMode::PerChapter, false) => "zip",
        (ChapterKind::Ranobe, _, true) => "epub",
        (ChapterKind::Ranobe, PackMode::Merged, false) => "epub",
        (ChapterKind::Ranobe, PackMode::PerChapter, false) => "zip",
    };
    let mut name = format!(
        "{} - {}",
        pack::sanitize_filename(&payload.title_name),
        pack::sanitize_filename(kind_label)
    );
    if total_parts > 1 {
        name.push_str(&format!(" - part {} of {}", k + 1, total_parts));
    }
    format!("{name}.{ext}")
}

fn payload_kind(payload: &JobPayload) -> ChapterKind {
    // The worker knows the source kind via the payload's source name.
    match payload.source.as_str() {
        crate::sources::RANOBELIB => ChapterKind::Ranobe,
        _ => ChapterKind::Manga,
    }
}

// ------------------------------------------------------------------ cache ---

struct CachedChapter {
    info: ChapterInfo,
    dir: PathBuf,
    bytes: u64,
}

fn cache_dir(state: &AppState, payload: &JobPayload, ch: &ChapterInfo) -> PathBuf {
    Path::new(&state.cfg.data_dir)
        .join("cache")
        .join(pack::sanitize_filename(&payload.source))
        .join(pack::sanitize_filename(&payload.title_id))
        .join(pack::sanitize_filename(&ch.id))
}

/// Load chapter bytes from cache, or download + store. Returns dir + size.
async fn ensure_cached(
    state: &AppState,
    payload: &JobPayload,
    ch: &ChapterInfo,
) -> Result<CachedChapter> {
    let dir = cache_dir(state, payload, ch);
    if let Some(row) =
        db::cache_get(&state.db, &payload.source, &payload.title_id, &ch.id, &payload.chapter_lang)
            .await?
    {
        let p = PathBuf::from(&row.path);
        if p.is_dir() && dir_non_empty(&p) {
            let bytes = dir_size(&p);
            return Ok(CachedChapter { info: ch.clone(), dir: p, bytes });
        }
        // Stale row (files wiped): drop it and re-download.
        db::cache_delete(&state.db, &payload.source, &payload.title_id, &ch.id, &payload.chapter_lang)
            .await?;
    }
    let Some(src) = state.source(&payload.source) else {
        return Err(BotError::SourceUnreachable(payload.source.clone()));
    };
    let data = src.download_chapter(&payload.title_id, ch).await?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(BotError::Io(e));
    }
    let kind = match &data {
        ChapterPayload::Manga { pages } => {
            for (i, page) in pages.iter().enumerate() {
                let name = format!("{:03}.{}", i + 1, page.ext);
                if let Err(e) = std::fs::write(dir.join(name), &page.bytes) {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(BotError::Io(e));
                }
            }
            "manga"
        }
        ChapterPayload::Text { heading, paragraphs } => {
            let doc = serde_json::json!({"heading": heading, "paragraphs": paragraphs});
            let text = serde_json::to_string(&doc)?;
            if let Err(e) = std::fs::write(dir.join("text.json"), text) {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(BotError::Io(e));
            }
            "ranobe"
        }
    };
    let bytes = dir_size(&dir);
    db::cache_put(
        &state.db,
        &payload.source,
        &payload.title_id,
        &ch.id,
        &payload.chapter_lang,
        kind,
        &dir.to_string_lossy(),
        bytes as i64,
    )
    .await?;
    Ok(CachedChapter { info: ch.clone(), dir, bytes })
}

fn dir_non_empty(p: &Path) -> bool {
    std::fs::read_dir(p).map(|mut d| d.next().is_some()).unwrap_or(false)
}

fn dir_size(p: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(p) {
        for e in entries.flatten() {
            if let Ok(m) = e.metadata() {
                if m.is_file() {
                    total += m.len();
                }
            }
        }
    }
    total
}

// ------------------------------------------------------------------- pack ---

async fn pack_part_blocking(
    state: &AppState,
    payload: &JobPayload,
    mode: PackMode,
    single: bool,
    metas: &[CachedChapter],
    indices: &[usize],
) -> Result<Vec<u8>> {
    let title = payload.title_name.clone();
    let kind = payload_kind(payload);
    let parts: Vec<(ChapterInfo, PathBuf)> = indices
        .iter()
        .filter_map(|k| metas.get(*k))
        .map(|m| (m.info.clone(), m.dir.clone()))
        .collect();
    let _ = state;
    tokio::task::spawn_blocking(move || pack_part_sync(&title, kind, mode, single, &parts))
        .await
        .map_err(|e| BotError::Pack(format!("pack task failed: {e}")))?
}

fn pack_part_sync(
    title: &str,
    kind: ChapterKind,
    mode: PackMode,
    single: bool,
    parts: &[(ChapterInfo, PathBuf)],
) -> Result<Vec<u8>> {
    match kind {
        ChapterKind::Manga => {
            let mut chapters: Vec<(String, Vec<crate::sources::PageData>)> = Vec::new();
            for (info, dir) in parts {
                chapters.push((chapter_file_stem(info), read_pages(dir)?));
            }
            if single {
                let pages = chapters.into_iter().next().map(|(_, p)| p).unwrap_or_default();
                pack::pack_manga_chapter(&pages)
            } else {
                match mode {
                    PackMode::Merged => {
                        let raws: Vec<Vec<crate::sources::PageData>> =
                            chapters.into_iter().map(|(_, p)| p).collect();
                        pack::pack_manga_merged(&raws)
                    }
                    PackMode::PerChapter => pack::pack_manga_per_chapter(&chapters),
                }
            }
        }
        ChapterKind::Ranobe => {
            let mut chapters: Vec<(String, Vec<String>)> = Vec::new();
            for (info, dir) in parts {
                let (heading, paras) = read_text(dir)?;
                let _ = info;
                chapters.push((heading, paras));
            }
            if single {
                let (h, p) = chapters.into_iter().next().unwrap_or((title.to_string(), Vec::new()));
                pack::pack_ranobe_single(title, &h, &p)
            } else {
                match mode {
                    PackMode::Merged => pack::pack_ranobe_volume(title, &chapters),
                    PackMode::PerChapter => pack::pack_ranobe_per_chapter(title, &chapters),
                }
            }
        }
    }
}

fn chapter_file_stem(info: &ChapterInfo) -> String {
    let num = info.number.clone().unwrap_or_else(|| "oneshot".to_string());
    match &info.title {
        Some(t) if !t.trim().is_empty() => format!("ch {num} - {}", t.trim()),
        _ => format!("ch {num}"),
    }
}

fn read_pages(dir: &Path) -> Result<Vec<crate::sources::PageData>> {
    let mut names: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(dir)?;
    for e in entries {
        let e = e?;
        if e.file_type()?.is_file() {
            if let Some(name) = e.file_name().to_str() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    let mut pages = Vec::with_capacity(names.len());
    for name in names {
        let bytes = std::fs::read(dir.join(&name))?;
        let ext = name.rsplit('.').next().unwrap_or("jpg").to_string();
        pages.push(crate::sources::PageData { ext, bytes });
    }
    if pages.is_empty() {
        return Err(BotError::Pack(format!("cache dir {} is empty", dir.display())));
    }
    Ok(pages)
}

fn read_text(dir: &Path) -> Result<(String, Vec<String>)> {
    let raw = std::fs::read_to_string(dir.join("text.json"))?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    let Some(heading) = v.get("heading").and_then(|h| h.as_str()) else {
        return Err(BotError::Pack(format!("cache dir {} has no heading", dir.display())));
    };
    let heading = heading.to_string();
    let paras: Vec<String> = v
        .get("paragraphs")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    if paras.is_empty() {
        return Err(BotError::Pack(format!("cache dir {} has no text", dir.display())));
    }
    Ok((heading, paras))
}

// ----------------------------------------------------------------- upload ---

async fn send_file(bot: &Bot, chat: ChatId, path: &Path, caption: &str) -> Result<()> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let res = bot
            .send_document(chat, InputFile::file(path))
            .caption(caption.to_string())
            .parse_mode(ParseMode::Html)
            .await;
        match res {
            Ok(_) => return Ok(()),
            Err(teloxide::RequestError::RetryAfter(d)) => {
                tracing::warn!("telegram flood on upload, sleeping {d:?}");
                tokio::time::sleep(d).await;
            }
            Err(e) => {
                if attempt >= MAX_SEND_ATTEMPTS {
                    return Err(BotError::Telegram(e));
                }
                tracing::warn!("upload attempt {attempt} failed: {e}; retrying");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        if attempt >= MAX_SEND_ATTEMPTS {
            return Err(BotError::Telegram(teloxide::RequestError::Io(std::io::Error::other(
                "upload flood retries exhausted",
            ))));
        }
    }
}

// ------------------------------------------------------------------- disk ---

/// Free bytes on the filesystem containing `path` (via `df`, Linux).
/// Returns None when the check itself is unavailable.
fn free_bytes(path: &Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-B1")
        .arg("--output=avail")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().nth(1)?.trim().parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_names() {
        let payload = JobPayload {
            source: "mangadex".to_string(),
            title_id: "x".to_string(),
            title_name: "Berserk".to_string(),
            kind: JobKind::Last10,
            chapters: Vec::new(),
            pack_mode: "merged".to_string(),
            chapter_lang: "en".to_string(),
            want_lang: "en".to_string(),
            lang_fallback: false,
        };
        assert_eq!(
            final_filename(&payload, PackMode::Merged, false, "last 10 chapters", 0, 1),
            "Berserk - last 10 chapters.cbz"
        );
        assert_eq!(
            final_filename(&payload, PackMode::Merged, false, "last 10 chapters", 1, 3),
            "Berserk - last 10 chapters - part 2 of 3.cbz"
        );
        let rnb = JobPayload { source: "ranobelib".to_string(), ..payload.clone() };
        assert_eq!(
            final_filename(&rnb, PackMode::PerChapter, false, "Vol. 1", 0, 1),
            "Berserk - Vol. 1.zip"
        );
    }
}
