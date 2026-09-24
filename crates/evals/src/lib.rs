//! Offline evaluation tools (never shipped with the app).
//!
//! - `fidelity`: builds and runs the fidelity benchmark (TEST_PLAN S13).
//! - `prompt-evals`: prompt-behaviour evals judged by Gemini (TEST_PLAN S12).
//!
//! Both read the key from `GEMINI_API_KEY` and use the app's own prompts and checks.

pub mod judge;

use std::sync::Arc;

use survey_core::engine::limiter::{Limits, RateLimiter};
use survey_core::llm::gemini::{newest_stable, GeminiClient};
use survey_core::AppError;

pub fn client() -> Result<GeminiClient, String> {
    let key = std::env::var("GEMINI_API_KEY").map_err(|_| "set GEMINI_API_KEY".to_string())?;
    Ok(GeminiClient::new(key))
}

/// (Flash for respondents, Pro for judging and drafting, falling back to Flash).
pub async fn models(c: &GeminiClient) -> Result<(String, String), String> {
    let list = c.list_models().await.map_err(|e| e.to_string())?;
    let flash = newest_stable(&list, "flash").ok_or("no stable Flash model for this key")?;
    let pro = newest_stable(&list, "pro").unwrap_or_else(|| flash.clone());
    Ok((flash, pro))
}

pub fn limiter(concurrency: u32) -> Arc<RateLimiter> {
    Arc::new(RateLimiter::new(
        Limits {
            max_concurrency: concurrency,
            ..Limits::default()
        },
        0,
    ))
}

pub fn err(e: AppError) -> String {
    e.message
}

/// `--name value` from argv.
pub fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}
