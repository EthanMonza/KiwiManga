//! Thin handlers: parse input -> touch DB/queue -> reply. No downloading here.

use crate::bot::callbacks::{self as cb, Callback};
use crate::bot::keyboards as kb;
use crate::bot::send;
use crate::db::{self, UserRow};
use crate::i18n::{is_supported, normalize_locale, I18n};
use crate::langdetect;
use crate::normalize;
use crate::queue::{self, EnqueueOutcome, JobKind, JobPayload};
use crate::sources::{distinct_volumes, parse_num, ChapterInfo, ChapterKind, TitleInfo};
use crate::state::{AppState, PendingRange, SearchSession, TitleSession};
use std::cmp::Reverse;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, ChatId, Message, MessageId, ReplyMarkup};

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

// -------------------------------------------------------------- entry ---

pub async fn on_message(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    if msg.from().map(|u| u.is_bot).unwrap_or(true) {
        return Ok(());
    }
    let Some(text) = msg.text().map(|t| t.trim().to_string()) else {
        return Ok(()); // non-text: stickers, photos, etc. are ignored
    };
    if text.is_empty() {
        return Ok(());
    }
    if let Some((cmd, _args)) = command_of(&text) {
        return on_command(bot, msg, state, &cmd).await;
    }
    // Plain text in groups is ignored (personal bot, avoid noise).
    if !matches!(msg.chat.kind, teloxide::types::ChatKind::Private(_)) {
        return Ok(());
    }
    // Pending range input wins over a new search (with an escape hatch).
    let chat_id = msg.chat.id.0;
    if state.pending.get(&chat_id).await.is_some() {
        if parse_range(&text).is_none() && looks_like_title(&text) {
            state.pending.invalidate(&chat_id).await;
        } else {
            return on_range_text(bot, msg, state, &text).await;
        }
    }
    flow_search(bot, msg, state, &text).await
}

fn command_of(text: &str) -> Option<(String, String)> {
    let t = text.strip_prefix('/')?;
    let mut parts = t.splitn(2, char::is_whitespace);
    let mut cmd = parts.next().unwrap_or("").to_string();
    if let Some(at) = cmd.find('@') {
        cmd.truncate(at);
    }
    if cmd.is_empty() {
        return None;
    }
    let args = parts.next().unwrap_or("").trim().to_string();
    Some((cmd.to_lowercase(), args))
}

/// Text that is clearly not a range (user changed their mind) -> new search.
fn looks_like_title(text: &str) -> bool {
    text.chars().count() > 4 && text.chars().any(|c| c.is_alphabetic())
}

// ------------------------------------------------------------ commands ---

async fn on_command(bot: Bot, msg: Message, state: AppState, cmd: &str) -> HandlerResult {
    match cmd {
        "start" => cmd_start(bot, msg, state).await,
        "language" => cmd_language(bot, msg, state).await,
        "help" => cmd_help(bot, msg, state).await,
        "queue" => cmd_queue(bot, msg, state).await,
        "cancel" => cmd_cancel(bot, msg, state).await,
        "status" => cmd_status(bot, msg, state).await,
        "settings" => cmd_settings(bot, msg, state).await,
        _ => {
            let (user, loc) = load_user(&state, msg.chat.id.0).await?;
            let _ = user;
            respond(&bot, msg.chat.id, &state.i18n.get(&loc, "unknown_cmd"), None, None).await?;
            Ok(())
        }
    }
}

async fn load_user(state: &AppState, chat_id: i64) -> Result<(UserRow, String), crate::error::BotError> {
    let user = db::get_or_create_user(&state.db, chat_id, &state.cfg.volume_pack_default).await?;
    let loc = user.effective_locale();
    Ok((user, loc))
}

async fn cmd_start(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    let (user, loc) = load_user(&state, msg.chat.id.0).await?;
    let _ = user;
    let t = |k: &str| state.i18n.get(&loc, k);
    let text = format!("{}\n\n{}\n\n{}\n\n{}", t("start_greeting"), t("start_about"), t("start_hint"), t("start_author"));
    respond(&bot, msg.chat.id, &text, Some(kb::reply_menu()), None).await?;
    Ok(())
}

