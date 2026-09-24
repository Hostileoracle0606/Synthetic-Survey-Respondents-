//! The report job (docs/DATA_FLOW.md §3, Step 5), run when a simulation completes or is
//! stopped, and again on Regenerate:
//!
//! 1. Theme coding: each open-ended question's answers are grouped into themes by Gemini
//!    (answer text only, never who wrote it).
//! 2. Synthesis: Gemini writes a summary, friction points and segment takeaways from the
//!    report's numbers. Rust then checks every claim: mention counts must equal a coded
//!    theme's count, segments must be real cross-tab groups, and any number must be one the
//!    model was given. Claims that fail are dropped and recorded, never shown.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::params;
use serde_json::{json, Value};

use super::call::with_retries;
use super::limiter::RateLimiter;
use crate::db::writer::Writer;
use crate::error::{AppError, AppResult};
use crate::llm::{LlmError, LlmProvider, StructuredRequest, StructuredResponse};
use crate::model::{
    CrossTab, FrictionPoint, QuestionReport, QuestionType, Report, SegmentTakeaway, SynthesisStatus,
};
use crate::report::{self, LOW_BASE};

pub const THEME_PROMPT_VERSION: &str = "theme.v1";
pub const PROMPT_VERSION: &str = "synthesis.v1";
const THEME_SYSTEM: &str = include_str!("../../prompts/theme.v1.md");
const SYSTEM: &str = include_str!("../../prompts/synthesis.v1.md");

pub struct SynthesisJob {
    pub run_id: i64,
    pub model: String,
    /// Read-only connections are opened here; all writes go through the writer.
    pub db_path: PathBuf,
    /// Code open answers again even if themes exist (Regenerate keeps them).
    pub recode_themes: bool,
}

/// Runs the job and records its outcome on the run (`synthesis_status`).
pub async fn run(
    job: SynthesisJob,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
) -> AppResult<()> {
    let run_id = job.run_id;
    writer
        .write(Box::new(move |c| {
            report::set_synthesis_status(c, run_id, SynthesisStatus::Generating, None)
        }))
        .await?;
    let outcome = inner(&job, &llm, &limiter, &writer).await;
    let (status, error) = match &outcome {
        Ok(()) => (SynthesisStatus::Ready, None),
        Err(e) => (SynthesisStatus::Failed, Some(e.message.clone())),
    };
    writer
        .write(Box::new(move |c| {
            report::set_synthesis_status(c, run_id, status, error.as_deref())
        }))
        .await?;
    outcome
}

