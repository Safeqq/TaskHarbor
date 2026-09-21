use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

const MAX_TRACKED_KEYS: usize = 1024;

#[derive(Debug, Clone)]
pub struct RateLimiter {
    state: Arc<Mutex<HashMap<String, VecDeque<Instant>>>>,
    limit: usize,
    window: Duration,
}

impl RateLimiter {
    pub fn new(limit: usize, window: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(HashMap::new())),
            limit,
            window,
        }
    }

    pub async fn check(&self, key: impl Into<String>) -> Result<(), Duration> {
        let now = Instant::now();
        let mut state = self.state.lock().await;
        for attempts in state.values_mut() {
            while attempts
                .front()
                .is_some_and(|attempt| now.duration_since(*attempt) >= self.window)
            {
                attempts.pop_front();
            }
        }
        state.retain(|_, attempts| !attempts.is_empty());
        let key = key.into();
        if !state.contains_key(&key) && state.len() >= MAX_TRACKED_KEYS {
            return Err(self.window);
        }
        let attempts = state.entry(key).or_default();
        if attempts.len() >= self.limit {
            let retry_after = attempts
                .front()
                .map(|attempt| self.window.saturating_sub(now.duration_since(*attempt)))
                .unwrap_or(self.window);
            return Err(retry_after);
        }
        attempts.push_back(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::RateLimiter;

    #[tokio::test]
    async fn rejects_requests_after_the_window_limit() {
        let limiter = RateLimiter::new(2, Duration::from_secs(60));
        assert!(limiter.check("owner").await.is_ok());
        assert!(limiter.check("owner").await.is_ok());
        assert!(limiter.check("owner").await.is_err());
        assert!(limiter.check("another-owner").await.is_ok());
    }
}
