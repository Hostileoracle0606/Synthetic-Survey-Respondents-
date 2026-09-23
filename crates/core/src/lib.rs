//! Core of the synthetic survey harness: storage, sampling, the Gemini adapter and the
//! types shared with the UI. Nothing here depends on Tauri, so it builds and tests on any OS.
//!
//! The wizard flow this implements is described in `docs/DATA_FLOW.md`.

pub mod countries;
pub mod db;
pub mod engine;
pub mod error;
pub mod llm;
pub mod model;
pub mod sampling;

pub use error::{AppError, AppResult, ErrorCode};