async fn inner(
    job: &SynthesisJob,
    llm: &Arc<dyn LlmProvider>,
    limiter: &RateLimiter,
    writer: &Writer,
) -> AppResult<()> {
    let data = report::load(&crate::db::open_reader(&job.db_path)?, job.run_id)?;
    for q in data
        .questions
        .iter()
        .filter(|q| q.body.question_type == QuestionType::OpenEnded)
    {
        if !job.recode_themes && data.themes.contains_key(&q.id) {
            continue;
        }
        let answers: Vec<(i64, String)> = data
            .answers
            .iter()
            .filter(|a| a.question_id == q.id && a.status == "valid")
            .filter_map(|a| a.answer.text.as_ref().map(|t| (a.response_id, t.clone())))
            .collect();
        if answers.is_empty() {
            continue;
        }
        let req = theme_request(&job.model, &q.body.text, &answers);
        let log = |attempt: u32, r: &Result<StructuredResponse, LlmError>| {
            log_call(writer, job.run_id, "theme", attempt, r)
        };
        let themes =
            with_repair(llm, limiter, req, log, |v| parse_themes(v, answers.len())).await?;
        let (run_id, question_id) = (job.run_id, q.id);
        writer
            .write(Box::new(move |c| {
                c.execute(
                    "DELETE FROM themes WHERE run_id = ?1 AND question_id = ?2",
                    params![run_id, question_id],
                )?;
                for t in &themes {
                    c.execute(
                        "INSERT INTO themes(run_id, question_id, label, description) VALUES (?1, ?2, ?3, ?4)",
                        params![run_id, question_id, t.label, t.description],
                    )?;
                    let theme_id = c.last_insert_rowid();
                    for i in &t.answers {
                        c.execute(
                            "INSERT OR IGNORE INTO response_themes(response_id, theme_id) VALUES (?1, ?2)",
                            params![answers[*i - 1].0, theme_id],
                        )?;
                    }
                }
                Ok(())
            }))
            .await?;
    }

    // Numbers and breakdowns now include the themes just coded.
    let (rep, tabs, objective) = {
        let c = crate::db::open_reader(&job.db_path)?;
        let rep = report::report(&c, job.run_id)?;
        let mut tabs = Vec::new();
        for q in &rep.questions {
            if q.question_type == QuestionType::Numeric
                || (q.question_type == QuestionType::OpenEnded && q.themes.is_empty())
            {
                continue;
            }
            for d in &rep.dimensions {
                tabs.push(report::crosstab(&c, job.run_id, q.question_id, &d.key)?);
            }
        }
        let objective: String = c.query_row(
            "SELECT COALESCE(p.research_goal, '') FROM simulation_runs r JOIN projects p ON p.id = r.project_id WHERE r.id = ?1",
            [job.run_id],
            |r| r.get(0),
        )?;
        (rep, tabs, objective)
    };
    if rep.based_on_n == 0 {
        return Err(AppError::invalid("the run has no answers to summarise yet"));
    }
    let prompt = synthesis_prompt(&objective, &rep, &tabs);
    let req = StructuredRequest {
        model: job.model.clone(),
        system: SYSTEM.to_string(),
        prompt,
        schema: synthesis_schema(),
        temperature: 0.4,
        max_output_tokens: 8_192,
    };
    let log = |attempt: u32, r: &Result<StructuredResponse, LlmError>| {
        log_call(writer, job.run_id, "synthesis", attempt, r)
    };
    let reply = with_retries(llm, limiter, &req, log).await?;
    let checked = verify(&reply.json, &rep, &tabs, &numbers_in(&req.prompt));
    let (run_id, model, n) = (job.run_id, job.model.clone(), rep.based_on_n);
    writer
        .write(Box::new(move |c| {
            c.execute(
                "INSERT INTO syntheses(run_id, model, prompt_version, based_on_n, content_json) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![run_id, model, PROMPT_VERSION, n, checked.to_string()],
            )?;
            Ok(())
        }))
        .await
}

/// Calls, parses with `parse`, and retries once with the problem stated if parsing fails.
async fn with_repair<T>(
    llm: &Arc<dyn LlmProvider>,
    limiter: &RateLimiter,
    mut req: StructuredRequest,
    log: impl Fn(u32, &Result<StructuredResponse, LlmError>) + Copy,
    parse: impl Fn(&Value) -> Result<T, String>,
) -> AppResult<T> {
    let first = with_retries(llm, limiter, &req, log).await?;
    match parse(&first.json) {
        Ok(t) => Ok(t),
        Err(problem) => {
            req.prompt.push_str(&format!(
                "\nYour previous reply was rejected: {problem}. Follow the format exactly.\n"
            ));
            let second = with_retries(llm, limiter, &req, log).await?;
            parse(&second.json).map_err(|p| AppError::from(LlmError::SchemaViolation(p)))
        }
    }
}

fn log_call(
    writer: &Writer,
    run_id: i64,
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
            "INSERT INTO llm_calls(run_id, purpose, attempt, input_tokens, cached_tokens, output_tokens, latency_ms, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                run_id,
                purpose,
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

// ---- theme coding ----

pub fn theme_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "themes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "label": { "type": "string" },
                        "description": { "type": "string" },
                        "answers": { "type": "array", "items": { "type": "integer" } }
                    },
                    "required": ["label", "description", "answers"]
                }
            }
        },
        "required": ["themes"]
    })
}

