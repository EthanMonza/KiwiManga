//! Telegram send/edit helpers: HTML mode, 4096-char chunking, RetryAfter
//! handling (sleep + one retry), "message is not modified" tolerance.

use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, Message, MessageId, ReplyMarkup};

/// Escape user/source-controlled text for HTML parse mode.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Split long text into <= `limit`-char chunks, preferably at newlines.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for line in text.split_inclusive('\n') {
        let llen = line.chars().count();
        if cur_len + llen > limit && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            cur_len = 0;
        }
        // A single gigantic line: hard-cut it.
        if llen > limit {
            let mut rest = line;
            while !rest.is_empty() {
                let cut: String = rest.chars().take(limit).collect();
                let used = cut.len();
                if cur_len + cut.chars().count() > limit && !cur.is_empty() {
                    chunks.push(std::mem::take(&mut cur));
                    cur_len = 0;
                }
                cur_len += cut.chars().count();
                cur.push_str(&cut);
                rest = &rest[used..];
            }
        } else {
            cur.push_str(line);
            cur_len += llen;
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

fn is_not_modified(e: &teloxide::RequestError) -> bool {
    format!("{e:?}").contains("message is not modified")
}

async fn sleep_retry_after(e: &teloxide::RequestError) -> bool {
    if let teloxide::RequestError::RetryAfter(d) = e {
        tracing::warn!("telegram flood control: sleeping {d:?}");
        tokio::time::sleep(*d).await;
        true
    } else {
        false
    }
}

pub async fn send_html(
    bot: &Bot,
    chat: teloxide::types::ChatId,
    text: &str,
    markup: Option<ReplyMarkup>,
) -> Result<Message, teloxide::RequestError> {
    let chunks = chunk_text(text, 4000);
    let mut last: Option<Message> = None;
    for (i, part) in chunks.iter().enumerate() {
        let is_last = i + 1 == chunks.len();
        let m = markup.clone().filter(|_| is_last);
        let mut req = bot.send_message(chat, part).parse_mode(teloxide::types::ParseMode::Html);
        if let Some(m) = m {
            req = req.reply_markup(m);
        }
        match req.await {
            Ok(msg) => last = Some(msg),
            Err(e) => {
                if sleep_retry_after(&e).await {
                    let mut req2 =
                        bot.send_message(chat, part).parse_mode(teloxide::types::ParseMode::Html);
                    if is_last {
                        if let Some(m) = markup.clone() {
                            req2 = req2.reply_markup(m);
                        }
                    }
                    last = Some(req2.await?);
                } else {
                    return Err(e);
                }
            }
        }
    }
    // chunk_text never returns empty vec, so `last` is always Some; the
    // fallback below is unreachable but keeps this total without unwrap.
    last.map(Ok).unwrap_or_else(|| {
        Err(teloxide::RequestError::Io(std::io::Error::other("send_html produced no chunks")))
    })
}

/// `editMessageText` accepts inline keyboards only (Telegram API limit);
/// reply-style markup is dropped here, mirroring that restriction.
fn inline_kb(markup: &Option<ReplyMarkup>) -> Option<InlineKeyboardMarkup> {
    match markup {
        Some(ReplyMarkup::InlineKeyboard(kb)) => Some(kb.clone()),
        _ => None,
    }
}

pub async fn edit_html(
    bot: &Bot,
    chat: teloxide::types::ChatId,
    msg_id: MessageId,
    text: &str,
    markup: Option<ReplyMarkup>,
) -> Result<(), teloxide::RequestError> {
    // Edits keep the first chunk only (progress/cards are short by design).
    let part = chunk_text(text, 4000).into_iter().next().unwrap_or_default();
    let mut req =
        bot.edit_message_text(chat, msg_id, part.clone()).parse_mode(teloxide::types::ParseMode::Html);
    if let Some(kb) = inline_kb(&markup) {
        req = req.reply_markup(kb);
    }
    match req.await {
        Ok(_) => Ok(()),
        Err(e) if is_not_modified(&e) => Ok(()),
        Err(e) => {
            if sleep_retry_after(&e).await {
                let mut req2 = bot
                    .edit_message_text(chat, msg_id, part)
                    .parse_mode(teloxide::types::ParseMode::Html);
                if let Some(kb) = inline_kb(&markup) {
                    req2 = req2.reply_markup(kb);
                }
                match req2.await {
                    Ok(_) | Err(_) => Ok(()), // second failure: drop the edit, job continues
                }
            } else if is_not_modified(&e) {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}

pub async fn answer_callback(bot: &Bot, q: &teloxide::types::CallbackQuery) {
    if let Err(e) = bot.answer_callback_query(q.id.clone()).await {
        // "query is too old" etc. — never fatal.
        tracing::debug!("answer_callback_query failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_html() {
        assert_eq!(escape("<a>&\"b\"</a>"), "&lt;a&gt;&amp;\"b\"&lt;/a&gt;");
    }

    #[test]
    fn chunking_short() {
        assert_eq!(chunk_text("hi", 4000), vec!["hi".to_string()]);
    }

    #[test]
    fn chunking_splits_at_newlines() {
        let text = "a\n".repeat(100);
        let chunks = chunk_text(&text, 100);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= 100);
        }
        assert_eq!(chunks.join(""), text);
    }

    #[test]
    fn chunking_cuts_giant_line() {
        let text = "x".repeat(500);
        let chunks = chunk_text(&text, 100);
        assert_eq!(chunks.len(), 5);
        assert_eq!(chunks.join(""), text);
    }
}
