//! Gemini `generateContent` adapter. Uses `generateContent` rather than the Interactions API
//! because the latter doesn't return log-probabilities (docs/SPEC.md §5).

use std::fmt;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Capabilities, LlmError, LlmProvider, StructuredRequest, StructuredResponse, Usage};

pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

pub struct GeminiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl fmt::Debug for GeminiClient {
    // Never print the key, even in debug logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeminiClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl GeminiClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL)
    }

    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("HTTP client builds");
        Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    /// Used by `test_connection`: lists model IDs the key can use.
    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let resp = self
            .http
            .get(format!("{}/v1beta/models", self.base_url))
            .header("x-goog-api-key", &self.api_key)
            .send()
            .await
            .map_err(|e| LlmError::Transient(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let retry_after = retry_after_header(&resp);
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if status != 200 {
            return Err(classify_error(status, &body, retry_after));
        }
        Ok(body["models"]
            .as_array()
            .map(|ms| {
                ms.iter()
                    .filter_map(|m| m["name"].as_str())
                    .map(|n| n.trim_start_matches("models/").to_string())
                    .collect()
            })
            .unwrap_or_default())
    }
}

#[async_trait]
impl LlmProvider for GeminiClient {
    async fn complete_structured(
        &self,
        req: &StructuredRequest,
    ) -> Result<StructuredResponse, LlmError> {
        let started = Instant::now();
        let resp = self
            .http
            .post(format!(
                "{}/v1beta/models/{}:generateContent",
                self.base_url, req.model
            ))
            .header("x-goog-api-key", &self.api_key)
            .json(&request_body(req))
            .send()
            .await
            .map_err(|e| LlmError::Transient(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let retry_after = retry_after_header(&resp);
        let body: Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Transient(format!("unreadable body: {e}")))?;
        if status != 200 {
            return Err(classify_error(status, &body, retry_after));
        }
        let (json, usage) = parse_response(&body)?;
        Ok(StructuredResponse {
            json,
            usage,
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            json_schema: true,
            prompt_cache: true,
            logprobs: false,
        }
    }
}

pub fn request_body(req: &StructuredRequest) -> Value {
    json!({
        "systemInstruction": { "parts": [{ "text": req.system }] },
        "contents": [{ "role": "user", "parts": [{ "text": req.prompt }] }],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseJsonSchema": req.schema,
            "temperature": req.temperature,
            "maxOutputTokens": req.max_output_tokens,
        }
    })
}

/// Extracts the JSON reply and token usage from a 200 response.
pub fn parse_response(body: &Value) -> Result<(Value, Usage), LlmError> {
    if let Some(reason) = body["promptFeedback"]["blockReason"].as_str() {
        return Err(LlmError::Blocked(reason.to_string()));
    }
    let candidate = &body["candidates"][0];
    if candidate.is_null() {
        return Err(LlmError::Blocked("no candidates returned".into()));
    }
    let finish = candidate["finishReason"].as_str().unwrap_or("");
    if matches!(
        finish,
        "SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "SPII" | "RECITATION"
    ) {
        return Err(LlmError::Blocked(finish.to_string()));
    }
    let text: String = candidate["content"]["parts"]
        .as_array()
        .map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect())
        .unwrap_or_default();
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        let why = if finish == "MAX_TOKENS" {
            "reply was cut off at the token limit".to_string()
        } else {
            e.to_string()
        };
        LlmError::SchemaViolation(why)
    })?;
    let u = &body["usageMetadata"];
    let n = |v: &Value| v.as_u64().unwrap_or(0) as u32;
    let usage = Usage {
        input_tokens: n(&u["promptTokenCount"]),
        cached_tokens: n(&u["cachedContentTokenCount"]),
        // Thinking tokens are billed as output.
        output_tokens: n(&u["candidatesTokenCount"]) + n(&u["thoughtsTokenCount"]),
    };
    Ok((json, usage))
}