/// Answers are numbered 1..n; the model never sees who wrote them.
pub fn theme_request(model: &str, question: &str, answers: &[(i64, String)]) -> StructuredRequest {
    let mut prompt = format!("Question: {question}\n\nAnswers:\n");
    for (i, (_, text)) in answers.iter().enumerate() {
        prompt.push_str(&format!("{}. {}\n", i + 1, text.replace('\n', " ")));
    }
    StructuredRequest {
        model: model.to_string(),
        system: THEME_SYSTEM.to_string(),
        prompt,
        schema: theme_schema(),
        temperature: 0.2,
        max_output_tokens: 16_384,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodedTheme {
    pub label: String,
    pub description: String,
    /// 1-based answer numbers.
    pub answers: Vec<usize>,
}

pub fn parse_themes(v: &Value, n: usize) -> Result<Vec<CodedTheme>, String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for t in v["themes"].as_array().cloned().unwrap_or_default() {
        let label = t["label"].as_str().unwrap_or_default().trim().to_string();
        if label.is_empty() || !seen.insert(label.to_lowercase()) {
            continue;
        }
        let mut answers: Vec<usize> = t["answers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_u64())
                    .map(|x| x as usize)
                    .filter(|x| (1..=n).contains(x))
                    .collect()
            })
            .unwrap_or_default();
        answers.sort_unstable();
        answers.dedup();
        if answers.is_empty() {
            continue;
        }
        out.push(CodedTheme {
            label,
            description: t["description"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string(),
            answers,
        });
    }
    if out.is_empty() {
        return Err(format!(
            "no usable themes; each needs a label and answer numbers from 1 to {n}"
        ));
    }
    Ok(out)
}

// ---- synthesis ----

pub fn synthesis_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "summary": { "type": "string" },
            "friction_points": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": { "label": { "type": "string" }, "mentions": { "type": "integer" } },
                    "required": ["label", "mentions"]
                }
            },
            "segments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "dimension": { "type": "string" },
                        "group": { "type": "string" },
                        "takeaway": { "type": "string" }
                    },
                    "required": ["dimension", "group", "takeaway"]
                }
            }
        },
        "required": ["summary", "friction_points", "segments"]
    })
}

fn describe_question(q: &QuestionReport) -> String {
    let mut s = format!("[{}] {} (", q.code, q.text);
    s.push_str(match q.question_type {
        QuestionType::SingleChoice => "single choice",
        QuestionType::MultiChoice => "multiple choice, % of respondents choosing each",
        QuestionType::Likert => "scale",
        QuestionType::Numeric => "number",
        QuestionType::OpenEnded => "open answer",
    });
    s.push_str(&format!(", n={}", q.n));
    if let Some(m) = q.mean {
        s.push_str(&format!(", mean {m}"));
    }
    if let (Some(md), Some(a), Some(b)) = (q.median, q.q1, q.q3) {
        s.push_str(&format!(
            ", median {md}, Q1 {a}, Q3 {b}{}",
            q.unit
                .as_deref()
                .map(|u| format!(" {u}"))
                .unwrap_or_default()
        ));
    }
    s.push_str(")\n");
    if q.question_type == QuestionType::OpenEnded {
        for t in &q.themes {
            s.push_str(&format!(
                "- theme \"{}\": {} mentions ({}%)\n",
                t.label, t.count, t.percent
            ));
        }
    } else if q.question_type != QuestionType::Numeric {
        for r in &q.rows {
            s.push_str(&format!("- {}: {} ({}%)\n", r.label, r.count, r.percent));
        }
    }
    s
}