async fn cmd_language(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    let (user, loc) = load_user(&state, msg.chat.id.0).await?;
    let _ = user;
    respond(
        &bot,
        msg.chat.id,
        &state.i18n.get(&loc, "lang_prompt"),
        Some(kb::as_inline(kb::language_list(&state.i18n))),
        None,
    )
    .await?;
    Ok(())
}

async fn cmd_help(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    let (user, loc) = load_user(&state, msg.chat.id.0).await?;
    let _ = user;
    let i18n = &state.i18n;
    let pack = i18n.render(&loc, "help_pack", &[("mb", state.cfg.max_file_mb.to_string())]);
    let text = format!(
        "{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
        i18n.get(&loc, "help_title"),
        i18n.get(&loc, "help_flow"),
        i18n.get(&loc, "help_zip_how"),
        i18n.get(&loc, "help_zip_why"),
        pack,
        i18n.get(&loc, "help_author"),
    );
    respond(&bot, msg.chat.id, &text, None, None).await?;
    Ok(())
}

fn step_key(status: &str, step: &str) -> String {
    if status == "pending" {
        "step_queued".to_string()
    } else {
        format!("step_{step}")
    }
}

async fn cmd_queue(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    let (user, loc) = load_user(&state, msg.chat.id.0).await?;
    let _ = user;
    let jobs = queue::list_active(&state.db, msg.chat.id.0).await?;
    if jobs.is_empty() {
        respond(&bot, msg.chat.id, &state.i18n.get(&loc, "queue_empty"), None, None).await?;
        return Ok(());
    }
    let mut text = state.i18n.get(&loc, "queue_title");
    for j in jobs {
        let (title, kind) = match j.payload_parsed() {
            Ok(p) => (p.title_name, queue::render_kind(&state.i18n, &loc, &p.kind)),
            Err(_) => (j.kind.clone(), j.kind.clone()),
        };
        let status = state.i18n.get(&loc, &step_key(&j.status, &j.step));
        text.push_str(&format!(
            "\n\n{}",
            state.i18n.render(
                &loc,
                "queue_line",
                &[
                    ("id", j.id.to_string()),
                    ("title", send::escape(&title)),
                    ("kind", send::escape(&kind)),
                    ("status", status),
                    ("done", j.progress_done.to_string()),
                    ("total", j.progress_total.to_string()),
                ],
            )
        ));
    }
    respond(&bot, msg.chat.id, &text, None, None).await?;
    Ok(())
}

async fn cmd_cancel(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    let (user, loc) = load_user(&state, msg.chat.id.0).await?;
    let _ = user;
    let jobs = queue::list_active(&state.db, msg.chat.id.0).await?;
    match jobs.len() {
        0 => {
            respond(&bot, msg.chat.id, &state.i18n.get(&loc, "cancel_none"), None, None).await?;
        }
        1 => {
            queue::request_cancel(&state.db, jobs[0].id).await?;
            let text = state.i18n.render(&loc, "cancel_ok", &[("id", jobs[0].id.to_string())]);
            respond(&bot, msg.chat.id, &text, None, None).await?;
        }
        _ => {
            let mut items = Vec::new();
            for j in &jobs {
                let title = j.payload_parsed().map(|p| p.title_name).unwrap_or_else(|_| j.kind.clone());
                items.push((j.id, title));
            }
            respond(
                &bot,
                msg.chat.id,
                &state.i18n.get(&loc, "cancel_many"),
                Some(kb::as_inline(kb::cancel_pick(&state.i18n, &loc, &items))),
                None,
            )
            .await?;
        }
    }
    Ok(())
}