pub fn classify_error(status: u16, body: &Value, retry_after: Option<Duration>) -> LlmError {
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or("no message")
        .to_string();
    let details = body["error"]["details"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let key_invalid = details
        .iter()
        .any(|d| d["reason"].as_str() == Some("API_KEY_INVALID"));
    match status {
        429 => LlmError::RateLimited {
            retry_after: retry_after.or_else(|| retry_delay_from_details(&details)),
        },
        401 | 403 => LlmError::Auth(message),
        400 if key_invalid => LlmError::Auth(message),
        400 | 404 => LlmError::InvalidRequest(message),
        _ => LlmError::Transient(format!("HTTP {status}: {message}")),
    }
}

/// Gemini puts the wait in a `RetryInfo` detail, e.g. `"retryDelay": "12s"`.
fn retry_delay_from_details(details: &[Value]) -> Option<Duration> {
    details.iter().find_map(|d| {
        let s = d["retryDelay"].as_str()?;
        s.strip_suffix('s')?
            .parse::<f64>()
            .ok()
            .map(Duration::from_secs_f64)
    })
}

fn retry_after_header(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn req() -> StructuredRequest {
        StructuredRequest {
            model: "gemini-test-flash".into(),
            system: "Answer as the persona.".into(),
            prompt: "Q1?".into(),
            schema: json!({ "type": "object", "properties": { "q1": { "type": "string" } }, "required": ["q1"] }),
            temperature: 1.0,
            max_output_tokens: 512,
        }
    }

    fn ok_body() -> Value {
        json!({
            "candidates": [{ "content": { "parts": [{ "text": "{\"q1\":\"B\"}" }] }, "finishReason": "STOP" }],
            "usageMetadata": { "promptTokenCount": 900, "cachedContentTokenCount": 600, "candidatesTokenCount": 40, "thoughtsTokenCount": 10 }
        })
    }

    #[test]
    fn request_uses_json_mode_and_schema() {
        let body = request_body(&req());
        assert_eq!(
            body["generationConfig"]["responseMimeType"],
            "application/json"
        );
        assert_eq!(
            body["generationConfig"]["responseJsonSchema"]["required"][0],
            "q1"
        );
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "Answer as the persona."
        );
    }

    #[test]
    fn parses_reply_and_usage() {
        let (json, usage) = parse_response(&ok_body()).unwrap();
        assert_eq!(json["q1"], "B");
        assert_eq!(
            usage,
            Usage {
                input_tokens: 900,
                cached_tokens: 600,
                output_tokens: 50
            }
        );
    }

    #[test]
    fn safety_blocks_and_bad_json_are_classified() {
        let blocked = json!({ "candidates": [{ "finishReason": "SAFETY" }] });
        assert!(matches!(
            parse_response(&blocked),
            Err(LlmError::Blocked(_))
        ));
        let prompt_blocked = json!({ "promptFeedback": { "blockReason": "OTHER" } });
        assert!(matches!(
            parse_response(&prompt_blocked),
            Err(LlmError::Blocked(_))
        ));
        let cut = json!({ "candidates": [{ "content": { "parts": [{ "text": "{\"q1\":" }] }, "finishReason": "MAX_TOKENS" }] });
        assert!(
            matches!(parse_response(&cut), Err(LlmError::SchemaViolation(m)) if m.contains("token limit"))
        );
    }

    #[test]
    fn http_errors_are_classified() {
        let rate = json!({ "error": { "message": "quota", "details": [{ "@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "12s" }] } });
        assert_eq!(
            classify_error(429, &rate, None),
            LlmError::RateLimited {
                retry_after: Some(Duration::from_secs(12))
            }
        );
        let bad_key = json!({ "error": { "message": "API key not valid", "details": [{ "reason": "API_KEY_INVALID" }] } });
        assert!(matches!(
            classify_error(400, &bad_key, None),
            LlmError::Auth(_)
        ));
        assert!(matches!(
            classify_error(400, &json!({}), None),
            LlmError::InvalidRequest(_)
        ));
        assert!(matches!(
            classify_error(503, &json!({}), None),
            LlmError::Transient(_)
        ));
    }

    #[test]
    fn debug_output_hides_the_key() {
        let c = GeminiClient::new("sk-TEST-CANARY-123");
        assert!(!format!("{c:?}").contains("CANARY"));
    }

    #[tokio::test]
    async fn end_to_end_against_mock_server() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/models/gemini-test-flash:generateContent"))
            .and(header("x-goog-api-key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;
        let client = GeminiClient::with_base_url("test-key", server.uri());
        let resp = client.complete_structured(&req()).await.unwrap();
        assert_eq!(resp.json["q1"], "B");
        assert_eq!(resp.usage.cached_tokens, 600);
    }

    #[tokio::test]
    async fn rate_limit_header_is_honoured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "3")
                    .set_body_json(json!({ "error": { "message": "slow down" } })),
            )
            .mount(&server)
            .await;
        let client = GeminiClient::with_base_url("k", server.uri());
        let err = client.complete_structured(&req()).await.unwrap_err();
        assert_eq!(
            err,
            LlmError::RateLimited {
                retry_after: Some(Duration::from_secs(3))
            }
        );
    }
}

/// Stable models of a family ("flash" or "pro") as (version, id): `gemini-<version>-<family>`,
/// skipping previews, lite and dated variants.
fn stable(models: &[String], family: &str) -> Vec<(Vec<u32>, String)> {
    let suffix = format!("-{family}");
    models
        .iter()
        .filter_map(|m| {
            let version = m.strip_prefix("gemini-")?.strip_suffix(suffix.as_str())?;
            let parts: Vec<u32> = version
                .split('.')
                .map(|p| p.parse().ok())
                .collect::<Option<_>>()?;
            Some((parts, m.clone()))
        })
        .collect()
}

/// The newest stable model of a family. New API keys can't use older models, so the app
/// picks from what `list_models` returns instead of hard-coding one.
pub fn newest_stable(models: &[String], family: &str) -> Option<String> {
    stable(models, family).into_iter().max().map(|(_, m)| m)
}

/// Model for drafting, theme coding and synthesis: the newest stable Pro, unless it is an
/// older generation than the newest stable Flash (older generations get closed to new keys,
/// e.g. `gemini-2.5-pro` once 3.x exists), in which case that Flash.
pub fn drafting_model(models: &[String]) -> Option<String> {
    let flash = stable(models, "flash").into_iter().max();
    let pro = stable(models, "pro").into_iter().max();
    match (pro, flash) {
        (Some(p), Some(f)) if p.0 >= f.0 => Some(p.1),
        (Some(p), None) => Some(p.1),
        (_, Some(f)) => Some(f.1),
        (None, None) => None,
    }
}

pub fn newest_stable_flash(models: &[String]) -> Option<String> {
    newest_stable(models, "flash")
}

#[cfg(test)]
mod pick_tests {
    use super::{drafting_model, newest_stable, newest_stable_flash};

    #[test]
    fn picks_the_highest_plain_flash_version() {
        let models: Vec<String> = [
            "gemini-2.5-flash",
            "gemini-3.8-flash",
            "gemini-3.10-flash-lite",
            "gemini-3.9-flash-preview",
            "gemini-3.7-flash",
            "gemini-3.1-pro",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            newest_stable_flash(&models).as_deref(),
            Some("gemini-3.8-flash")
        );
        assert_eq!(newest_stable_flash(&[]), None);
        assert_eq!(
            newest_stable(&models, "pro").as_deref(),
            Some("gemini-3.1-pro")
        );
        // Pro 3.1 is older than Flash 3.8: draft with the Flash (seen live: an old Pro
        // generation was closed to new keys).
        assert_eq!(drafting_model(&models).as_deref(), Some("gemini-3.8-flash"));
        let mut newer_pro = models.clone();
        newer_pro.push("gemini-3.9-pro".into());
        assert_eq!(
            drafting_model(&newer_pro).as_deref(),
            Some("gemini-3.9-pro")
        );
        assert_eq!(drafting_model(&[]), None);
    }
}