pub fn synthesis_prompt(objective: &str, rep: &Report, tabs: &[CrossTab]) -> String {
    let mut p = format!(
        "Research objective:\n{}\n\nRespondents: {} synthetic respondents answered.\n\nResults\n",
        objective.trim(),
        rep.based_on_n
    );
    for q in &rep.questions {
        p.push_str(&describe_question(q));
    }
    p.push_str("\nGroup breakdowns (% within each group)\n");
    for d in &rep.dimensions {
        p.push_str(&format!("dimension \"{}\" ({}):\n", d.key, d.label));
        for t in tabs.iter().filter(|t| t.dimension.key == d.key) {
            let code = rep
                .questions
                .iter()
                .find(|q| q.question_id == t.question_id)
                .map_or("", |q| q.code.as_str());
            for g in &t.groups {
                let cells: Vec<String> = t
                    .columns
                    .iter()
                    .zip(&g.cells)
                    .map(|(c, v)| format!("{} {}%", c.label, v))
                    .collect();
                p.push_str(&format!(
                    "  [{code}] group \"{}\" (n={}{}): {}{}\n",
                    g.label,
                    g.n,
                    if g.low_base { ", low base" } else { "" },
                    cells.join(", "),
                    g.mean.map(|m| format!("; mean {m}")).unwrap_or_default()
                ));
            }
        }
    }
    p
}

