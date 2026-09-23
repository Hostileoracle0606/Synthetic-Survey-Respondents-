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

/// Picks `GEMINI_MODEL`, else the newest stable Flash model the key can use.
async fn pick_model(c: &GeminiClient) -> String {
    if let Ok(m) = std::env::var("GEMINI_MODEL") {
        return m;
    }
    let models = c.list_models().await.expect("list_models");
    survey_core::llm::gemini::newest_stable_flash(&models)
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

#[tokio::test]
#[ignore = "calls the live Gemini API"]
async fn live_persona_batch() {
    use survey_core::engine::persona::{build_request, parse_reply, BatchInput};
    use survey_core::model::{CohortConfig, QuotaGroup, QuotaRow};
    use survey_core::sampling::Sampler;

    let c = client();
    let model = pick_model(&c).await;
    let cfg = CohortConfig {
        size: 4,
        seed: 2026,
        quotas: vec![QuotaGroup {
            key: "age".into(),
            label: "Age".into(),
            rows: vec![
                QuotaRow {
                    label: "18–29".into(),
                    percent: 50,
                },
                QuotaRow {
                    label: "60+".into(),
                    percent: 50,
                },
            ],
        }],
        screening: String::new(),
    };
    let skeletons = Sampler::new(&cfg, &["CA".into()]).unwrap().draw().unwrap();
    let req = build_request(
        &model,
        &BatchInput {
            skeletons: &skeletons,
            category: Some("mobile_phone"),
            screening: "Owns a smartphone.",
            already_written: &[],
        },
    );
    let resp = c
        .complete_structured(&req)
        .await
        .expect("persona call succeeds");
    let ordinals: Vec<u32> = skeletons.iter().map(|s| s.ordinal).collect();
    let personas =
        parse_reply(&resp.json, &ordinals, Some("mobile_phone")).expect("reply passes checks");
    for p in &personas {
        eprintln!(
            "{} — {} | trigger {:?} | screen {}",
            p.name,
            p.summary.chars().take(90).collect::<String>(),
            p.category_profile.get("upgrade_trigger"),
            p.passes_screen
        );
    }
    eprintln!("usage {:?}, {} ms", resp.usage, resp.latency_ms);
}

/// M2 exit check: a 200-person Canadian cohort generated end to end through Gemini, with the
/// kept respondents matching every quota row exactly. Takes several minutes at default limits.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "calls the live Gemini API (about 25 requests)"]
async fn live_cohort_200_matches_quotas() {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use survey_core::db::{cohorts, writer::Writer};
    use survey_core::engine::cohort::{run, CohortJob};
    use survey_core::engine::limiter::{Limits, RateLimiter};
    use survey_core::engine::persona::PROMPT_VERSION;
    use survey_core::model::CohortConfig;
    use survey_core::sampling::{apportion, census_default_quotas};

    let c = client();
    let model = pick_model(&c).await;
    let config = CohortConfig {
        size: 200,
        seed: 2026,
        quotas: census_default_quotas("CA").expect("CA census table"),
        screening: "Owns a smartphone.".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.db");
    let conn = survey_core::db::open(&path).unwrap();
    conn.execute("INSERT INTO projects(title) VALUES ('live')", [])
        .unwrap();
    let cohort = cohorts::create(&conn, 1, &config, None, &model, PROMPT_VERSION).unwrap();
    let (writer, _h) = Writer::spawn(
        survey_core::db::open(&path).unwrap(),
        50,
        Duration::from_millis(50),
    );
    let job = CohortJob {
        cohort_id: cohort.id,
        countries: vec!["CA".into()],
        category: Some("mobile_phone".into()),
        config: config.clone(),
        model: model.clone(),
        batch_size: 8,
        max_screen_attempts: 3,
    };
    let started = Instant::now();
    run(
        job,
        Arc::new(c),
        Arc::new(RateLimiter::new(Limits::default(), 0)),
        writer,
        Arc::new(|_| {}),
    )
    .await
    .expect("cohort job succeeds");

    let kept: Vec<String> = conn
        .prepare(
            "SELECT quota_cell FROM respondents WHERE cohort_id = ?1 AND screen_status <> 'failed'",
        )
        .unwrap()
        .query_map([cohort.id], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(kept.len(), 200);
    for g in &config.quotas {
        let want = apportion(&g.rows.iter().map(|r| r.percent).collect::<Vec<_>>(), 200).unwrap();
        let mut got: HashMap<&str, u32> = HashMap::new();
        for cell in &kept {
            let v = cell
                .split('|')
                .find_map(|kv| kv.strip_prefix(&format!("{}=", g.key)))
                .expect("quota key in cell");
            *got.entry(v).or_default() += 1;
        }
        for (row, n) in g.rows.iter().zip(want) {
            assert_eq!(
                got.get(row.label.as_str()).copied().unwrap_or(0),
                n,
                "{} / {}",
                g.key,
                row.label
            );
        }
    }
    let (calls, out_tokens, flagged, failed): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM llm_calls), (SELECT COALESCE(SUM(output_tokens),0) FROM llm_calls),
                    (SELECT COUNT(*) FROM respondents WHERE screen_status = 'flagged'),
                    (SELECT COUNT(*) FROM respondents WHERE screen_status = 'failed')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    eprintln!(
        "model {model}: 200 kept in {:.0}s; {calls} calls, {out_tokens} output tokens, {failed} failed screening, {flagged} flagged",
        started.elapsed().as_secs_f64()
    );
}