async fn cmd_status(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    let (user, loc) = load_user(&state, msg.chat.id.0).await?;
    let _ = user;
    let jobs = queue::list_active(&state.db, msg.chat.id.0).await?;
    if jobs.is_empty() {
        respond(&bot, msg.chat.id, &state.i18n.get(&loc, "status_none"), None, None).await?;
        return Ok(());
    }
    let mut text = state.i18n.get(&loc, "status_title");
    for j in jobs {
        let title = j.payload_parsed().map(|p| p.title_name).unwrap_or_else(|_| j.kind.clone());
        let pct = if j.progress_total > 0 { j.progress_done * 100 / j.progress_total } else { 0 };
        let step = state.i18n.get(&loc, &step_key(&j.status, &j.step));
        text.push_str(&format!(
            "\n\n{}",
            state.i18n.render(
                &loc,
                "status_line",
                &[
                    ("id", j.id.to_string()),
                    ("title", send::escape(&title)),
                    ("step", step),
                    ("done", j.progress_done.to_string()),
                    ("total", j.progress_total.to_string()),
                    ("pct", pct.to_string()),
                ],
            )
        ));
    }
    respond(&bot, msg.chat.id, &text, None, None).await?;
    Ok(())
}

fn settings_text(i18n: &I18n, loc: &str, user: &UserRow) -> String {
    let mode_name = i18n.get(
        loc,
        if user.pack_mode == "per_chapter" { "pack_per_chapter_name" } else { "pack_merged_name" },
    );
    let lang_name = match &user.chapter_lang {
        Some(code) => code.clone(),
        None => i18n.get(loc, "settings_chlang_auto_name"),
    };
    format!(
        "{}\n\n{}: <b>{}</b>\n{}: <b>{}</b>",
        i18n.get(loc, "settings_title"),
        i18n.get(loc, "settings_pack"),
        send::escape(&mode_name),
        i18n.get(loc, "settings_chlang"),
        send::escape(&lang_name),
    )
}

async fn cmd_settings(bot: Bot, msg: Message, state: AppState) -> HandlerResult {
    let (user, loc) = load_user(&state, msg.chat.id.0).await?;
    respond(
        &bot,
        msg.chat.id,
        &settings_text(&state.i18n, &loc, &user),
        Some(kb::as_inline(kb::settings(
            &state.i18n,
            &loc,
            user.pack_mode != "per_chapter",
            user.chapter_lang.as_deref(),
        ))),
        None,
    )
    .await?;
    Ok(())
}

// -------------------------------------------------------------- search ---

async fn flow_search(bot: Bot, msg: Message, state: AppState, text: &str) -> HandlerResult {
    let chat = msg.chat.id;
    let mut user = db::get_or_create_user(&state.db, chat.0, &state.cfg.volume_pack_default).await?;
    // Auto-detect only when the user never chose manually.
    if user.locale.is_none() {
        if let Some(guess) = langdetect::detect_locale(text) {
            if guess != user.auto_locale {
                db::set_auto_locale(&state.db, chat.0, guess).await?;
                user.auto_locale = guess.to_string();
            }
        }
    }
    let loc = user.effective_locale();
    let wait = send::send_html(
        &bot,
        chat,
        &state.i18n.render(&loc, "searching", &[("q", send::escape(text))]),
        None,
    )
    .await?;
    let edit = Some(wait.id);

    // Concurrent search across all enabled sources. Errors are stringified
    // inside the tasks so the JoinSet output stays trivially Send.
    let mut set = tokio::task::JoinSet::new();
    for src in state.sources.iter() {
        let s = src.clone();
        let q = text.to_string();
        set.spawn(async move {
            let name = s.name().to_string();
            let res = s.search(&q, 10).await.map_err(|e| e.to_string());
            (name, res)
        });
    }
    let mut hits: Vec<(String, TitleInfo)> = Vec::new();
    let mut first_err: Option<(String, String)> = None;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((name, Ok(list))) => {
                for t in list {
                    hits.push((name.clone(), t));
                }
            }
            Ok((name, Err(e))) => {
                tracing::warn!("search on {name} failed: {e}");
                if first_err.is_none() {
                    first_err = Some((name, e));
                }
            }
            Err(e) => tracing::warn!("search task failed: {e}"),
        }
    }
    // Rank: exact > prefix > substring, using title + alt titles.
    hits.sort_by_key(|(_, t)| Reverse(normalize::best_score(text, &t.all_names())));
    hits.truncate(20);

    if hits.is_empty() {
        if let Some((src, _)) = first_err {
            let text = state.i18n.render(&loc, "err_source_down", &[("source", send::escape(&src))]);
            respond(&bot, chat, &text, None, edit).await?;
        } else {
            let text =
                state.i18n.render(&loc, "search_none", &[("q", send::escape(text))]);
            respond(&bot, chat, &text, None, edit).await?;
        }
        return Ok(());
    }
    let mut results: Vec<TitleInfo> = hits.into_iter().map(|(_, t)| t).collect();
    let single = if results.len() == 1 { results.pop() } else { None };
    if let Some(title) = single {
        return resolve_title(&bot, chat, &state, &title, edit).await;
    }
    let sid = state.next_id();
    state.searches.insert(sid, SearchSession { chat_id: chat.0, query: text.to_string(), results }).await;
    send_search_page(&bot, chat, &state, &loc, sid, 0, edit).await
}

