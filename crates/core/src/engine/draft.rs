//! The survey draft job (docs/DATA_FLOW.md §3, Step 3): one Gemini call drafts the core
//! questions and the suggestions from the research brief. It never sees the cohort, and the
//! reply schema has no field for expected answers.

use std::sync::{Arc, Mutex};

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::call::with_retries;
use super::critic;
use super::limiter::RateLimiter;
use crate::db::surveys::{self, CriticTarget, NewQuestion};
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

/// The brief as the drafter reads it. Everything after it is the instruction.
fn brief_text(brief: &DraftBrief) -> String {
    let (_, _, focus) = preset(brief.research_type);
    let category = brief
        .product_category
        .as_deref()
        .map(|c| c.replace('_', " "))
        .unwrap_or_else(|| "not specified".into());
    format!(
        "Research brief\n\
         - Research type: {} (focus: {focus})\n\
         - Product category: {category}\n\
         - Countries surveyed: {}\n\
         - Project title: {}\n\
         - Research objective and requirements:\n{}\n\n",
        research_type_label(brief.research_type),
        brief.countries.join(", "),
        brief.title,
        brief.objective.trim(),
    )
}

pub fn build_request(model: &str, brief: &DraftBrief) -> StructuredRequest {
    let (core, suggestions, _) = preset(brief.research_type);
    let prompt = format!(
        "{}Write {core} core questions and {suggestions} suggestions.\n",
        brief_text(brief)
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

/// "Suggest more": the same brief, asking for suggestions only, with every question the
/// survey already has listed so Gemini doesn't repeat them.
pub fn build_more_request(
    model: &str,
    brief: &DraftBrief,
    existing: &[String],
) -> StructuredRequest {
    let (_, suggestions, _) = preset(brief.research_type);
    let mut req = build_request(model, brief);
    req.prompt = brief_text(brief);
    req.prompt.push_str(&format!(
        "Write 0 core questions and {suggestions} suggestions. The survey already has the questions \
         below. Each suggestion must cover something they don't; do not repeat or reword them.\n"
    ));
    for t in existing {
        req.prompt.push_str(&format!("- {t}\n"));
    }
    req
}

/// Lowercase words, punctuation dropped: "What's your budget?" → ["what", "s", "your", "budget"].
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Same question again: identical words (case and punctuation aside), or at least 90% of
/// the words shared, so a one-word variant counts but "…the battery?" vs "…the camera?" doesn't.
pub fn is_duplicate(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let a: std::collections::HashSet<&String> = a.iter().collect();
    let b: std::collections::HashSet<&String> = b.iter().collect();
    let shared = a.intersection(&b).count() as f64;
    shared / a.union(&b).count() as f64 >= 0.9
}

/// Keeps the new suggestions that duplicate neither an existing question or suggestion nor
/// each other.
pub fn dedupe(new: Vec<NewQuestion>, existing: &[String]) -> Vec<NewQuestion> {
    let mut seen: Vec<String> = existing.to_vec();
    let mut kept = Vec::new();
    for q in new {
        if seen.iter().any(|t| is_duplicate(t, &q.body.text)) {
            continue;
        }
        seen.push(q.body.text.clone());
        kept.push(q);
    }
    kept
}

/// Suggest more (DATA_FLOW §3, Step 3): one Gemini call for new suggestions, saved inactive
/// and `suggested` after removing duplicates of anything already in the survey. Returns the
/// saved suggestions for the critic to check.
pub async fn suggest_more(
    job: &DraftJob,
    llm: &Arc<dyn LlmProvider>,
    limiter: &RateLimiter,
    writer: &Writer,
    existing: Vec<String>,
) -> AppResult<Vec<CriticTarget>> {
    let mut req = build_more_request(&job.model, &job.brief, &existing);
    let log = |attempt: u32, r: &Result<StructuredResponse, LlmError>| {
        log_call(writer, "suggestion", attempt, r)
    };
    let first = with_retries(llm, limiter, &req, log).await?;
    let d = match parse_reply(&first.json, 0) {
        Ok(d) if !d.suggestions.is_empty() => d,
        _ => {
            req.prompt.push_str(
                "\nYour previous reply had no usable suggestions. Follow the format exactly.\n",
            );
            let second = with_retries(llm, limiter, &req, log).await?;
            parse_reply(&second.json, 0)?
        }
    };
    let survey_id = job.survey_id;
    let saved = Arc::new(Mutex::new(Vec::new()));
    let out = saved.clone();
    writer
        .write(Box::new(move |c| {
            // Compare with the survey as it is now, not as it was when the call started.
            let texts = surveys::all_texts(c, survey_id)?;
            let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
            for q in dedupe(d.suggestions, &texts) {
                let id = surveys::insert_ai(c, survey_id, &q, true)?;
                out.push(surveys::start_critique(c, id, critic::PROMPT_VERSION)?);
            }
            Ok(())
        }))
        .await?;
    let targets = std::mem::take(&mut *saved.lock().unwrap_or_else(|e| e.into_inner()));
    Ok(targets)
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
/// or `failed` (people can still write questions by hand after a failure), then has the
/// critic check every drafted question.
pub async fn run(
    job: DraftJob,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
) -> AppResult<()> {
    let survey_id = job.survey_id;
    let outcome = draft(&job, &llm, &limiter, &writer).await;
    let error = outcome.as_ref().err().map(|e| e.to_string());
    let to_check = Arc::new(Mutex::new(Vec::new()));
    let checks = to_check.clone();
    writer
        .write(Box::new(move |c| match outcome {
            Ok(d) => {
                // Redraft replaces only questions nobody has touched.
                surveys::clear_untouched_ai(c, survey_id)?;
                let mut ids = Vec::new();
                for q in &d.questions {
                    ids.push(surveys::insert_ai(c, survey_id, q, false)?);
                }
                for q in &d.suggestions {
                    ids.push(surveys::insert_ai(c, survey_id, q, true)?);
                }
                // Every drafted question, suggestions included, goes to the critic.
                let mut checks = checks.lock().unwrap_or_else(|e| e.into_inner());
                for id in ids {
                    checks.push(surveys::start_critique(c, id, critic::PROMPT_VERSION)?);
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
        .await?;
    // The draft is usable now; flags appear on each question as its check finishes.
    let targets = std::mem::take(&mut *to_check.lock().unwrap_or_else(|e| e.into_inner()));
    critic::run(targets, job.model, llm, limiter, writer).await
}

async fn draft(
    job: &DraftJob,
    llm: &Arc<dyn LlmProvider>,
    limiter: &RateLimiter,
    writer: &Writer,
) -> Result<Draft, LlmError> {
    let (core, _, _) = preset(job.brief.research_type);
    let mut req = build_request(&job.model, &job.brief);
    let log = |attempt: u32, r: &Result<StructuredResponse, LlmError>| {
        log_call(writer, "survey_draft", attempt, r)
    };
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

fn log_call(
    writer: &Writer,
    purpose: &'static str,
    attempt: u32,
    result: &Result<StructuredResponse, LlmError>,
) {
    let (usage, latency, error) = match result {
        Ok(r) => (Some(r.usage), Some(r.latency_ms as i64), None),
        Err(e) => (None, None, Some(e.to_string())),
    };
    let _ = writer.send(Box::new(move |c| {
        c.execute(
            "INSERT INTO llm_calls(purpose, attempt, input_tokens, cached_tokens, output_tokens, latency_ms, error)
             VALUES (?7, ?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                attempt,
                usage.map(|u| u.input_tokens),
                usage.map(|u| u.cached_tokens),
                usage.map(|u| u.output_tokens),
                latency,
                error,
                purpose
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

    fn is_critic(r: &StructuredRequest) -> bool {
        r.prompt.starts_with("Check this survey question")
    }

    /// The critic flags the first core question and passes the rest.
    fn critic_reply(r: &StructuredRequest) -> Value {
        if r.prompt.contains("Q question 1 about") {
            json!({ "flags": [{ "issue": "double_barrelled", "note": "Asks two things." }] })
        } else {
            json!({ "flags": [] })
        }
    }

    #[tokio::test]
    async fn draft_saves_pending_core_questions_and_inactive_suggestions() {
        let llm = ScriptedLlm::new(|r, _| {
            Ok(if is_critic(r) {
                critic_reply(r)
            } else {
                sample_reply(10, 6)
            })
        });
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

        // The critic checked every drafted question, suggestions included, one call each.
        let critic_calls: Vec<_> = llm.requests().into_iter().filter(is_critic).collect();
        assert_eq!(critic_calls.len(), 16);
        for q in s.questions.iter().chain(&s.suggestions) {
            let c = q.critique.as_ref().expect("critique stored");
            assert_eq!(c.status, crate::model::CriticStatus::Done);
            assert_eq!(c.prompt_version, "critic.v1");
            assert_eq!(
                c.flags.len(),
                usize::from(q.body.text.starts_with("Q question 1 "))
            );
        }
        // It sees one question and its options, never the objective or rationale.
        for r in &critic_calls {
            assert!(!r.prompt.contains("Purchase intent") && !r.prompt.contains("Measures intent"));
        }
    }

    #[test]
    fn duplicates_are_the_same_words_not_the_same_topic() {
        assert!(is_duplicate(
            "What's the most you'd pay for a phone?",
            "  what's the MOST you'd pay for a phone "
        ));
        assert!(is_duplicate(
            "Say how satisfied are you overall with the battery life of your current smartphone today?",
            "Please say how satisfied are you overall with the battery life of your current smartphone today?"
        ));
        assert!(!is_duplicate(
            "How satisfied are you with the battery of your phone?",
            "How satisfied are you with the camera of your phone?"
        ));
        assert!(!is_duplicate("", ""));
    }

    #[test]
    fn suggest_more_lists_what_the_survey_has_and_asks_for_suggestions_only() {
        let r = build_more_request("m", &brief(), &["Which brand do you own?".into()]);
        assert!(r.prompt.contains("Market Response Survey"));
        assert!(r
            .prompt
            .contains("Write 0 core questions and 6 suggestions"));
        assert!(r.prompt.contains("- Which brand do you own?\n"));
        assert!(!r.prompt.contains("Write 10 core"));
    }

    #[tokio::test]
    async fn suggest_more_saves_only_new_suggestions_for_the_critic() {
        let llm = ScriptedLlm::new(|r, _| {
            Ok(if is_critic(r) {
                critic_reply(r)
            } else {
                sample_reply(10, 6)
            })
        });
        let (_d, path, id) = run_with(llm).await;
        let conn = crate::db::open(&path).unwrap();
        let before = surveys::get(&conn, id).unwrap();
        let existing = surveys::all_texts(&conn, id).unwrap();

        // Gemini repeats a core question and a suggestion (reworded by case and punctuation),
        // repeats itself, and has two new ideas.
        let mut reply = sample_reply(0, 0);
        let q = |text: &str| {
            let mut v = sample_reply(1, 0)["questions"][0].clone();
            v["text"] = json!(text);
            v
        };
        reply["suggestions"] = json!([
            q("q QUESTION 1 about the category"),
            q("S question 2 about the category?"),
            q("Would a trade-in offer change when you upgrade?"),
            q("Would a trade-in offer change when you upgrade"),
            q("How do you usually pay for a new phone?"),
        ]);
        let more = ScriptedLlm::new(move |_, _| Ok(reply.clone()));
        let (writer, _h) = Writer::spawn(
            crate::db::open(&path).unwrap(),
            50,
            Duration::from_millis(10),
        );
        let limiter = RateLimiter::new(
            Limits {
                requests_per_minute: 1000,
                tokens_per_minute: 10_000_000,
                requests_per_day: 1000,
                max_concurrency: 4,
            },
            0,
        );
        let job = DraftJob {
            survey_id: id,
            brief: brief(),
            model: "pro".into(),
        };
        let llm: Arc<dyn LlmProvider> = Arc::new(more.clone());
        let targets = suggest_more(&job, &llm, &limiter, &writer, existing)
            .await
            .unwrap();
        assert_eq!(targets.len(), 2);
        assert!(more.requests()[0]
            .prompt
            .contains("- Q question 1 about the category?\n"));

        let after = surveys::get(&conn, id).unwrap();
        assert_eq!(after.questions.len(), before.questions.len());
        let added: Vec<&str> = after.suggestions[before.suggestions.len()..]
            .iter()
            .map(|q| q.body.text.as_str())
            .collect();
        assert_eq!(
            added,
            [
                "Would a trade-in offer change when you upgrade?",
                "How do you usually pay for a new phone?"
            ]
        );
        assert!(after
            .suggestions
            .iter()
            .all(|q| !q.is_active && q.review_status == ReviewStatus::Suggested));
        // New suggestions go to the critic, like every drafted question.
        assert!(after.suggestions[before.suggestions.len()..]
            .iter()
            .all(|q| q.critique.as_ref().unwrap().status == crate::model::CriticStatus::Checking));
        writer.write(Box::new(|_| Ok(()))).await.unwrap();
        let logged: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM llm_calls WHERE purpose = 'suggestion'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(logged, 1);
    }

    #[tokio::test]
    async fn a_bad_reply_is_retried_once_then_the_draft_fails_cleanly() {
        let llm = ScriptedLlm::new(|r, i| {
            Ok(if is_critic(r) {
                critic_reply(r)
            } else if i == 0 {
                json!({"title": "", "intro": "", "questions": [], "suggestions": []})
            } else {
                sample_reply(10, 6)
            })
        });
        let (_d, path, id) = run_with(llm.clone()).await;
        let drafts: Vec<_> = llm
            .requests()
            .into_iter()
            .filter(|r| !is_critic(r))
            .collect();
        assert_eq!(drafts.len(), 2);
        assert!(drafts[1].prompt.contains("previous reply was rejected"));
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
