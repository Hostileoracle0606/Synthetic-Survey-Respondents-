//! One logical Gemini request with the shared retry policy: wait on the rate limiter,
//! retry rate limits (honouring the server's delay) and temporary failures with backoff.

use std::sync::Arc;
use std::time::Duration;

use super::limiter::RateLimiter;
use crate::llm::{LlmError, LlmProvider, StructuredRequest, StructuredResponse};

pub const MAX_ATTEMPTS: u32 = 6;

/// `log(attempt, result)` is called after every attempt, e.g. to record it in `llm_calls`.
pub async fn with_retries(
    llm: &Arc<dyn LlmProvider>,
    limiter: &RateLimiter,
    req: &StructuredRequest,
    log: impl Fn(u32, &Result<StructuredResponse, LlmError>),
) -> Result<StructuredResponse, LlmError> {
    let estimate = ((req.system.len() + req.prompt.len()) / 4) as u32;
    for attempt in 1..=MAX_ATTEMPTS {
        let permit = limiter
            .acquire(estimate)
            .await
            .map_err(|e| LlmError::InvalidRequest(e.message))?;
        let result = llm.complete_structured(req).await;
        log(attempt, &result);
        match result {
            Ok(resp) => {
                limiter
                    .record_actual(&permit, resp.usage.input_tokens)
                    .await;
                return Ok(resp);
            }
            Err(LlmError::RateLimited { retry_after }) if attempt < MAX_ATTEMPTS => {
                drop(permit);
                tokio::time::sleep(backoff(attempt, retry_after)).await;
            }
            Err(LlmError::Transient(_)) if attempt < MAX_ATTEMPTS => {
                drop(permit);
                tokio::time::sleep(backoff(attempt, None)).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(LlmError::Transient(
        "gave up after repeated failures".into(),
    ))
}

/// The server's `retryDelay` when given, else 2, 4, 8, 16, 32 s.
pub fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.unwrap_or(Duration::from_secs(1 << attempt.clamp(1, 5)))
}
