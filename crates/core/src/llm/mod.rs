//! LLM provider layer. v1 ships one adapter, for Gemini (see docs/SPEC.md §5).

pub mod gemini;
pub mod scripted;

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A request whose reply must match `schema` (JSON Schema, Gemini-supported subset).
#[derive(Debug, Clone, PartialEq)]
pub struct StructuredRequest {
    pub model: String,
    pub system: String,
    pub prompt: String,
    pub schema: serde_json::Value,
    pub temperature: f32,
    pub max_output_tokens: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub cached_tokens: u32,
    pub output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructuredResponse {
    pub json: serde_json::Value,
    pub usage: Usage,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub json_schema: bool,
    pub prompt_cache: bool,
    /// Probed per model by `test_connection`; not every Gemini model returns log-probabilities.
    pub logprobs: bool,
}

/// Classified so the engine can decide: retry, slow down, pause the run, or store as refused.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LlmError {
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("temporary failure: {0}")]
    Transient(String),
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("reply did not match the schema: {0}")]
    SchemaViolation(String),
    #[error("blocked by safety filters: {0}")]
    Blocked(String),
}

impl From<LlmError> for crate::AppError {
    fn from(e: LlmError) -> Self {
        crate::AppError::new(crate::ErrorCode::Llm, e.to_string())
    }
}

#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete_structured(
        &self,
        req: &StructuredRequest,
    ) -> Result<StructuredResponse, LlmError>;
    fn capabilities(&self) -> Capabilities;
}
