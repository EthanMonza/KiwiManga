//! Shared HTTP client: 30s per-request timeout, <=5 attempts, honors 429 +
//! Retry-After, exponential backoff with jitter. No retry for 4xx (except 429).

use crate::error::{BotError, Result};
use crate::ratelimit::RateLimiter;
use serde::de::DeserializeOwned;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const USER_AGENT: &str =
    "KiwiMangaBot/1.0 (+https://github.com/EthanMonza/KiwiManga; contact @EthanMonza)";

#[derive(Debug, Clone)]
pub struct HttpClient {
    client: reqwest::Client,
    max_attempts: u32,
}

impl HttpClient {
    pub fn new(timeout: Duration, max_attempts: u32) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(timeout)
            .build()
            .map_err(|e| BotError::Config(format!("cannot build http client: {e}")))?;
        Ok(Self { client, max_attempts: max_attempts.max(1) })
    }

    pub async fn get_json<T, Q>(
        &self,
        url: &str,
        query: &Q,
        limiter: &RateLimiter,
        source: &str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
        Q: serde::Serialize + ?Sized,
    {
        let bytes = self.get_bytes_inner(url, query, limiter, source).await?;
        serde_json::from_slice(&bytes).map_err(|e| BotError::Source {
            source_name: source.to_string(),
            message: format!("bad json from {url}: {e}"),
        })
    }

    pub async fn get_bytes(
        &self,
        url: &str,
        limiter: &RateLimiter,
        source: &str,
    ) -> Result<Vec<u8>> {
        let empty: [(&str, &str); 0] = [];
        self.get_bytes_inner(url, &empty, limiter, source).await
    }

    async fn get_bytes_inner<Q>(
        &self,
        url: &str,
        query: &Q,
        limiter: &RateLimiter,
        source: &str,
    ) -> Result<Vec<u8>>
    where
        Q: serde::Serialize + ?Sized,
    {
        let mut last_err = String::from("no attempts made");
        for attempt in 0..self.max_attempts {
            limiter.acquire().await;
            let req = self.client.get(url).query(query);
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last_err = format!("request failed: {e}");
                    if attempt + 1 < self.max_attempts {
                        sleep_backoff(attempt).await;
                        continue;
                    }
                    break;
                }
            };
            let status = resp.status();
            if status.is_success() {
                return resp.bytes().await.map(|b| b.to_vec()).map_err(|e| BotError::Source {
                    source_name: source.to_string(),
                    message: format!("cannot read body of {url}: {e}"),
                });
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                let wait = retry_after_secs(&resp).unwrap_or_else(|| backoff_ms(attempt));
                last_err = "429 too many requests".to_string();
                tracing::warn!(source, attempt, wait_ms = wait, "429 from source, backing off");
                tokio::time::sleep(Duration::from_millis(wait)).await;
                continue;
            }
            if status.is_server_error() {
                last_err = format!("server error {status}");
                if attempt + 1 < self.max_attempts {
                    sleep_backoff(attempt).await;
                    continue;
                }
                break;
            }
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(BotError::NotFound(format!("{source}: {url} -> 404")));
            }
            // Other 4xx: do not retry, surface honestly.
            let body = resp.text().await.unwrap_or_default();
            let short: String = body.chars().take(200).collect();
            return Err(BotError::Source {
                source_name: source.to_string(),
                message: format!("{url} -> {status}: {short}"),
            });
        }
        // Exhausted attempts on network errors / 5xx / 429 flood.
        if last_err.contains("429") || last_err.contains("server error") {
            Err(BotError::Source { source_name: source.to_string(), message: last_err })
        } else {
            Err(BotError::SourceUnreachable(format!("{source} ({last_err})")))
        }
    }
}

/// Pure backoff computation (testable): 500ms * 2^attempt + jitter, capped.
pub fn backoff_ms(attempt: u32) -> u64 {
    let base = 500u64.saturating_mul(2u64.saturating_pow(attempt.min(6)));
    base.saturating_add(jitter_ms(250)).min(15_000)
}

async fn sleep_backoff(attempt: u32) {
    tokio::time::sleep(Duration::from_millis(backoff_ms(attempt))).await;
}

/// Jitter without the `rand` crate (crate budget): time-nanos based.
fn jitter_ms(upper: u64) -> u64 {
    if upper == 0 {
        return 0;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % (upper + 1))
        .unwrap_or(0)
}

fn retry_after_secs(resp: &reqwest::Response) -> Option<u64> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|s| s.saturating_mul(1000).min(60_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let a = backoff_ms(0);
        let b = backoff_ms(1);
        assert!(a <= 750, "a={a}");
        assert!(b >= 900 && b <= 1250, "b={b}");
        assert!(backoff_ms(100) <= 15_000);
    }
}
