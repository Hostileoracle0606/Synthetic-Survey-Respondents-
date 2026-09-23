//! Shared rate limiter for every Gemini call: requests per minute, input tokens per minute,
//! requests per day, and a cap on calls in flight. Gemini applies limits per Google Cloud
//! project, so one limiter serves all jobs. Adaptive concurrency (halving on 429s) lands in M3.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::error::{AppError, AppResult, ErrorCode};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub requests_per_minute: u32,
    pub tokens_per_minute: u32,
    pub requests_per_day: u32,
    pub max_concurrency: u32,
}

impl Default for Limits {
    /// Conservative until the user's Gemini usage tier is known (BACKLOG B1).
    fn default() -> Self {
        Self {
            requests_per_minute: 60,
            tokens_per_minute: 1_000_000,
            requests_per_day: 1_000,
            max_concurrency: 4,
        }
    }
}

struct Buckets {
    requests: f64,
    tokens: f64,
    last: Instant,
    used_today: u32,
}

pub struct RateLimiter {
    limits: Limits,
    slots: Arc<Semaphore>,
    buckets: Mutex<Buckets>,
}

/// Held while a call is in flight; dropping it frees the concurrency slot.
pub struct Permit {
    _slot: OwnedSemaphorePermit,
    pub estimated_tokens: u32,
}

impl RateLimiter {
    /// `used_today`: requests already made today (from `llm_calls`), so restarts don't reset the daily budget.
    pub fn new(limits: Limits, used_today: u32) -> Self {
        Self {
            limits,
            slots: Arc::new(Semaphore::new(limits.max_concurrency.max(1) as usize)),
            buckets: Mutex::new(Buckets {
                requests: f64::from(limits.requests_per_minute),
                tokens: f64::from(limits.tokens_per_minute),
                last: Instant::now(),
                used_today,
            }),
        }
    }

    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Waits until a request of about `estimated_tokens` input tokens may start.
    /// Fails without waiting when the daily request budget is spent.
    pub async fn acquire(&self, estimated_tokens: u32) -> AppResult<Permit> {
        let slot = self
            .slots
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::new(ErrorCode::Internal, "limiter closed"))?;
        let need_tokens = f64::from(estimated_tokens.min(self.limits.tokens_per_minute));
        loop {
            let wait = {
                let mut b = self.buckets.lock().await;
                if b.used_today >= self.limits.requests_per_day {
                    return Err(AppError::new(
                        ErrorCode::Llm,
                        format!("daily Gemini request budget ({}) is used up; it resets at midnight Pacific time", self.limits.requests_per_day),
                    ));
                }
                let now = Instant::now();
                let elapsed = now.duration_since(b.last).as_secs_f64();
                b.last = now;
                let rpm = f64::from(self.limits.requests_per_minute);
                let tpm = f64::from(self.limits.tokens_per_minute);
                b.requests = (b.requests + elapsed * rpm / 60.0).min(rpm);
                b.tokens = (b.tokens + elapsed * tpm / 60.0).min(tpm);
                if b.requests >= 1.0 && b.tokens >= need_tokens {
                    b.requests -= 1.0;
                    b.tokens -= need_tokens;
                    b.used_today += 1;
                    None
                } else {
                    let for_request = if b.requests >= 1.0 {
                        0.0
                    } else {
                        (1.0 - b.requests) * 60.0 / rpm
                    };
                    let for_tokens = if b.tokens >= need_tokens {
                        0.0
                    } else {
                        (need_tokens - b.tokens) * 60.0 / tpm
                    };
                    Some(Duration::from_secs_f64(
                        for_request.max(for_tokens).max(0.001),
                    ))
                }
            };
            match wait {
                None => {
                    return Ok(Permit {
                        _slot: slot,
                        estimated_tokens,
                    })
                }
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }

    /// Corrects the token bucket once the real input token count is known.
    pub async fn record_actual(&self, permit: &Permit, actual_input_tokens: u32) {
        let mut b = self.buckets.lock().await;
        let diff = f64::from(actual_input_tokens) - f64::from(permit.estimated_tokens);
        b.tokens -= diff;
    }

    pub async fn used_today(&self) -> u32 {
        self.buckets.lock().await.used_today
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(rpm: u32, tpm: u32, rpd: u32, conc: u32) -> Limits {
        Limits {
            requests_per_minute: rpm,
            tokens_per_minute: tpm,
            requests_per_day: rpd,
            max_concurrency: conc,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn requests_per_minute_are_respected() {
        let l = RateLimiter::new(limits(60, 10_000_000, 10_000, 100), 0);
        let start = Instant::now();
        for _ in 0..120 {
            drop(l.acquire(10).await.unwrap());
        }
        // 60 are available at once; the next 60 take about a minute.
        let took = start.elapsed().as_secs_f64();
        assert!((58.0..=62.0).contains(&took), "took {took}s");
    }

    #[tokio::test(start_paused = true)]
    async fn tokens_per_minute_are_respected() {
        let l = RateLimiter::new(limits(10_000, 10_000, 10_000, 100), 0);
        let start = Instant::now();
        for _ in 0..20 {
            drop(l.acquire(1_000).await.unwrap());
        }
        let took = start.elapsed().as_secs_f64();
        assert!((58.0..=62.0).contains(&took), "took {took}s");
    }

    #[tokio::test(start_paused = true)]
    async fn daily_budget_stops_further_calls() {
        let l = RateLimiter::new(limits(1_000, 1_000_000, 5, 10), 3);
        drop(l.acquire(10).await.unwrap());
        drop(l.acquire(10).await.unwrap());
        let err = l.acquire(10).await.err().unwrap();
        assert!(err.message.contains("daily"));
        assert_eq!(l.used_today().await, 5);
    }

    #[tokio::test(start_paused = true)]
    async fn concurrency_is_capped() {
        let l = Arc::new(RateLimiter::new(limits(10_000, 10_000_000, 10_000, 2), 0));
        let a = l.acquire(1).await.unwrap();
        let _b = l.acquire(1).await.unwrap();
        let l2 = l.clone();
        let third = tokio::spawn(async move { l2.acquire(1).await.map(|_| ()) });
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(!third.is_finished());
        drop(a);
        third.await.unwrap().unwrap();
    }
}
