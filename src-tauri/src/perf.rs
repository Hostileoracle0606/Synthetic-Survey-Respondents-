//! Performance-harness builds only (`--features perf`): TEST_PLAN S7 `memory_1000` and S10
//! `stream_fps_1000`, BACKLOG B14. Release installers are built without this feature.
//!
//! When `SURVEY_PERF_GEMINI_URL` names a server on this machine (`survey-evals`' `mock-gemini`),
//! Gemini calls go there with a dummy key instead of the keychain's, and the rate limits are
//! lifted so a 1,000-respondent run streams at full speed.

use survey_core::engine::limiter::Limits;
use survey_core::llm::gemini::GeminiClient;

const MOCK_URL_VAR: &str = "SURVEY_PERF_GEMINI_URL";

/// The mock server's base URL, if set and on this machine (never a remote host).
fn mock_url() -> Option<String> {
    let url = std::env::var(MOCK_URL_VAR).ok()?;
    url.starts_with("http://127.0.0.1:").then_some(url)
}

pub fn mock_client() -> Option<GeminiClient> {
    mock_url().map(|url| GeminiClient::with_base_url("perf-harness", url))
}

pub fn limits(default: Limits) -> Limits {
    if mock_url().is_none() {
        return default;
    }
    Limits {
        requests_per_minute: 1_000_000,
        tokens_per_minute: u32::MAX,
        requests_per_day: 1_000_000,
        max_concurrency: 10,
    }
}
