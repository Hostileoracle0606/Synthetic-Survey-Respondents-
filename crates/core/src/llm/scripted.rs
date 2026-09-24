//! `ScriptedLlm`: an in-process fake provider for engine tests (test plan §3). No HTTP;
//! each call is answered by a closure, optionally after a delay, and every request is
//! recorded so tests can check prompts, order and concurrency.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use super::{Capabilities, LlmError, LlmProvider, StructuredRequest, StructuredResponse, Usage};

type Responder = dyn Fn(&StructuredRequest, usize) -> Result<Value, LlmError> + Send + Sync;

#[derive(Clone)]
pub struct ScriptedLlm {
    responder: Arc<Responder>,
    delay: Duration,
    calls: Arc<AtomicUsize>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<StructuredRequest>>>,
    log_requests: bool,
}

impl ScriptedLlm {
    /// `responder(request, call_index)` returns the reply JSON or an error for that call.
    pub fn new(
        responder: impl Fn(&StructuredRequest, usize) -> Result<Value, LlmError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            responder: Arc::new(responder),
            delay: Duration::ZERO,
            calls: Arc::default(),
            in_flight: Arc::default(),
            max_in_flight: Arc::default(),
            requests: Arc::default(),
            log_requests: true,
        }
    }

    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Don't keep a copy of every request (memory tests measure the engine, not the fake).
    pub fn without_request_log(mut self) -> Self {
        self.log_requests = false;
        self
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> Vec<StructuredRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmProvider for ScriptedLlm {
    async fn complete_structured(
        &self,
        req: &StructuredRequest,
    ) -> Result<StructuredResponse, LlmError> {
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.log_requests {
            self.requests.lock().unwrap().push(req.clone());
        }
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        let result = (self.responder)(req, index);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        let json = result?;
        let usage = Usage {
            input_tokens: (req.system.len() + req.prompt.len()) as u32 / 4,
            cached_tokens: 0,
            output_tokens: json.to_string().len() as u32 / 4,
        };
        Ok(StructuredResponse {
            json,
            usage,
            latency_ms: self.delay.as_millis() as u64,
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            json_schema: true,
            prompt_cache: false,
            logprobs: false,
        }
    }
}
