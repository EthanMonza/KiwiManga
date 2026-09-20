//! Environment configuration. See env.example for the full list.

use crate::error::{BotError, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub bot_token: String,
    pub database_url: String,
    pub data_dir: String,
    pub workers: usize,
    pub max_file_mb: u64,
    pub log_level: String,
    pub sources_enabled: Vec<String>,
    pub volume_pack_default: String,
    pub mangadex_api_base: String,
    pub ranobelib_api_base: String,
    pub http_timeout_secs: u64,
    pub http_max_attempts: u32,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let bot_token = env_opt("BOT_TOKEN")
            .or_else(|| env_opt("TELOXIDE_TOKEN"))
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                BotError::Config(
                    "BOT_TOKEN is not set (copy env.example and fill it in)".to_string(),
                )
            })?;
        let database_url = normalize_db_url(&env_or("DATABASE_URL", "sqlite:/data/bot.db?mode=rwc"));
        let data_dir = env_or("DATA_DIR", "/data");
        let workers = env_or("WORKERS", "2").parse::<usize>().map_err(|_| {
            BotError::Config("WORKERS must be a positive integer".to_string())
        })?;
        if workers == 0 {
            return Err(BotError::Config("WORKERS must be >= 1".to_string()));
        }
        let max_file_mb = env_or("MAX_FILE_MB", "50").parse::<u64>().map_err(|_| {
            BotError::Config("MAX_FILE_MB must be a positive integer".to_string())
        })?;
        if max_file_mb == 0 {
            return Err(BotError::Config("MAX_FILE_MB must be >= 1".to_string()));
        }
        let log_level = env_or("LOG_LEVEL", "info");
        let sources_enabled = env_or("SOURCES_ENABLED", "mangadex,ranobelib")
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>();
        if sources_enabled.is_empty() {
            return Err(BotError::Config("SOURCES_ENABLED must not be empty".to_string()));
        }
        let volume_pack_default = env_or("VOLUME_PACK_DEFAULT", "merged").to_lowercase();
        if volume_pack_default != "merged" && volume_pack_default != "per_chapter" {
            return Err(BotError::Config(
                "VOLUME_PACK_DEFAULT must be 'merged' or 'per_chapter'".to_string(),
            ));
        }
        let mangadex_api_base =
            env_or("MANGADEX_API_BASE", "https://api.mangadex.org").trim_end_matches('/').to_string();
        let ranobelib_api_base =
            env_or("RANOBELIB_API_BASE", "https://api.cdnlibs.org/api").trim_end_matches('/').to_string();
        let http_timeout_secs = env_or("HTTP_TIMEOUT_SECS", "30").parse::<u64>().map_err(|_| {
            BotError::Config("HTTP_TIMEOUT_SECS must be a positive integer".to_string())
        })?;
        let http_max_attempts = env_or("HTTP_MAX_ATTEMPTS", "5").parse::<u32>().map_err(|_| {
            BotError::Config("HTTP_MAX_ATTEMPTS must be a positive integer".to_string())
        })?;
        Ok(Self {
            bot_token,
            database_url,
            data_dir,
            workers,
            max_file_mb,
            log_level,
            sources_enabled,
            volume_pack_default,
            mangadex_api_base,
            ranobelib_api_base,
            http_timeout_secs,
            http_max_attempts: http_max_attempts.max(1),
        })
    }

    pub fn max_file_bytes(&self) -> u64 {
        self.max_file_mb * 1024 * 1024
    }

    pub fn source_enabled(&self, name: &str) -> bool {
        self.sources_enabled.iter().any(|s| s == name)
    }
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn env_or(key: &str, default: &str) -> String {
    env_opt(key).unwrap_or_else(|| default.to_string())
}

/// Accepts both `sqlite:/path?mode=rwc` and a bare path (`/data/bot.db`,
/// as shown in the task spec) plus `sqlite::memory:` for tests.
pub fn normalize_db_url(raw: &str) -> String {
    let s = raw.trim();
    if s.starts_with("sqlite:") {
        s.to_string()
    } else if s == ":memory:" {
        "sqlite::memory:".to_string()
    } else {
        format!("sqlite:{s}?mode=rwc")
    }
}