async fn send_search_page(
    bot: &Bot,
    chat: ChatId,
    state: &AppState,
    loc: &str,
    sid: u32,
    page: usize,
    edit: Option<MessageId>,
) -> HandlerResult {
    let Some(session) = state.searches.get(&sid).await else {
        respond(bot, chat, &state.i18n.get(loc, "err_expired"), None, edit).await?;
        return Ok(());
    };
    if session.chat_id != chat.0 {
        return Ok(()); // buttons belong to another chat; ignore quietly
    }
    let total_pages = (session.results.len() + kb::SEARCH_PER_PAGE - 1) / kb::SEARCH_PER_PAGE;
    let page = page.min(total_pages.max(1) - 1);
    let mut text = state.i18n.render(
        loc,
        "search_many",
        &[
            ("n", session.results.len().to_string()),
            ("p", (page + 1).to_string()),
            ("tp", total_pages.max(1).to_string()),
        ],
    );
    let start = page * kb::SEARCH_PER_PAGE;
    for (k, t) in session.results.iter().skip(start).take(kb::SEARCH_PER_PAGE).enumerate() {
        text.push_str(&format!("\n{}. {} ({})", start + k + 1, send::escape(&t.title), send::escape(&t.source)));
    }
    respond(
        bot,
        chat,
        &text,
        Some(kb::as_inline(kb::search_results(&state.i18n, loc, sid, page, &session.results))),
        edit,
    )
    .await?;
    Ok(())
}

// -------------------------------------------------------- title resolve ---

/// UI locale -> MangaDex translation code.
fn md_lang(locale: &str) -> &str {
    match locale {
        "kiwi-en" => "en",
        other => other,
    }
}

fn chapter_display(i18n: &I18n, loc: &str, c: &ChapterInfo) -> String {
    let suffix = match &c.title {
        Some(t) if !t.trim().is_empty() => format!(" — {}", t.trim()),
        _ => String::new(),
    };
    match &c.number {
        Some(n) => i18n.render(loc, "chapter_label", &[("c", n.clone()), ("t", suffix)]),
        None => format!("{}{suffix}", i18n.get(loc, "oneshot")),
    }
}