/// Every number in a text: "31.6%" → 31.6, "1,200" → 1200, "18–29" → 18 and 29.
pub fn numbers_in(text: &str) -> Vec<f64> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut s = String::new();
            while i < chars.len() {
                let c = chars[i];
                let next_digit = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
                if c.is_ascii_digit() {
                    s.push(c);
                } else if c == '.' && next_digit {
                    s.push('.');
                } else if c == ','
                    && next_digit
                    && chars.get(i + 2).is_some_and(|n| n.is_ascii_digit())
                    && chars.get(i + 3).is_some_and(|n| n.is_ascii_digit())
                {
                    // thousands separator
                } else {
                    break;
                }
                i += 1;
            }
            if let Ok(v) = s.parse() {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

fn supported(text: &str, allowed: &[f64]) -> bool {
    numbers_in(text)
        .iter()
        .all(|n| allowed.iter().any(|a| (a - n).abs() < 1e-9))
}

/// Keeps only claims that pass the checks; returns the content to store, including the
/// dropped claims and why (kept for the JSON export, never displayed).
pub fn verify(reply: &Value, rep: &Report, tabs: &[CrossTab], allowed: &[f64]) -> Value {
    let mut dropped: Vec<String> = Vec::new();

    let mut summary = Vec::new();
    for sentence in split_sentences(reply["summary"].as_str().unwrap_or_default()) {
        if supported(&sentence, allowed) {
            summary.push(sentence);
        } else {
            dropped.push(format!(
                "summary sentence with an unsupported number: {sentence}"
            ));
        }
    }

    let themes: Vec<(String, u32)> = rep
        .questions
        .iter()
        .flat_map(|q| q.themes.iter().map(|t| (t.label.to_lowercase(), t.count)))
        .collect();
    let mut friction = Vec::new();
    for f in reply["friction_points"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let label = f["label"].as_str().unwrap_or_default().trim().to_string();
        let mentions = f["mentions"].as_u64().unwrap_or(0) as u32;
        let known = themes.iter().find(|(l, _)| *l == label.to_lowercase());
        match known {
            Some((_, count)) if *count == mentions && mentions > 0 => {
                friction.push(FrictionPoint { label, mentions })
            }
            Some((_, count)) => dropped.push(format!(
                "friction point \"{label}\": {mentions} mentions, but the theme has {count}"
            )),
            None => dropped.push(format!("friction point \"{label}\" is not a coded theme")),
        }
    }

    let mut segments = Vec::new();
    for s in reply["segments"].as_array().cloned().unwrap_or_default() {
        let dimension = s["dimension"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let group = s["group"].as_str().unwrap_or_default().trim().to_string();
        let mut takeaway = s["takeaway"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let found = tabs
            .iter()
            .filter(|t| t.dimension.key == dimension)
            .flat_map(|t| t.groups.iter())
            .find(|g| g.label == group);
        match found {
            None => dropped.push(format!(
                "segment {dimension} = \"{group}\" is not a real group"
            )),
            Some(_) if !supported(&takeaway, allowed) => dropped.push(format!(
                "segment {dimension} = \"{group}\" cites an unsupported number: {takeaway}"
            )),
            Some(g) => {
                if g.n < LOW_BASE {
                    takeaway.push_str(&format!(" (low base, n={})", g.n));
                }
                segments.push(SegmentTakeaway {
                    dimension,
                    group,
                    takeaway,
                });
            }
        }
    }

    json!({
        "summary": summary.join(" "),
        "friction_points": friction,
        "segments": segments,
        "dropped": dropped.len(),
        "dropped_claims": dropped,
    })
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        cur.push(*c);
        let end =
            matches!(c, '.' | '!' | '?') && chars.get(i + 1).map_or(true, |n| n.is_whitespace());
        if end {
            out.push(cur.trim().to_string());
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open;
    use crate::engine::limiter::Limits;
    use crate::llm::scripted::ScriptedLlm;
    use crate::report::fixture::Fixture;
    use std::time::Duration;

    #[test]
    fn numbers_are_extracted_like_a_reader_would() {
        assert_eq!(
            numbers_in("31.6% of 1,200 people aged 18–29 said 3."),
            [31.6, 1200.0, 18.0, 29.0, 3.0]
        );
        assert_eq!(numbers_in("no numbers"), Vec::<f64>::new());
        assert_eq!(
            split_sentences("One. Two 3.5 ok! Three"),
            ["One.", "Two 3.5 ok!", "Three"]
        );
    }

    #[test]
    fn theme_parsing_keeps_valid_answer_numbers_only() {
        let v = json!({"themes": [
            {"label": "Price", "description": "d", "answers": [1, 2, 2, 9, 0]},
            {"label": "price", "description": "dup", "answers": [3]},
            {"label": "Empty", "description": "d", "answers": []}
        ]});
        let t = parse_themes(&v, 5).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].answers, [1, 2]);
        assert!(parse_themes(&json!({"themes": []}), 5).is_err());
    }

    struct Env {
        _dir: tempfile::TempDir,
        path: PathBuf,
        writer: Writer,
        run_id: i64,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.db");
        let f = Fixture::build_on(open(&path).unwrap());
        // Start with no themes, so the job codes them.
        f.conn
            .execute_batch("DELETE FROM response_themes; DELETE FROM themes;")
            .unwrap();
        let (writer, _h) = Writer::spawn(open(&path).unwrap(), 50, Duration::from_millis(10));
        Env {
            _dir: dir,
            path,
            writer,
            run_id: f.run_id,
        }
    }

    fn limiter() -> Arc<RateLimiter> {
        Arc::new(RateLimiter::new(
            Limits {
                requests_per_minute: 1000,
                tokens_per_minute: 100_000_000,
                requests_per_day: 1000,
                max_concurrency: 4,
            },
            0,
        ))
    }

    /// Themes: "Price" for answers 1–24, "Battery is fine" for 25–34. Synthesis: one good and
    /// one bad claim of each kind.
    fn scripted() -> ScriptedLlm {
        ScriptedLlm::new(|req, _| {
            if req.system.contains("code open-ended") {
                assert!(
                    !req.prompt.contains("Person "),
                    "theme coding must not see who answered"
                );
                Ok(json!({"themes": [
                    {"label": "Price", "description": "Too expensive", "answers": (1..=24).collect::<Vec<_>>()},
                    {"label": "Battery is fine", "description": "No need yet", "answers": (25..=34).collect::<Vec<_>>()}
                ]}))
            } else {
                Ok(json!({
                    "summary": "Half of respondents own Brand one (50.0%). Price is the main barrier with 24 mentions. Most people would pay 1234 CAD.",
                    "friction_points": [
                        {"label": "Price", "mentions": 24},
                        {"label": "Battery is fine", "mentions": 99},
                        {"label": "Invented theme", "mentions": 5}
                    ],
                    "segments": [
                        {"dimension": "gender", "group": "Female", "takeaway": "Every woman chose Brand one (100.0%)."},
                        {"dimension": "age", "group": "Teenagers", "takeaway": "Made up."},
                        {"dimension": "income", "group": "Under $50k", "takeaway": "About 97.9% prefer Brand two."}
                    ]
                }))
            }
        })
    }

    async fn go(e: &Env, llm: &ScriptedLlm, recode: bool) -> AppResult<()> {
        run(
            SynthesisJob {
                run_id: e.run_id,
                model: "pro".into(),
                db_path: e.path.clone(),
                recode_themes: recode,
            },
            Arc::new(llm.clone()),
            limiter(),
            e.writer.clone(),
        )
        .await
    }

    #[tokio::test]
    async fn synthesis_shows_only_claims_that_pass_the_checks() {
        let e = env();
        let llm = scripted();
        go(&e, &llm, false).await.unwrap();
        let conn = open(&e.path).unwrap();
        let rep = report::report(&conn, e.run_id).unwrap();
        assert_eq!(rep.synthesis_status, SynthesisStatus::Ready);
        let why = rep.questions.iter().find(|q| q.code == "WHY").unwrap();
        assert_eq!(
            why.themes
                .iter()
                .map(|t| (t.label.as_str(), t.count))
                .collect::<Vec<_>>(),
            [("Price", 24), ("Battery is fine", 10)]
        );

        let s = rep.synthesis.unwrap();
        assert_eq!(s.summary, "Half of respondents own Brand one (50.0%). Price is the main barrier with 24 mentions.");
        assert_eq!(
            s.friction_points,
            [FrictionPoint {
                label: "Price".into(),
                mentions: 24
            }]
        );
        assert_eq!(s.segments.len(), 1);
        assert_eq!(s.segments[0].group, "Female");
        assert!(s.segments[0].takeaway.ends_with("(low base, n=19)"));
        assert_eq!(s.dropped, 5);
        assert_eq!(s.based_on_n, 40);
        let stored: String = conn
            .query_row("SELECT content_json FROM syntheses", [], |r| r.get(0))
            .unwrap();
        assert!(stored.contains("1234"), "dropped claims are recorded");
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM llm_calls WHERE purpose IN ('theme','synthesis')",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn regenerate_keeps_themes_unless_asked_to_recode() {
        let e = env();
        let llm = scripted();
        go(&e, &llm, false).await.unwrap();
        go(&e, &llm, false).await.unwrap();
        assert_eq!(llm.calls(), 3, "second run reused the themes");
        go(&e, &llm, true).await.unwrap();
        assert_eq!(llm.calls(), 5);
        let conn = open(&e.path).unwrap();
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM themes", [], |r| r.get(0))
                .unwrap(),
            2
        );
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM syntheses", [], |r| r.get(0))
                .unwrap(),
            3
        );
    }

    #[tokio::test]
    async fn a_failure_is_recorded_and_the_report_still_works() {
        let e = env();
        let llm = ScriptedLlm::new(|_, _| Err(LlmError::Auth("API_KEY_INVALID".into())));
        assert!(go(&e, &llm, false).await.is_err());
        let rep = report::report(&open(&e.path).unwrap(), e.run_id).unwrap();
        assert_eq!(rep.synthesis_status, SynthesisStatus::Failed);
        assert!(rep.synthesis_error.unwrap().contains("authentication"));
        assert!(rep.synthesis.is_none());
        assert_eq!(rep.questions.len(), 5);
    }
}
