//! The critic (docs/DATA_FLOW.md §3, Step 3): a second Gemini call on every drafted, added or
//! edited question flags leading, double-barrelled or unclear wording. It sees one question
//! and its options, never the objective, the cohort or any answers. Its flags are advice for
//! the reviewer: they are stored with the question and never block approval.

use std::sync::Arc;

use rusqlite::params;
use serde_json::{json, Value};
use tokio::task::JoinSet;

use super::call::with_retries;
use super::limiter::RateLimiter;
use crate::db::surveys::{self, CriticTarget};
use crate::db::writer::Writer;
use crate::error::AppResult;
use crate::llm::{LlmError, LlmProvider, StructuredRequest, StructuredResponse};
use crate::model::{CriticFlag, CriticIssue, CriticStatus, Critique, QuestionBody, QuestionType};

pub const PROMPT_VERSION: &str = "critic.v1";
const SYSTEM: &str = include_str!("../../prompts/critic.v1.md");
const MAX_NOTE: usize = 400;

pub fn reply_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "flags": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "issue": { "type": "string", "enum": ["leading", "double_barrelled", "unclear"] },
                        "note": { "type": "string" }
                    },
                    "required": ["issue", "note"]
                }
            }
        },
        "required": ["flags"]
    })
}

/// The question as a respondent would see it: type, text and options, nothing else.
pub fn question_text(b: &QuestionBody) -> String {
    let mut t = String::from("Question type: ");
    match b.question_type {
        QuestionType::SingleChoice => t.push_str("single choice (pick one option)"),
        QuestionType::MultiChoice => t.push_str(&format!(
            "multiple choice (pick up to {})",
            b.max_choices.unwrap_or(b.options.len() as u32)
        )),
        QuestionType::Likert => t.push_str("rating scale"),
        QuestionType::Numeric => t.push_str("number"),
        QuestionType::OpenEnded => t.push_str("open answer in the respondent's own words"),
    }
    t.push_str(&format!("\nQuestion: {}\n", b.text));
    if b.question_type.is_choice() {
        t.push_str("Options:\n");
        for (i, o) in b.options.iter().enumerate() {
            t.push_str(&format!("  {}. {}\n", i + 1, o.label));
        }
        if b.randomize {
            t.push_str("(Options are shown in a random order.)\n");
        }
    }
    if let Some(s) = &b.scale {
        let end = |v: i32, l: &str| {
            if l.is_empty() {
                v.to_string()
            } else {
                format!("{v} = {l}")
            }
        };
        t.push_str(&format!(
            "Scale: {} to {}\n",
            end(s.min, &s.min_label),
            end(s.max, &s.max_label)
        ));
    }
    if let Some(r) = &b.numeric {
        let unit = if r.unit.is_empty() {
            String::new()
        } else {
            format!(" {}", r.unit)
        };
        t.push_str(&format!(
            "Answer: a number from {} to {}{unit}\n",
            r.min, r.max
        ));
    }
    t
}

pub fn build_request(model: &str, body: &QuestionBody) -> StructuredRequest {
    StructuredRequest {
        model: model.to_string(),
        system: SYSTEM.to_string(),
        prompt: format!("Check this survey question.\n\n{}", question_text(body)),
        schema: reply_schema(),
        temperature: 0.2,
        // Gemini 3.x counts its thinking as output.
        max_output_tokens: 8_192,
    }
}

