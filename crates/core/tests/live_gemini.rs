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
        non_binary_share: 0,
        countries: vec!["CA".into()],
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
        non_binary_share: 0,
        countries: vec!["CA".into()],
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

/// M3: the draft prompt returns a usable questionnaire on the real API (Pro when the key has
/// it, else Flash), with no expected answers anywhere in the reply.
#[tokio::test]
#[ignore = "calls the live Gemini API"]
async fn live_survey_draft() {
    use survey_core::engine::draft::{build_request, parse_reply, preset, DraftBrief};
    use survey_core::model::ResearchType;
    let c = client();
    let models = c.list_models().await.expect("list_models");
    let model = survey_core::llm::gemini::drafting_model(&models).expect("a model");
    let brief = DraftBrief {
        research_type: ResearchType::MarketResponse,
        product_category: Some("mobile_phone".into()),
        countries: vec!["Canada".into()],
        title: "Smartphone upgrade intent".into(),
        objective: "Understand what drives Canadians to replace their smartphone, what they would pay, and what holds them back.".into(),
    };
    let resp = c
        .complete_structured(&build_request(&model, &brief))
        .await
        .expect("draft call succeeds");
    let draft = parse_reply(&resp.json, preset(brief.research_type).0).expect("usable draft");
    assert!(
        draft.questions.len() >= 8,
        "{} core questions",
        draft.questions.len()
    );
    let text = resp.json.to_string().to_lowercase();
    assert!(!text.contains("expected answer"));
    eprintln!(
        "model {model}: {} core, {} suggestions, {} ms, usage {:?}",
        draft.questions.len(),
        draft.suggestions.len(),
        resp.latency_ms,
        resp.usage
    );
}

/// M3: one whole-survey answering call on the real API passes every answer check.
#[tokio::test]
#[ignore = "calls the live Gemini API"]
async fn live_whole_survey_answer() {
    use survey_core::engine::answer::{build_request, check_reply, shown_order};
    use survey_core::model::{
        ChoiceOption, NumericRange, Question, QuestionBody, QuestionOrigin, QuestionType,
        ReviewStatus, Scale,
    };
    let c = client();
    let model = pick_model(&c).await;
    let q = |id: i64, t: QuestionType, text: &str| Question {
        id,
        code: format!("Q{id}"),
        order_index: id as u32,
        body: QuestionBody {
            text: text.into(),
            question_type: t,
            options: if matches!(t, QuestionType::SingleChoice | QuestionType::MultiChoice) {
                [
                    "Battery life",
                    "Camera",
                    "Price drop",
                    "Phone broke",
                    "None of these",
                ]
                .iter()
                .enumerate()
                .map(|(i, l)| ChoiceOption {
                    code: ((b'A' + i as u8) as char).to_string(),
                    label: l.to_string(),
                })
                .collect()
            } else {
                vec![]
            },
            randomize: true,
            max_choices: Some(2),
            scale: (t == QuestionType::Likert).then(|| Scale {
                min: 1,
                max: 7,
                min_label: "Not at all likely".into(),
                max_label: "Extremely likely".into(),
            }),
            numeric: (t == QuestionType::Numeric).then(|| NumericRange {
                min: 0.0,
                max: 3000.0,
                unit: "CAD".into(),
            }),
        },
        is_active: true,
        origin: QuestionOrigin::Ai,
        review_status: ReviewStatus::Accepted,
        objective: None,
        rationale: None,
        critique: None,
    };
    let qs = [
        q(
            1,
            QuestionType::SingleChoice,
            "What would most likely make you replace your phone?",
        ),
        q(
            2,
            QuestionType::MultiChoice,
            "Which of these matter when choosing a new phone?",
        ),
        q(
            3,
            QuestionType::Likert,
            "How likely are you to buy a new phone in the next 12 months?",
        ),
        q(
            4,
            QuestionType::Numeric,
            "What is the most you would pay for your next phone?",
        ),
        q(
            5,
            QuestionType::OpenEnded,
            "What, if anything, puts you off upgrading?",
        ),
    ];
    let pairs: Vec<_> = qs.iter().map(|x| (x, shown_order(x, 7, 1))).collect();
    let persona = "Name: Marie Tremblay\nAge: 58\nGender: Female\nLives in: Quebec, Canada\nHousehold income: $50k–$100k (yearly, local currency)\nOccupation group: Sales & office\n\nMarie keeps her phone until it stops working and dislikes paying for features she won't use.\n";
    let resp = c
        .complete_structured(&build_request(
            &model,
            "Thanks for taking part.",
            &pairs,
            persona,
        ))
        .await
        .expect("answer call succeeds");
    let checked = check_reply(&pairs, &resp.json);
    for r in &checked {
        assert!(r.is_ok(), "{r:?} in {}", resp.json);
    }
    eprintln!(
        "model {model}: {} in {} ms, usage {:?}",
        resp.json, resp.latency_ms, resp.usage
    );
}