/// Fetch chapters (with en/ru fallback), store a TitleSession, show the card.
async fn resolve_title(
    bot: &Bot,
    chat: ChatId,
    state: &AppState,
    title: &TitleInfo,
    edit: Option<MessageId>,
) -> HandlerResult {
    let (user, loc) = load_user(state, chat.0).await?;
    let want_ui = user.effective_chapter_lang();
    let want = md_lang(&want_ui).to_string();
    let Some(src) = state.source(&title.source) else {
        respond(bot, chat, &state.i18n.get(&loc, "err_expired"), None, edit).await?;
        return Ok(());
    };
    // Primary language attempt.
    let mut chapters = src.list_chapters(&title.source_id, &want).await.unwrap_or_else(|e| {
        tracing::warn!("list_chapters {} failed: {e}", title.source);
        Vec::new()
    });
    let mut chapter_lang = want.clone();
    let mut fallback = false;
    if chapters.is_empty() {
        // Honest fallback: en for manga, ru for ranobe.
        let fb = match src.kind() {
            ChapterKind::Manga => "en",
            ChapterKind::Ranobe => "ru",
        };
        if fb != want {
            chapters = src.list_chapters(&title.source_id, fb).await.unwrap_or_default();
            if !chapters.is_empty() {
                chapter_lang = fb.to_string();
                fallback = true;
            }
        }
    }
    if chapters.is_empty() {
        respond(bot, chat, &state.i18n.get(&loc, "err_not_found"), None, edit).await?;
        return Ok(());
    }
    let volumes = distinct_volumes(&chapters);
    let tid = state.next_id();
    state
        .titles
        .insert(
            tid,
            TitleSession {
                chat_id: chat.0,
                title: title.clone(),
                chapters: chapters.clone(),
                chapter_lang: chapter_lang.clone(),
                want_lang: want.clone(),
                lang_fallback: fallback,
                volumes,
            },
        )
        .await;
    // Card.
    let kind = state.i18n.get(
        &loc,
        match title.kind {
            ChapterKind::Manga => "kind_manga",
            ChapterKind::Ranobe => "kind_ranobe",
        },
    );
    let langs = if title.langs.is_empty() {
        chapter_lang.clone()
    } else {
        title.langs.iter().take(8).cloned().collect::<Vec<_>>().join(", ")
    };
    let desc = truncate(&title.description, 300);
    let mut text = state.i18n.render(
        &loc,
        "search_card",
        &[
            ("title", send::escape(&title.title)),
            ("source", send::escape(&title.source)),
            ("kind", kind),
            ("orig", send::escape(&title.orig_lang)),
            ("langs", send::escape(&langs)),
            ("desc", send::escape(&desc)),
        ],
    );
    text.push_str(&format!(
        "\n{}",
        state.i18n.render(
            &loc,
            "card_chapters",
            &[("n", chapters.len().to_string()), ("lang", chapter_lang.clone())]
        )
    ));
    if fallback {
        text.push_str(&format!(
            "\n{}",
            state.i18n.render(
                &loc,
                "lang_fallback",
                &[("want", send::escape(&want)), ("got", send::escape(&chapter_lang))]
            )
        ));
    }
    text.push_str(&format!("\n\n{}", state.i18n.get(&loc, "ask_what")));
    respond(bot, chat, &text, Some(kb::as_inline(kb::title_options(&state.i18n, &loc, tid))), edit)
        .await?;
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

// --------------------------------------------------------------- enqueue ---

#[allow(clippy::too_many_arguments)]
async fn enqueue_job(
    bot: &Bot,
    chat: ChatId,
    state: &AppState,
    loc: &str,
    session: &TitleSession,
    kind: JobKind,
    chapters: Vec<ChapterInfo>,
    edit: Option<MessageId>,
) -> HandlerResult {
    let user = db::get_user(&state.db, chat.0).await?;
    let payload = JobPayload {
        source: session.title.source.clone(),
        title_id: session.title.source_id.clone(),
        title_name: session.title.title.clone(),
        kind,
        chapters,
        pack_mode: user.pack_mode.clone(),
        chapter_lang: session.chapter_lang.clone(),
        want_lang: session.want_lang.clone(),
        lang_fallback: session.lang_fallback,
    };
    let text = match queue::enqueue(&state.db, chat.0, &payload).await? {
        EnqueueOutcome::Accepted { id, position } => state.i18n.render(
            loc,
            "job_accepted",
            &[("id", id.to_string()), ("n", position.to_string())],
        ),
        EnqueueOutcome::Duplicate { id } => {
            state.i18n.render(loc, "job_dup", &[("id", id.to_string())])
        }
        EnqueueOutcome::RejectActive { active, max } => state.i18n.render(
            loc,
            "job_limit_active",
            &[("n", active.to_string()), ("max", max.to_string())],
        ),
        EnqueueOutcome::RejectTooMany { n, max } => state.i18n.render(
            loc,
            "job_limit_chapters",
            &[("n", n.to_string()), ("max", max.to_string())],
        ),
    };
    // Receipts replace the button message (chat stays clean), no buttons.
    respond(bot, chat, &text, None, edit).await?;
    Ok(())
}

// ----------------------------------------------------------------- range ---

fn parse_range(text: &str) -> Option<(f64, f64)> {
    let t = text.trim().replace(['–', '—', ':', '/'], "-");
    if t.starts_with('-') || t.ends_with('-') {
        return None;
    }
    let parts: Vec<&str> = t.split('-').map(str::trim).filter(|s| !s.is_empty()).collect();
    match parts.len() {
        1 => {
            let a: f64 = parts[0].parse().ok()?;
            if a.is_finite() && a >= 0.0 {
                Some((a, a))
            } else {
                None
            }
        }
        2 => {
            let a: f64 = parts[0].parse().ok()?;
            let b: f64 = parts[1].parse().ok()?;
            if a.is_finite() && b.is_finite() && a >= 0.0 && b >= 0.0 {
                Some((a.min(b), a.max(b)))
            } else {
                None
            }
        }
        _ => None,
    }
}

async fn on_range_text(bot: Bot, msg: Message, state: AppState, text: &str) -> HandlerResult {
    let chat = msg.chat.id;
    let (user, loc) = load_user(&state, chat.0).await?;
    let _ = user;
    let Some((a, b)) = parse_range(text) else {
        respond(&bot, chat, &state.i18n.get(&loc, "range_invalid"), None, None).await?;
        return Ok(());
    };
    let Some(pending) = state.pending.get(&chat.0).await else {
        respond(&bot, chat, &state.i18n.get(&loc, "err_expired"), None, None).await?;
        return Ok(());
    };
    state.pending.invalidate(&chat.0).await;
    let Some(session) = state.titles.get(&pending.tid).await else {
        respond(&bot, chat, &state.i18n.get(&loc, "err_expired"), None, None).await?;
        return Ok(());
    };
    if session.chat_id != chat.0 {
        return Ok(());
    }
    let picked: Vec<ChapterInfo> = session
        .chapters
        .iter()
        .filter(|c| {
            let n = parse_num(c.number.as_deref());
            n >= a && n <= b
        })
        .cloned()
        .collect();
    if picked.is_empty() {
        respond(&bot, chat, &state.i18n.get(&loc, "range_none"), None, None).await?;
        return Ok(());
    }
    let from = fmt_num(a);
    let to = fmt_num(b);
    enqueue_job(&bot, chat, &state, &loc, &session, JobKind::Range { from, to }, picked, None).await
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

// ------------------------------------------------------------- callbacks ---

pub async fn on_callback(bot: Bot, q: CallbackQuery, state: AppState) -> HandlerResult {
    send::answer_callback(&bot, &q).await;
    let Some(data) = q.data.clone() else {
        return Ok(());
    };
    let Some(cmd) = cb::parse(&data) else {
        return Ok(());
    };
    let Some(msg) = q.message.clone() else {
        return Ok(()); // inline-mode queries carry no message; not used
    };
    let chat = msg.chat.id;
    let edit = Some(msg.id);
    let (_user, loc) = load_user(&state, chat.0).await?;

    match cmd {
        Callback::Close => {
            if let Some(id) = edit {
                let _ = bot.delete_message(chat, id).await;
            }
        }
        Callback::SearchPage { sid, page } => {
            send_search_page(&bot, chat, &state, &loc, sid, page, edit).await?;
        }
        Callback::Pick { sid, idx } => {
            let Some(session) = state.searches.get(&sid).await else {
                respond(&bot, chat, &state.i18n.get(&loc, "err_expired"), None, edit).await?;
                return Ok(());
            };
            if session.chat_id != chat.0 {
                return Ok(());
            }
            let Some(title) = session.results.get(idx).cloned() else {
                respond(&bot, chat, &state.i18n.get(&loc, "err_expired"), None, edit).await?;
                return Ok(());
            };
            resolve_title(&bot, chat, &state, &title, edit).await?;
        }
        Callback::Last10 { tid } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            let n = s.chapters.len();
            let picked: Vec<ChapterInfo> = s.chapters.iter().skip(n.saturating_sub(10)).cloned().collect();
            enqueue_job(&bot, chat, &state, &loc, &s, JobKind::Last10, picked, edit).await?;
        }
        Callback::Range { tid } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            let _ = s;
            state.pending.insert(chat.0, PendingRange { tid }).await;
            let text = state.i18n.render(
                &loc,
                "range_prompt",
                &[("max", queue::MAX_CHAPTERS_PER_JOB.to_string())],
            );
            respond(&bot, chat, &text, None, edit).await?;
        }
        Callback::Volumes { tid } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            if s.volumes.is_empty() {
                // No volume info -> fall back to the chapter list.
                send_chapters_page(&bot, chat, &state, &loc, &s, tid, 0, edit).await?;
            } else {
                send_volumes_page(&bot, chat, &state, &loc, &s, tid, 0, edit).await?;
            }
        }
        Callback::Single { tid } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            send_chapters_page(&bot, chat, &state, &loc, &s, tid, 0, edit).await?;
        }
        Callback::BackOptions { tid } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            let _ = s;
            respond(
                &bot,
                chat,
                &state.i18n.get(&loc, "ask_what"),
                Some(kb::as_inline(kb::title_options(&state.i18n, &loc, tid))),
                edit,
            )
            .await?;
        }
        Callback::VolPage { tid, page } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            send_volumes_page(&bot, chat, &state, &loc, &s, tid, page, edit).await?;
        }
        Callback::VolPick { tid, idx } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            let Some(vol) = s.volumes.get(idx).cloned() else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            let picked: Vec<ChapterInfo> = s
                .chapters
                .iter()
                .filter(|c| c.volume.as_deref() == Some(vol.as_str()))
                .cloned()
                .collect();
            enqueue_job(&bot, chat, &state, &loc, &s, JobKind::Volumes(vec![vol]), picked, edit).await?;
        }
        Callback::ChPage { tid, page } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            send_chapters_page(&bot, chat, &state, &loc, &s, tid, page, edit).await?;
        }
        Callback::ChPick { tid, idx } => {
            let Some(s) = title_session(&state, tid, chat.0).await else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            let Some(ch) = s.chapters.get(idx).cloned() else {
                return expired(&bot, chat, &state, &loc, edit).await;
            };
            let label = send::escape(&chapter_display(&state.i18n, &loc, &ch));
            enqueue_job(&bot, chat, &state, &loc, &s, JobKind::Single { ch: label }, vec![ch], edit).await?;
        }
        Callback::CancelJob { job_id } => {
            let job = queue::get(&state.db, job_id).await;
            let Ok(job) = job else {
                respond(&bot, chat, &state.i18n.get(&loc, "cancel_none"), None, edit).await?;
                return Ok(());
            };
            if job.chat_id != chat.0 {
                return Ok(());
            }
            queue::request_cancel(&state.db, job_id).await?;
            let text = state.i18n.render(&loc, "cancel_ok", &[("id", job_id.to_string())]);
            respond(&bot, chat, &text, None, edit).await?;
        }
        Callback::PackMode { merged } => {
            let mode = if merged { "merged" } else { "per_chapter" };
            db::set_pack_mode(&state.db, chat.0, mode).await?;
            let user = db::get_user(&state.db, chat.0).await?;
            respond(
                &bot,
                chat,
                &settings_text(&state.i18n, &loc, &user),
                Some(kb::as_inline(kb::settings(
                    &state.i18n,
                    &loc,
                    merged,
                    user.chapter_lang.as_deref(),
                ))),
                edit,
            )
            .await?;
        }
        Callback::ChapterLang { code } => {
            if code == "auto" {
                db::set_chapter_lang(&state.db, chat.0, None).await?;
            } else if kb::CHAPTER_LANGS.contains(&code.as_str()) {
                db::set_chapter_lang(&state.db, chat.0, Some(&code)).await?;
            } else {
                return Ok(()); // forged/old button
            }
            let user = db::get_user(&state.db, chat.0).await?;
            respond(
                &bot,
                chat,
                &settings_text(&state.i18n, &loc, &user),
                Some(kb::as_inline(kb::settings(
                    &state.i18n,
                    &loc,
                    user.pack_mode != "per_chapter",
                    user.chapter_lang.as_deref(),
                ))),
                edit,
            )
            .await?;
        }
        Callback::Lang { code } => {
            if !is_supported(&code) {
                return Ok(());
            }
            let code = normalize_locale(&code).to_string();
            db::set_locale(&state.db, chat.0, &code).await?;
            let name = crate::i18n::display_name(&state.i18n, &code);
            // Reply in the NEW locale.
            let text = state.i18n.render(&code, "lang_set", &[("name", send::escape(&name))]);
            respond(&bot, chat, &text, None, edit).await?;
        }
    }
    Ok(())
}