/// Keeps known issues, one flag per kind, with a trimmed note.
pub fn parse_reply(v: &Value) -> Result<Vec<CriticFlag>, LlmError> {
    let items = v["flags"]
        .as_array()
        .ok_or_else(|| LlmError::SchemaViolation("no \"flags\" list".into()))?;
    let mut flags: Vec<CriticFlag> = Vec::new();
    for item in items {
        let issue = match item["issue"].as_str() {
            Some("leading") => CriticIssue::Leading,
            Some("double_barrelled") => CriticIssue::DoubleBarrelled,
            Some("unclear") => CriticIssue::Unclear,
            _ => continue,
        };
        let note: String = item["note"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .chars()
            .take(MAX_NOTE)
            .collect();
        if note.is_empty() || flags.iter().any(|f| f.issue == issue) {
            continue;
        }
        flags.push(CriticFlag { issue, note });
    }
    Ok(flags)
}

/// Checks one question: one call, retried once if the reply can't be read.
pub async fn critique(
    llm: &Arc<dyn LlmProvider>,
    limiter: &RateLimiter,
    writer: &Writer,
    model: &str,
    body: &QuestionBody,
) -> Critique {
    let req = build_request(model, body);
    let log = |attempt: u32, r: &Result<StructuredResponse, LlmError>| log_call(writer, attempt, r);
    let mut outcome = Err(LlmError::SchemaViolation("not asked".into()));
    for _ in 0..2 {
        outcome = match with_retries(llm, limiter, &req, log).await {
            Ok(resp) => parse_reply(&resp.json),
            Err(e) => Err(e),
        };
        if !matches!(outcome, Err(LlmError::SchemaViolation(_))) {
            break;
        }
    }
    match outcome {
        Ok(flags) => Critique {
            status: CriticStatus::Done,
            flags,
            error: None,
            prompt_version: PROMPT_VERSION.into(),
        },
        Err(e) => Critique {
            status: CriticStatus::Failed,
            flags: Vec::new(),
            error: Some(e.to_string()),
            prompt_version: PROMPT_VERSION.into(),
        },
    }
}

/// Checks the questions side by side (the shared rate limiter paces them) and saves each
/// result as it arrives. A question edited in the meantime keeps its newer state.
pub async fn run(
    targets: Vec<CriticTarget>,
    model: String,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
) -> AppResult<()> {
    let mut set = JoinSet::new();
    for t in targets {
        let (llm, limiter, writer, model) =
            (llm.clone(), limiter.clone(), writer.clone(), model.clone());
        set.spawn(async move {
            let c = critique(&llm, &limiter, &writer, &model, &t.body).await;
            writer
                .write(Box::new(move |conn| {
                    surveys::save_critique(conn, &t, &c).map(|_| ())
                }))
                .await
        });
    }
    let mut first_error = Ok(());
    while let Some(done) = set.join_next().await {
        if let Ok(Err(e)) = done {
            if first_error.is_ok() {
                first_error = Err(e);
            }
        }
    }
    first_error
}

fn log_call(writer: &Writer, attempt: u32, result: &Result<StructuredResponse, LlmError>) {
    let (usage, latency, error) = match result {
        Ok(r) => (Some(r.usage), Some(r.latency_ms as i64), None),
        Err(e) => (None, None, Some(e.to_string())),
    };
    let _ = writer.send(Box::new(move |c| {
        c.execute(
            "INSERT INTO llm_calls(purpose, attempt, input_tokens, cached_tokens, output_tokens, latency_ms, error)
             VALUES ('critic', ?1, ?2, ?3, ?4, ?5, ?6)",
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::surveys::NewQuestion;
    use crate::engine::limiter::Limits;
    use crate::llm::scripted::ScriptedLlm;
    use crate::model::{ChoiceOption, Question};
    use std::time::Duration;

    fn body(text: &str) -> QuestionBody {
        QuestionBody {
            text: text.into(),
            question_type: QuestionType::SingleChoice,
            options: ["Yes", "No"]
                .iter()
                .enumerate()
                .map(|(i, l)| ChoiceOption {
                    code: surveys::option_code(i),
                    label: l.to_string(),
                })
                .collect(),
            randomize: false,
            max_choices: None,
            scale: None,
            numeric: None,
        }
    }

    #[test]
    fn the_request_holds_one_question_and_its_options_only() {
        let r = build_request("pro", &body("Is our amazing phone fast and cheap?"));
        assert!(r
            .prompt
            .contains("Question: Is our amazing phone fast and cheap?"));
        assert!(r.prompt.contains("  1. Yes\n  2. No\n"));
        assert!(r.system.contains("double_barrelled"));
        let schema = r.schema.to_string();
        assert!(schema.contains("leading") && schema.contains("unclear"));
    }

    #[test]
    fn parse_keeps_one_flag_per_known_issue() {
        let flags = parse_reply(&json!({ "flags": [
            { "issue": "leading", "note": " Says 'amazing'. " },
            { "issue": "leading", "note": "Again." },
            { "issue": "loaded", "note": "Not a kind we show." },
            { "issue": "unclear", "note": "" },
            { "issue": "double_barrelled", "note": "Fast and cheap are two questions." }
        ]}))
        .unwrap();
        assert_eq!(
            flags,
            vec![
                CriticFlag {
                    issue: CriticIssue::Leading,
                    note: "Says 'amazing'.".into()
                },
                CriticFlag {
                    issue: CriticIssue::DoubleBarrelled,
                    note: "Fast and cheap are two questions.".into()
                },
            ]
        );
        assert!(parse_reply(&json!({ "flags": [] })).unwrap().is_empty());
        assert!(parse_reply(&json!({ "verdict": "fine" })).is_err());
    }

    struct Env {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        writer: Writer,
        limiter: Arc<RateLimiter>,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.db");
        let conn = crate::db::open(&path).unwrap();
        conn.execute("INSERT INTO projects(title) VALUES ('p')", [])
            .unwrap();
        surveys::for_project(&conn, 1).unwrap();
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
        Env {
            _dir: dir,
            path,
            writer,
            limiter,
        }
    }

    fn add(env: &Env, text: &str) -> CriticTarget {
        let conn = crate::db::open(&env.path).unwrap();
        let id = surveys::insert_ai(
            &conn,
            1,
            &NewQuestion {
                code: String::new(),
                body: body(text),
                objective: Some("SECRET OBJECTIVE".into()),
                rationale: Some("SECRET RATIONALE".into()),
            },
            false,
        )
        .unwrap();
        surveys::start_critique(&conn, id, PROMPT_VERSION).unwrap()
    }

    fn stored(env: &Env, id: i64) -> Question {
        surveys::question(&crate::db::open(&env.path).unwrap(), id).unwrap()
    }

    #[tokio::test]
    async fn flags_are_stored_on_each_question_and_calls_are_logged() {
        let env = env();
        let leading = add(&env, "Don't you love how fast our phone is?");
        let fine = add(&env, "How old is your current phone?");
        let llm = ScriptedLlm::new(|r, _| {
            Ok(if r.prompt.contains("love") {
                json!({ "flags": [{ "issue": "leading", "note": "Presumes the respondent loves it." }] })
            } else {
                json!({ "flags": [] })
            })
        });
        run(
            vec![leading.clone(), fine.clone()],
            "pro".into(),
            Arc::new(llm.clone()),
            env.limiter.clone(),
            env.writer.clone(),
        )
        .await
        .unwrap();
        let c = stored(&env, leading.question_id).critique.unwrap();
        assert_eq!(c.status, CriticStatus::Done);
        assert_eq!(c.flags[0].issue, CriticIssue::Leading);
        let c = stored(&env, fine.question_id).critique.unwrap();
        assert_eq!((c.status, c.flags.len()), (CriticStatus::Done, 0));
        // It never sees the objective or rationale.
        assert!(llm.requests().iter().all(|r| !r.prompt.contains("SECRET")));
        env.writer.write(Box::new(|_| Ok(()))).await.unwrap();
        let logged: i64 = crate::db::open(&env.path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM llm_calls WHERE purpose = 'critic'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(logged, 2);
    }

    #[tokio::test]
    async fn an_unreadable_reply_is_retried_once_then_the_check_is_marked_failed() {
        let env = env();
        let t = add(&env, "How old is your current phone?");
        let llm = ScriptedLlm::new(|_, i| {
            Ok(if i == 0 {
                json!({ "verdict": "ok" })
            } else {
                json!({ "flags": [] })
            })
        });
        run(
            vec![t.clone()],
            "pro".into(),
            Arc::new(llm.clone()),
            env.limiter.clone(),
            env.writer.clone(),
        )
        .await
        .unwrap();
        assert_eq!(llm.calls(), 2);
        assert_eq!(
            stored(&env, t.question_id).critique.unwrap().status,
            CriticStatus::Done
        );

        let t = add(&env, "How many phones have you owned?");
        let bad = ScriptedLlm::new(|_, _| Err(LlmError::Auth("bad key".into())));
        run(
            vec![t.clone()],
            "pro".into(),
            Arc::new(bad),
            env.limiter.clone(),
            env.writer.clone(),
        )
        .await
        .unwrap();
        let c = stored(&env, t.question_id).critique.unwrap();
        assert_eq!(c.status, CriticStatus::Failed);
        assert!(c.error.unwrap().contains("bad key"));
        // Advice only: a failed or flagged check doesn't stop approval.
        let conn = crate::db::open(&env.path).unwrap();
        assert!(surveys::approve_question(&conn, t.question_id).is_ok());
    }

    #[tokio::test]
    async fn a_check_of_old_wording_does_not_overwrite_an_edit() {
        let env = env();
        let t = add(&env, "Don't you love our phone?");
        let conn = crate::db::open(&env.path).unwrap();
        surveys::update_question(&conn, t.question_id, body("How do you rate our phone?")).unwrap();
        let llm = ScriptedLlm::new(|_, _| {
            Ok(json!({ "flags": [{ "issue": "leading", "note": "Loaded." }] }))
        });
        run(
            vec![t.clone()],
            "pro".into(),
            Arc::new(llm),
            env.limiter.clone(),
            env.writer.clone(),
        )
        .await
        .unwrap();
        assert!(stored(&env, t.question_id).critique.is_none());
    }
}
