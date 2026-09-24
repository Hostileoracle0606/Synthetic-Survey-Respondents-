//! The survey draft job (docs/DATA_FLOW.md §3, Step 3): one Gemini call drafts the core
//! questions and the suggestions from the research brief. It never sees the cohort, and the
//! reply schema has no field for expected answers.

use std::sync::Arc;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::call::with_retries;
use super::limiter::RateLimiter;
use crate::db::surveys::{self, NewQuestion};
use crate::db::writer::Writer;
use crate::error::AppResult;
use crate::llm::{LlmError, LlmProvider, StructuredRequest, StructuredResponse};
use crate::model::{
    ChoiceOption, DraftStatus, NumericRange, QuestionBody, QuestionType, ResearchType, Scale,
};

pub const PROMPT_VERSION: &str = "survey_draft.v1";
const SYSTEM: &str = include_str!("../../prompts/survey_draft.v1.md");

/// What the drafter sees: never personas, answers or the cohort.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DraftBrief {
    pub research_type: ResearchType,
    pub product_category: Option<String>,
    /// Country names, not codes.
    pub countries: Vec<String>,
    pub title: String,
    pub objective: String,
}

impl DraftBrief {
    /// Stored on the survey (`brief_json`) so the draft can be traced.
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_default()
    }
}

/// Research-type presets: (core questions, suggestions, focus).
pub fn preset(t: ResearchType) -> (usize, usize, &'static str) {
    match t {
        ResearchType::Brand => (10, 6, "awareness, consideration, brand image, loyalty"),
        ResearchType::MarketResponse => (10, 6, "purchase intent, price, drivers, barriers"),
        ResearchType::Concept => (8, 6, "concept appeal, clarity, fit, price, likely use"),
    }
}

fn research_type_label(t: ResearchType) -> &'static str {
    match t {
        ResearchType::Brand => "Brand Survey",
        ResearchType::MarketResponse => "Market Response Survey",
        ResearchType::Concept => "Concept Survey",
    }
}

fn question_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "code": { "type": "string" },
            "text": { "type": "string" },
            "type": { "type": "string", "enum": ["single_choice", "multi_choice", "likert", "numeric", "open_ended"] },
            "options": { "type": "array", "items": { "type": "string" } },
            "max_choices": { "type": "integer" },
            "scale_min": { "type": "integer" },
            "scale_max": { "type": "integer" },
            "scale_min_label": { "type": "string" },
            "scale_max_label": { "type": "string" },
            "number_min": { "type": "number" },
            "number_max": { "type": "number" },
            "unit": { "type": "string" },
            "objective": { "type": "string" },
            "rationale": { "type": "string" }
        },
        "required": ["code", "text", "type", "objective", "rationale"]
    })
}

pub fn reply_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "intro": { "type": "string" },
            "questions": { "type": "array", "items": question_schema() },
            "suggestions": { "type": "array", "items": question_schema() }
        },
        "required": ["title", "intro", "questions", "suggestions"]
    })
}

pub fn build_request(model: &str, brief: &DraftBrief) -> StructuredRequest {
    let (core, suggestions, focus) = preset(brief.research_type);
    let category = brief
        .product_category
        .as_deref()
        .map(|c| c.replace('_', " "))
        .unwrap_or_else(|| "not specified".into());
    let prompt = format!(
        "Research brief\n\
         - Research type: {} (focus: {focus})\n\
         - Product category: {category}\n\
         - Countries surveyed: {}\n\
         - Project title: {}\n\
         - Research objective and requirements:\n{}\n\n\
         Write {core} core questions and {suggestions} suggestions.\n",
        research_type_label(brief.research_type),
        brief.countries.join(", "),
        brief.title,
        brief.objective.trim(),
    );
    StructuredRequest {
        model: model.to_string(),
        system: SYSTEM.to_string(),
        prompt,
        schema: reply_schema(),
        temperature: 0.7,
        max_output_tokens: 16_384,
    }
}

