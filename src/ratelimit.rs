//! GLOBAL (not per-user) token-bucket rate limiter, one bucket per source.
//! The source is shared by all users, so the bucket must be global too.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    per_second: f64,
    capacity: f64,
    bucket: Mutex<Bucket>,
}

impl RateLimiter {
    pub fn new(per_second: f64, capacity: f64) -> Arc<Self> {
        Arc::new(Self {
            per_second: per_second.max(0.1),
            capacity: capacity.max(1.0),
            bucket: Mutex::new(Bucket { tokens: capacity.max(1.0), last: Instant::now() }),
        })
    }

    /// Wait until one token is available, then take it.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut b = self.bucket.lock().await;
                let now = Instant::now();
                let elapsed = now.duration_since(b.last).as_secs_f64();
                b.last = now;
                b.tokens = (b.tokens + elapsed * self.per_second).min(self.capacity);
                if b.tokens >= 1.0 {
                    b.tokens -= 1.0;
                    None
                } else {
                    let need = (1.0 - b.tokens) / self.per_second;
                    Some(Duration::from_secs_f64(need.max(0.001)))
                }
            };
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }
}

/// Global buckets shared by the whole process.
#[derive(Debug, Clone)]
pub struct SourceLimits {
    pub mangadex: Arc<RateLimiter>,
    pub ranobelib: Arc<RateLimiter>,
    pub fallback: Arc<RateLimiter>,
}

impl SourceLimits {
    pub fn new() -> Self {
        Self {
            // MangaDex asks for courtesy; task spec: ~1 rps global.
            mangadex: RateLimiter::new(1.0, 2.0),
            ranobelib: RateLimiter::new(2.0, 4.0),
            fallback: RateLimiter::new(1.0, 2.0),
        }
    }

    pub fn for_source(&self, source: &str) -> &Arc<RateLimiter> {
        match source {
            crate::sources::MANGADEX => &self.mangadex,
            crate::sources::RANOBELIB => &self.ranobelib,
            _ => &self.fallback,
        }
    }
}

impl Default for SourceLimits {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn burst_then_throttles() {
        let lim = RateLimiter::new(10.0, 2.0);
        // First two are free (full bucket).
        lim.acquire().await;
        lim.acquire().await;
        // Third must wait ~100ms at 10 rps.
        let t = Instant::now();
        lim.acquire().await;
        assert!(t.elapsed() >= Duration::from_millis(50));
    }
}
