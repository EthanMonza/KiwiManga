//! Single error type for the whole bot (thiserror-based).

use thiserror::Error;

pub type Result<T> = std::result::Result<T, BotError>;

#[derive(Debug, Error)]
pub enum BotError {
    #[error("config error: {0}")]
    Config(String),

    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("telegram error: {0}")]
    Telegram(#[from] teloxide::RequestError),

    #[error("source error ({source_name}): {message}")]
    Source { source_name: String, message: String },

    /// Source TCP/TLS/DNS failure or persistent 5xx/timeouts: most likely the
    /// hosting IP cannot reach the source. Surfaced to the user as an honest
    /// "source unreachable from this hosting" message.
    #[error("source unreachable from this hosting: {0}")]
    SourceUnreachable(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("pack error: {0}")]
    Pack(String),

    #[error("epub error: {0}")]
    Epub(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("i18n error: {0}")]
    I18n(String),
}

impl BotError {
    /// True for failures worth a job-level retry (network flakiness, 5xx, TG floods).
    pub fn is_transient(&self) -> bool {
        match self {
            BotError::SourceUnreachable(_) => true,
            BotError::Source { .. } => true,
            BotError::Http(e) => e.is_timeout() || e.is_connect() || e.is_request(),
            BotError::Telegram(e) => {
                // Permanent delivery failures (blocked bot / deleted chat) must
                // not be retried. Matched via Debug text on purpose: the
                // ApiError variant set differs between teloxide versions, and a
                // Debug-based check cannot break compilation on upgrade.
                let s = format!("{e:?}");
                !(s.contains("BotBlocked")
                    || s.contains("bot was blocked")
                    || s.contains("ChatNotFound")
                    || s.contains("chat not found")
                    || s.contains("UserDeactivated")
                    || s.contains("user is deactivated"))
            }
            _ => false,
        }
    }
}