/// A drafted question as Gemini returned it; `None` if it can't be made valid.
fn parse_question(v: &Value) -> Option<NewQuestion> {
    let s = |k: &str| v[k].as_str().map(str::trim).unwrap_or_default().to_string();
    let question_type = QuestionType::from_db(v["type"].as_str()?)?;
    let options: Vec<ChoiceOption> = v["options"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|o| o.as_str())
                .enumerate()
                .map(|(i, l)| ChoiceOption {
                    code: surveys::option_code(i),
                    label: l.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let body = QuestionBody {
        text: s("text"),
        question_type,
        randomize: question_type.is_choice(),
        options,
        max_choices: v["max_choices"].as_u64().map(|m| m as u32),
        scale: (question_type == QuestionType::Likert).then(|| Scale {
            min: v["scale_min"].as_i64().unwrap_or(1) as i32,
            max: v["scale_max"].as_i64().unwrap_or(5) as i32,
            min_label: s("scale_min_label"),
            max_label: s("scale_max_label"),
        }),
        numeric: (question_type == QuestionType::Numeric).then(|| NumericRange {
            min: v["number_min"].as_f64().unwrap_or(0.0),
            max: v["number_max"].as_f64().unwrap_or(100.0),
            unit: s("unit"),
        }),
    };
    let body = surveys::normalise(body).ok()?;
    let opt = |k: &str| Some(s(k)).filter(|x| !x.is_empty());
    Some(NewQuestion {
        code: s("code"),
        body,
        objective: opt("objective"),
        rationale: opt("rationale"),
    })
}

pub struct Draft {
    pub title: String,
    pub intro: String,
    pub questions: Vec<NewQuestion>,
    pub suggestions: Vec<NewQuestion>,
}

/// Keeps the valid questions. Fails (so the call is retried) when fewer than half of the
/// requested core questions are usable.
pub fn parse_reply(v: &Value, wanted_core: usize) -> Result<Draft, LlmError> {
    let list = |k: &str| -> Vec<NewQuestion> {
        v[k].as_array()
            .map(|a| a.iter().filter_map(parse_question).collect())
            .unwrap_or_default()
    };
    let questions = list("questions");
    if questions.len() * 2 < wanted_core {
        return Err(LlmError::SchemaViolation(format!(
            "only {} usable core questions; each needs text, a valid type and, for choice questions, 2–15 distinct options",
            questions.len()
        )));
    }
    Ok(Draft {
        title: v["title"].as_str().unwrap_or_default().trim().to_string(),
        intro: v["intro"].as_str().unwrap_or_default().trim().to_string(),
        questions,
        suggestions: list("suggestions"),
    })
}

pub struct DraftJob {
    pub survey_id: i64,
    pub brief: DraftBrief,
    pub model: String,
}

/// Runs the draft. The caller sets the survey to `generating` first; this sets it to `ready`
/// or `failed` (people can still write questions by hand after a failure).
pub async fn run(
    job: DraftJob,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
) -> AppResult<()> {
    let survey_id = job.survey_id;
    let outcome = draft(&job, &llm, &limiter, &writer).await;
    let error = outcome.as_ref().err().map(|e| e.to_string());
    writer
        .write(Box::new(move |c| match outcome {
            Ok(d) => {
                // Redraft replaces only questions nobody has touched.
                surveys::clear_untouched_ai(c, survey_id)?;
                for q in &d.questions {
                    surveys::insert_ai(c, survey_id, q, false)?;
                }
                for q in &d.suggestions {
                    surveys::insert_ai(c, survey_id, q, true)?;
                }
                if !d.intro.is_empty() {
                    surveys::set_drafted_intro(c, survey_id, &d.intro)?;
                }
                surveys::set_draft_status(c, survey_id, DraftStatus::Ready, None)
            }
            Err(_) => {
                surveys::set_draft_status(c, survey_id, DraftStatus::Failed, error.as_deref())
            }
        }))
        .await
}

async fn draft(
    job: &DraftJob,
    llm: &Arc<dyn LlmProvider>,
    limiter: &RateLimiter,
    writer: &Writer,
) -> Result<Draft, LlmError> {
    let (core, _, _) = preset(job.brief.research_type);
    let mut req = build_request(&job.model, &job.brief);
    let log = |attempt: u32, r: &Result<StructuredResponse, LlmError>| log_call(writer, attempt, r);
    let first = with_retries(llm, limiter, &req, log).await?;
    match parse_reply(&first.json, core) {
        Err(LlmError::SchemaViolation(problem)) => {
            req.prompt.push_str(&format!(
                "\nYour previous reply was rejected: {problem}. Follow the format exactly.\n"
            ));
            let second = with_retries(llm, limiter, &req, log).await?;
            parse_reply(&second.json, core)
        }
        other => other,
    }
}

fn log_call(writer: &Writer, attempt: u32, result: &Result<StructuredResponse, LlmError>) {
    let (usage, latency, error) = match result {
        Ok(r) => (Some(r.usage), Some(r.latency_ms as i64), None),
        Err(e) => (None, None, Some(e.to_string())),
    };
    let _ = writer.send(Box::new(move |c| {
        c.execute(
            "INSERT INTO llm_calls(purpose, attempt, input_tokens, cached_tokens, output_tokens, latency_ms, error)
             VALUES ('survey_draft', ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                attempt,
                usage.map(|u| u.input_tokens),
                usage.map(|u| u.cached_tokens),
                usage.map(|u| u.output_tokens),
                latency,
                error
            ],
        )?;
        Ok(())
    }));
}

/// Test helper: a valid draft reply with `core` questions and `suggestions` suggestions.
pub fn sample_reply(core: usize, suggestions: usize) -> Value {
    let q = |i: usize, prefix: &str| {
        let t = [
            "single_choice",
            "multi_choice",
            "likert",
            "numeric",
            "open_ended",
        ][i % 5];
        json!({
            "code": format!("{prefix}{}", i + 1),
            "text": format!("{prefix} question {} about the category?", i + 1),
            "type": t,
            "options": ["Yes", "No", "Not sure"],
            "max_choices": 2,
            "scale_min": 1, "scale_max": 7,
            "scale_min_label": "Not at all likely", "scale_max_label": "Extremely likely",
            "number_min": 0, "number_max": 2000, "unit": "CAD",
            "objective": "Purchase intent",
            "rationale": "Measures intent."
        })
    };
    json!({
        "title": "Draft",
        "intro": "Thanks for taking part. There are no right or wrong answers.",
        "questions": (0..core).map(|i| q(i, "Q")).collect::<Vec<_>>(),
        "suggestions": (0..suggestions).map(|i| q(i, "S")).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::limiter::Limits;
    use crate::llm::scripted::ScriptedLlm;
    use crate::model::{DraftStatus, QuestionOrigin, ReviewStatus};
    use std::time::Duration;

    fn brief() -> DraftBrief {
        DraftBrief {
            research_type: ResearchType::MarketResponse,
            product_category: Some("mobile_phone".into()),
            countries: vec!["Canada".into()],
            title: "Phone upgrades".into(),
            objective: "HYPOTHESIS: people upgrade for the camera.".into(),
        }
    }

    #[test]
    fn schema_has_no_place_for_expected_answers() {
        let text = reply_schema().to_string().to_lowercase();
        for banned in ["expected", "answer", "hypothes", "correct", "predict"] {
            assert!(!text.contains(banned), "schema mentions {banned}");
        }
    }

    #[test]
    fn prompt_carries_the_brief_and_preset_counts() {
        let r = build_request("m", &brief());
        assert!(r.prompt.contains("Market Response Survey"));
        assert!(r.prompt.contains("mobile phone"));
        assert!(r.prompt.contains("Canada"));
        assert!(r
            .prompt
            .contains("Write 10 core questions and 6 suggestions"));
        assert_eq!(preset(ResearchType::Concept).0, 8);
    }

    #[test]
    fn parse_drops_bad_questions_and_rejects_mostly_bad_replies() {
        let mut v = sample_reply(10, 6);
        v["questions"][0]["options"] = json!(["only one"]);
        v["questions"][1]["type"] = json!("ranking");
        let d = parse_reply(&v, 10).unwrap();
        assert_eq!(d.questions.len(), 8);
        assert_eq!(d.suggestions.len(), 6);
        assert!(parse_reply(&sample_reply(4, 0), 10).is_err());
    }

    async fn run_with(llm: ScriptedLlm) -> (tempfile::TempDir, std::path::PathBuf, i64) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.db");
        let conn = crate::db::open(&path).unwrap();
        conn.execute("INSERT INTO projects(title) VALUES ('p')", [])
            .unwrap();
        let s = surveys::for_project(&conn, 1).unwrap();
        surveys::set_draft_status(&conn, s.id, DraftStatus::Generating, None).unwrap();
        let (writer, _h) = Writer::spawn(
            crate::db::open(&path).unwrap(),
            50,
            Duration::from_millis(10),
        );
        let limiter = Arc::new(RateLimiter::new(
            Limits {
                requests_per_minute: 1000,
                tokens_per_minute: 10_000_000,
                requests_per_day: 1000,
                max_concurrency: 4,
            },
            0,
        ));
        run(
            DraftJob {
                survey_id: s.id,
                brief: brief(),
                model: "pro".into(),
            },
            Arc::new(llm),
            limiter,
            writer,
        )
        .await
        .unwrap();
        (dir, path, s.id)
    }

    #[tokio::test]
    async fn draft_saves_pending_core_questions_and_inactive_suggestions() {
        let llm = ScriptedLlm::new(|_, _| Ok(sample_reply(10, 6)));
        let (_d, path, id) = run_with(llm.clone()).await;
        let s = surveys::get(&crate::db::open(&path).unwrap(), id).unwrap();
        assert_eq!(s.draft_status, DraftStatus::Ready);
        assert_eq!(s.questions.len(), 10);
        assert_eq!(s.suggestions.len(), 6);
        assert!(s
            .questions
            .iter()
            .all(|q| q.review_status == ReviewStatus::Pending && q.origin == QuestionOrigin::Ai));
        assert!(s.intro.contains("no right or wrong"));
        // The drafter sees the brief; it never sees personas.
        assert!(!llm.requests()[0].prompt.contains("persona"));
    }

    #[tokio::test]
    async fn a_bad_reply_is_retried_once_then_the_draft_fails_cleanly() {
        let llm = ScriptedLlm::new(|_, i| {
            Ok(if i == 0 {
                json!({"title": "", "intro": "", "questions": [], "suggestions": []})
            } else {
                sample_reply(10, 6)
            })
        });
        let (_d, path, id) = run_with(llm.clone()).await;
        assert_eq!(llm.calls(), 2);
        assert!(llm.requests()[1]
            .prompt
            .contains("previous reply was rejected"));
        assert_eq!(
            surveys::get(&crate::db::open(&path).unwrap(), id)
                .unwrap()
                .questions
                .len(),
            10
        );

        let bad = ScriptedLlm::new(|_, _| {
            Ok(json!({"title": "", "intro": "", "questions": [], "suggestions": []}))
        });
        let (_d, path, id) = run_with(bad).await;
        let s = surveys::get(&crate::db::open(&path).unwrap(), id).unwrap();
        assert_eq!(s.draft_status, DraftStatus::Failed);
        assert!(s.draft_error.unwrap().contains("usable core questions"));
    }
}
