//! Background jobs. Each lands in a later milestone (see docs/IMPLEMENTATION_PLAN.md):
//!
//! - `cohort` (M2): sample skeletons, enrich with Gemini Flash, screen, save per batch.
//! - `survey_draft` (M3): draft core questions and suggestions with Gemini Pro; run the critic.
//! - `run` (M3): worker pool with rate limiter, retries, pause, stop and resume.
//! - `synthesis` (M4): summarise results with Gemini Pro and check every number it cites.
//!
//! All jobs share one rate limiter and write through [`crate::db::writer::Writer`].