async fn title_session(state: &AppState, tid: u32, chat_id: i64) -> Option<TitleSession> {
    let s = state.titles.get(&tid).await?;
    if s.chat_id != chat_id {
        return None;
    }
    Some(s)
}

async fn expired(
    bot: &Bot,
    chat: ChatId,
    state: &AppState,
    loc: &str,
    edit: Option<MessageId>,
) -> HandlerResult {
    respond(bot, chat, &state.i18n.get(loc, "err_expired"), None, edit).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_volumes_page(
    bot: &Bot,
    chat: ChatId,
    state: &AppState,
    loc: &str,
    s: &TitleSession,
    tid: u32,
    page: usize,
    edit: Option<MessageId>,
) -> HandlerResult {
    let total_pages = (s.volumes.len() + kb::VOLS_PER_PAGE - 1) / kb::VOLS_PER_PAGE;
    let page = page.min(total_pages.max(1) - 1);
    let text = state.i18n.render(
        loc,
        "volumes_prompt",
        &[("p", (page + 1).to_string()), ("tp", total_pages.max(1).to_string())],
    );
    respond(
        bot,
        chat,
        &text,
        Some(kb::as_inline(kb::volumes_page(&state.i18n, loc, tid, page, &s.volumes))),
        edit,
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn send_chapters_page(
    bot: &Bot,
    chat: ChatId,
    state: &AppState,
    loc: &str,
    s: &TitleSession,
    tid: u32,
    page: usize,
    edit: Option<MessageId>,
) -> HandlerResult {
    let labels: Vec<String> =
        s.chapters.iter().map(|c| chapter_display(&state.i18n, loc, c)).collect();
    let total_pages = (labels.len() + kb::CHS_PER_PAGE - 1) / kb::CHS_PER_PAGE;
    let page = page.min(total_pages.max(1) - 1);
    let text = state.i18n.render(
        loc,
        "chapters_prompt",
        &[("p", (page + 1).to_string()), ("tp", total_pages.max(1).to_string())],
    );
    respond(
        bot,
        chat,
        &text,
        Some(kb::as_inline(kb::chapters_page(&state.i18n, loc, tid, page, &labels))),
        edit,
    )
    .await?;
    Ok(())
}

// ------------------------------------------------------------------ util ---

async fn respond(
    bot: &Bot,
    chat: ChatId,
    text: &str,
    markup: Option<ReplyMarkup>,
    edit: Option<MessageId>,
) -> Result<(), teloxide::RequestError> {
    match edit {
        Some(id) => send::edit_html(bot, chat, id, text, markup).await,
        None => send::send_html(bot, chat, text, markup).await.map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_parse() {
        assert_eq!(
            command_of("/start"),
            Some(("start".to_string(), String::new()))
        );
        assert_eq!(
            command_of("/Settings@KiwiMangaBot payload here"),
            Some(("settings".to_string(), "payload here".to_string()))
        );
        assert_eq!(command_of("plain"), None);
        assert_eq!(command_of("/"), None);
    }

    #[test]
    fn range_parse() {
        assert_eq!(parse_range("10-25"), Some((10.0, 25.0)));
        assert_eq!(parse_range("25-10"), Some((10.0, 25.0)));
        assert_eq!(parse_range("10 – 25"), Some((10.0, 25.0)));
        assert_eq!(parse_range("12"), Some((12.0, 12.0)));
        assert_eq!(parse_range("1.5-2.5"), Some((1.5, 2.5)));
        assert_eq!(parse_range("abc"), None);
        assert_eq!(parse_range("10-20-30"), None);
        assert_eq!(parse_range("-5"), None);
    }
}
