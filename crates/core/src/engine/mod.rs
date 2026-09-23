//! Background jobs and their building blocks (see docs/IMPLEMENTATION_PLAN.md):
//!
//! - `cohort` (M2): sample skeletons, enrich with Gemini Flash, screen, save per batch.
//! - `draft` (M3): draft core questions and suggestions with Gemini (Pro when available).
//! - `run` (M3): worker pool with rate limiter, retries, pause, stop and resume.
//! - `synthesis` (M4): summarise results with Gemini Pro and check every number it cites.
//!
//! All jobs share one [`limiter::RateLimiter`] and write through [`crate::db::writer::Writer`].

pub mod answer;
pub mod call;
pub mod cohort;
pub mod draft;
pub mod limiter;
pub mod persona;
pub mod run;
