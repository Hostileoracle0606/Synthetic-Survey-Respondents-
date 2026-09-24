//! Background jobs and their building blocks (see docs/IMPLEMENTATION_PLAN.md):
//!
//! - `cohort` (M2): sample skeletons, enrich with Gemini Flash, screen, save per batch.
//! - `draft` (M3): draft core questions and suggestions with Gemini (Pro when available).
//! - `critic` (M3): flag leading, double-barrelled or unclear wording on each question.
//! - `run` (M3): worker pool with rate limiter, retries, pause, stop and resume.
//! - `synthesis` (M4): code open answers into themes, then summarise with Gemini and check
//!   every number, mention count and segment it cites.
//!
//! All jobs share one [`limiter::RateLimiter`] and write through [`crate::db::writer::Writer`].

pub mod answer;
pub mod call;
pub mod cohort;
pub mod critic;
pub mod draft;
pub mod limiter;
pub mod persona;
pub mod run;
pub mod synthesis;