/// B9: the critic prompt on the real API (Pro when the key has it) flags an obviously
/// leading, double-barrelled question and returns no unknown issue kinds.
#[tokio::test]
#[ignore = "calls the live Gemini API"]
async fn live_critic() {
    use survey_core::engine::critic::{build_request, parse_reply};
    use survey_core::model::{ChoiceOption, CriticIssue, QuestionBody, QuestionType};
    let c = client();
    let models = c.list_models().await.expect("list_models");
    let model = survey_core::llm::gemini::drafting_model(&models).expect("a model");
    let body = QuestionBody {
        text: "Don't you agree that our amazing new phone's battery and camera are better than anything else?".into(),
        question_type: QuestionType::SingleChoice,
        options: ["Yes", "Definitely yes"]
            .iter()
            .enumerate()
            .map(|(i, l)| ChoiceOption {
                code: ((b'A' + i as u8) as char).to_string(),
                label: l.to_string(),
            })
            .collect(),
        randomize: false,
        max_choices: None,
        scale: None,
        numeric: None,
    };
    let resp = c
        .complete_structured(&build_request(&model, &body))
        .await
        .expect("critic call succeeds");
    let flags = parse_reply(&resp.json).expect("reply has a flags list");
    assert!(
        flags.iter().any(|f| f.issue == CriticIssue::Leading),
        "no leading flag in {}",
        resp.json
    );
    eprintln!(
        "model {model}: {:?} in {} ms, usage {:?}",
        flags, resp.latency_ms, resp.usage
    );
}

/// SPEC §5 prompt caching (B13): answer calls for one survey share a long prefix (rules, then
/// the survey), so Gemini's implicit cache should serve part of the input from the second
/// call on. The survey is made long enough to pass the model's minimum cacheable prefix
/// (4,096 tokens for Gemini 3.x Flash). Cache hits are best effort, so up to five calls are
/// made; the cached-token counts go to the job summary either way.
#[tokio::test]
#[ignore = "calls the live Gemini API (up to 5 requests)"]
async fn live_implicit_cache_hits() {
    use std::io::Write;
    use survey_core::engine::answer::build_request;
    use survey_core::model::{
        ChoiceOption, Question, QuestionBody, QuestionOrigin, QuestionType, ReviewStatus,
    };
    let c = client();
    let model = pick_model(&c).await;
    let topics = [
        "battery life",
        "camera quality",
        "screen size",
        "storage",
        "price",
        "brand reputation",
        "trade-in value",
        "carrier deals",
        "durability",
        "software updates",
        "privacy",
        "charging speed",
        "weight",
        "design",
        "repairability",
        "resale value",
        "5G coverage",
        "accessories",
        "warranty",
        "store experience",
        "online reviews",
        "friends' advice",
        "environmental impact",
        "financing options",
        "security features",
        "audio quality",
        "gaming performance",
        "water resistance",
        "wireless charging",
        "headphone jack",
        "biometric unlock",
        "display brightness",
        "refresh rate",
        "satellite messaging",
        "dual SIM support",
        "AI features",
        "stylus support",
        "foldable design",
        "customer service",
        "availability in stores",
    ];
    let qs: Vec<Question> = topics
        .iter()
        .enumerate()
        .map(|(i, t)| Question {
            id: i as i64 + 1,
            code: format!("Q{}", i + 1),
            order_index: i as u32 + 1,
            body: QuestionBody {
                text: format!(
                    "Thinking about the next time you choose a smartphone, how much would {t} influence which model you buy, compared with everything else you consider?"
                ),
                question_type: QuestionType::SingleChoice,
                options: [
                    "It would be the single most important factor in my decision",
                    "It would be one of the two or three most important factors",
                    "It would matter somewhat, but other things would matter more",
                    "It would matter only if everything else were equal",
                    "It would not matter to me at all",
                    "I don't know or haven't thought about it",
                ]
                .iter()
                .enumerate()
                .map(|(j, l)| ChoiceOption {
                    code: ((b'A' + j as u8) as char).to_string(),
                    label: l.to_string(),
                })
                .collect(),
                // Same option order for everyone, so the whole survey is a shared prefix.
                randomize: false,
                max_choices: None,
                scale: None,
                numeric: None,
            },
            is_active: true,
            origin: QuestionOrigin::Ai,
            review_status: ReviewStatus::Accepted,
            objective: None,
            rationale: None,
            critique: None,
        })
        .collect();
    let pairs: Vec<_> = qs
        .iter()
        .map(|q| (q, q.body.options.iter().map(|o| o.code.clone()).collect()))
        .collect();
    let people = [
        "Name: Marie Tremblay\nAge: 58\nGender: Female\nLives in: Quebec, Canada\n",
        "Name: Daniel Okafor\nAge: 27\nGender: Male\nLives in: Ontario, Canada\n",
        "Name: Priya Nair\nAge: 41\nGender: Female\nLives in: British Columbia, Canada\n",
        "Name: Tom Brennan\nAge: 66\nGender: Male\nLives in: Atlantic, Canada\n",
        "Name: Sofia Reyes\nAge: 33\nGender: Female\nLives in: Prairies, Canada\n",
    ];
    let mut report = Vec::new();
    let mut hit = false;
    for (i, persona) in people.iter().enumerate() {
        let req = build_request(&model, "Thanks for taking part.", &pairs, persona);
        let resp = c
            .complete_structured(&req)
            .await
            .expect("answer call succeeds");
        report.push(format!(
            "call {}: {} input tokens, {} cached",
            i + 1,
            resp.usage.input_tokens,
            resp.usage.cached_tokens
        ));
        if i > 0 && resp.usage.cached_tokens > 0 {
            hit = true;
            break;
        }
    }
    let summary = format!(
        "### Gemini implicit cache ({model})\n\n{}\n\nCache hit: {}\n",
        report
            .iter()
            .map(|l| format!("- {l}"))
            .collect::<Vec<_>>()
            .join("\n"),
        if hit { "yes" } else { "no" }
    );
    eprintln!("{summary}");
    if let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") {
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(path) {
            let _ = writeln!(f, "{summary}");
        }
    }
    assert!(
        hit,
        "no cached tokens in {} calls: {report:?}",
        report.len()
    );
}
