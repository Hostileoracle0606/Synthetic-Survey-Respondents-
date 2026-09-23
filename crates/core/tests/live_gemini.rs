//! Live checks against the real Gemini API. Ignored by default so PR CI never calls the API;
//! run with `GEMINI_API_KEY=... cargo test -p survey-core --test live_gemini -- --ignored`.
//! Optional: `GEMINI_MODEL` picks the model for the structured-output check.

use serde_json::json;
use survey_core::llm::gemini::GeminiClient;
use survey_core::llm::{LlmProvider, StructuredRequest};

fn client() -> GeminiClient {
    let key = std::env::var("GEMINI_API_KEY").expect("set GEMINI_API_KEY to run live tests");
    GeminiClient::new(key)
}

/// Picks `GEMINI_MODEL`, else the newest stable plain Flash model (`gemini-<version>-flash`).
/// Older models can be closed to new keys, so never hard-code one.
async fn pick_model(c: &GeminiClient) -> String {
    if let Ok(m) = std::env::var("GEMINI_MODEL") {
        return m;
    }
    let models = c.list_models().await.expect("list_models");
    models
        .iter()
        .filter_map(|m| {
            let version = m.strip_prefix("gemini-")?.strip_suffix("-flash")?;
            let parts: Vec<u32> = version
                .split('.')
                .map(|p| p.parse().ok())
                .collect::<Option<_>>()?;
            Some((parts, m.clone()))
        })
        .max()
        .map(|(_, m)| m)
        .expect("no stable flash model available to this key")
}

#[tokio::test]
#[ignore = "calls the live Gemini API"]
async fn live_list_models() {
    let models = client()
        .list_models()
        .await
        .expect("the key can list models");
    assert!(!models.is_empty());
    eprintln!(
        "{} models available; flash models: {:?}",
        models.len(),
        models
            .iter()
            .filter(|m| m.contains("flash"))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "calls the live Gemini API"]
async fn live_structured_output() {
    let c = client();
    let model = pick_model(&c).await;
    let req = StructuredRequest {
        model: model.clone(),
        system: "You are a survey respondent: a 34-year-old nurse in Ohio who keeps phones until they break.".into(),
        prompt: "Q1: How likely are you to replace your phone in the next 12 months? Answer on a 1-7 scale, with one sentence of reasoning.".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "q1": { "type": "integer", "minimum": 1, "maximum": 7 },
                "reason": { "type": "string" }
            },
            "required": ["q1", "reason"]
        }),
        temperature: 1.0,
        max_output_tokens: 1024,
    };
    let resp = c
        .complete_structured(&req)
        .await
        .expect("structured call succeeds");
    let q1 = resp.json["q1"].as_i64().expect("q1 is an integer");
    assert!((1..=7).contains(&q1), "q1 out of range: {q1}");
    assert!(resp.json["reason"].as_str().is_some_and(|r| !r.is_empty()));
    assert!(resp.usage.input_tokens > 0 && resp.usage.output_tokens > 0);
    eprintln!(
        "model {model}: {} in {} ms, usage {:?}",
        resp.json, resp.latency_ms, resp.usage
    );
}
