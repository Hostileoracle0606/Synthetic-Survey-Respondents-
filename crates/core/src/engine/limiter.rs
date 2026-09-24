//! Shared rate limiter for every Gemini call: requests per minute, input tokens per minute,
//! requests per day, and a cap on calls in flight. Gemini applies limits per Google Cloud
//! project, so one limiter serves all jobs. [`AdaptiveConcurrency`] narrows a run's parallelism
//! when Gemini starts answering 429.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::model::UsageTier;

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
        Self::for_tier(UsageTier::Free)
    }
}

impl Limits {
    /// Published Gemini API RPM/TPM/RPD per tier (Free through Tier 3); concurrency is our own
    /// conservative choice, not part of Gemini's published limits. Settings (BACKLOG B1) lets
    /// the user pick the tier their Google Cloud project is actually on.
    pub fn for_tier(tier: UsageTier) -> Self {
        match tier {
            UsageTier::Free => Self {
                requests_per_minute: 60,
                tokens_per_minute: 1_000_000,
                requests_per_day: 1_000,
                max_concurrency: 4,
            },
            UsageTier::Tier1 => Self {
                requests_per_minute: 360,
                tokens_per_minute: 4_000_000,
                requests_per_day: 10_000,
                max_concurrency: 8,
            },
            UsageTier::Tier2 => Self {
                requests_per_minute: 1_000,
                tokens_per_minute: 8_000_000,
                requests_per_day: 100_000,
                max_concurrency: 16,
            },
            UsageTier::Tier3 => Self {
                requests_per_minute: 2_000,
                tokens_per_minute: 30_000_000,
                requests_per_day: 1_000_000,
                max_concurrency: 32,
            },
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

/// Calls in flight for one simulation run, adjusted to the 429 rate: above 20% of the last
/// 20 attempts it halves (at most once per 20 attempts); after 30 s without a 429 it grows
/// by one, up to `max`.
pub struct AdaptiveConcurrency {
    max: u32,
    state: std::sync::Mutex<Adaptive>,
    notify: Notify,
}

struct Adaptive {
    limit: u32,
    in_flight: u32,
    window: VecDeque<bool>,
    last_change: Instant,
    last_429: Option<Instant>,
    min_seen: u32,
}

const WINDOW: usize = 20;

/// Frees its slot when dropped.
pub struct Slot {
    owner: Arc<AdaptiveConcurrency>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.owner.state.lock().unwrap().in_flight -= 1;
        self.owner.notify.notify_waiters();
    }
}

impl AdaptiveConcurrency {
    pub fn new(max: u32) -> Arc<Self> {
        let max = max.max(1);
        Arc::new(Self {
            max,
            state: std::sync::Mutex::new(Adaptive {
                limit: max,
                in_flight: 0,
                window: VecDeque::new(),
                last_change: Instant::now(),
                last_429: None,
                min_seen: max,
            }),
            notify: Notify::new(),
        })
    }

    pub async fn acquire(self: &Arc<Self>) -> Slot {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut s = self.state.lock().unwrap();
                self.grow(&mut s);
                if s.in_flight < s.limit {
                    s.in_flight += 1;
                    return Slot {
                        owner: self.clone(),
                    };
                }
            }
            // Wake at least every second so recovery can happen while everyone waits.
            let _ = tokio::time::timeout(Duration::from_secs(1), notified).await;
        }
    }

    /// Records one attempt. Returns the new limit if it changed.
    pub fn record(&self, rate_limited: bool) -> Option<u32> {
        let mut s = self.state.lock().unwrap();
        s.window.push_back(rate_limited);
        if s.window.len() > WINDOW {
            s.window.pop_front();
        }
        if rate_limited {
            s.last_429 = Some(Instant::now());
        }
        let hits = s.window.iter().filter(|x| **x).count();
        if s.window.len() >= 5 && hits * 5 > s.window.len() && s.limit > 1 {
            s.limit = (s.limit / 2).max(1);
            s.min_seen = s.min_seen.min(s.limit);
            s.window.clear();
            s.last_change = Instant::now();
            return Some(s.limit);
        }
        let before = s.limit;
        self.grow(&mut s);
        (s.limit != before).then_some(s.limit)
    }

    fn grow(&self, s: &mut Adaptive) {
        let now = Instant::now();
        let quiet = s
            .last_429
            .map_or(true, |t| now.duration_since(t) >= Duration::from_secs(30));
        if s.limit < self.max
            && quiet
            && now.duration_since(s.last_change) >= Duration::from_secs(30)
        {
            s.limit += 1;
            s.last_change = now;
            self.notify.notify_waiters();
        }
    }

    pub fn current(&self) -> u32 {
        self.state.lock().unwrap().limit
    }

    /// Lowest limit reached so far (the 429 acceptance test checks it went down).
    pub fn min_seen(&self) -> u32 {
        self.state.lock().unwrap().min_seen
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

    #[tokio::test(start_paused = true)]
    async fn adaptive_halves_on_429s_and_recovers_slowly() {
        let ac = AdaptiveConcurrency::new(8);
        for _ in 0..4 {
            assert_eq!(ac.record(false), None);
        }
        // 2 of 6 attempts rate limited (33%) → halve.
        ac.record(true);
        assert_eq!(ac.record(true), Some(4));
        assert_eq!(ac.current(), 4);
        // At most one halving per window of attempts.
        assert_eq!(ac.record(true), None);
        // No recovery while 429s are recent.
        tokio::time::advance(Duration::from_secs(20)).await;
        assert_eq!(ac.record(false), None);
        tokio::time::advance(Duration::from_secs(31)).await;
        assert_eq!(ac.record(false), Some(5));
        assert_eq!(ac.record(false), None);
        tokio::time::advance(Duration::from_secs(31)).await;
        assert_eq!(ac.record(false), Some(6));
        assert_eq!(ac.min_seen(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn adaptive_caps_slots_in_flight() {
        let ac = AdaptiveConcurrency::new(2);
        let a = ac.acquire().await;
        let _b = ac.acquire().await;
        let ac2 = ac.clone();
        let third = tokio::spawn(async move {
            let _s = ac2.acquire().await;
        });
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!third.is_finished());
        drop(a);
        third.await.unwrap();
    }
}
