//! Step 3 persistence: the survey and its questions (docs/DATA_FLOW.md §3, Step 3).
//!
//! Every active question must be accepted by a person before a run can start; the
//! `trg_runs_require_approved_survey` trigger enforces that in the database.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::model::{
    CriticStatus, Critique, DraftStatus, NumericRange, Question, QuestionBody, QuestionOrigin,
    QuestionType, ReviewStatus, Scale, Survey, SurveyStatus,
};

pub const MAX_OPTIONS: usize = 15;
pub const MAX_TEXT: usize = 1_000;

/// Checks a question body and fills in missing option codes ("A", "B", …).
pub fn normalise(mut b: QuestionBody) -> AppResult<QuestionBody> {
    b.text = b.text.trim().to_string();
    if b.text.is_empty() {
        return Err(AppError::invalid("the question has no text"));
    }
    if b.text.chars().count() > MAX_TEXT {
        return Err(AppError::invalid(format!(
            "question text is over {MAX_TEXT} characters"
        )));
    }
    if b.question_type.is_choice() {
        for o in &mut b.options {
            o.label = o.label.trim().to_string();
        }
        b.options.retain(|o| !o.label.is_empty());
        if b.options.len() < 2 || b.options.len() > MAX_OPTIONS {
            return Err(AppError::invalid(format!(
                "a choice question needs 2 to {MAX_OPTIONS} options"
            )));
        }
        let mut labels: Vec<String> = b.options.iter().map(|o| o.label.to_lowercase()).collect();
        labels.sort();
        labels.dedup();
        if labels.len() != b.options.len() {
            return Err(AppError::invalid("two options have the same text"));
        }
        let codes_ok = {
            let mut c: Vec<&str> = b.options.iter().map(|o| o.code.as_str()).collect();
            c.sort();
            c.dedup();
            c.len() == b.options.len() && b.options.iter().all(|o| !o.code.trim().is_empty())
        };
        if !codes_ok {
            for (i, o) in b.options.iter_mut().enumerate() {
                o.code = option_code(i);
            }
        }
        if b.question_type == QuestionType::MultiChoice {
            let n = b.options.len() as u32;
            b.max_choices = Some(b.max_choices.unwrap_or(n).clamp(1, n));
        } else {
            b.max_choices = None;
        }
    } else {
        b.options.clear();
        b.randomize = false;
        b.max_choices = None;
    }
    if b.question_type == QuestionType::Likert {
        let s = b.scale.get_or_insert(Scale {
            min: 1,
            max: 5,
            min_label: String::new(),
            max_label: String::new(),
        });
        if s.min < 0 || s.max <= s.min || s.max - s.min > 10 {
            return Err(AppError::invalid(
                "a scale needs 2 to 11 points, starting at 0 or more",
            ));
        }
    } else {
        b.scale = None;
    }
    if b.question_type == QuestionType::Numeric {
        let r = b.numeric.get_or_insert(NumericRange {
            min: 0.0,
            max: 100.0,
            unit: String::new(),
        });
        if !r.min.is_finite() || !r.max.is_finite() || r.max <= r.min {
            return Err(AppError::invalid("the number range is empty"));
        }
    } else {
        b.numeric = None;
    }
    Ok(b)
}

/// "A".."Z", then "AA", "AB", …
pub fn option_code(i: usize) -> String {
    let letters = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    if i < 26 {
        (letters[i] as char).to_string()
    } else {
        format!("{}{}", letters[i / 26 - 1] as char, letters[i % 26] as char)
    }
}

/// Stored shape, as documented in schema.sql:
/// choice `{"options":[…],"randomize":true,"max":3}`, likert `{"min","max","labels"}`,
/// numeric `{"min","max","unit"}`.
fn options_json(b: &QuestionBody) -> Option<String> {
    let v = match b.question_type {
        QuestionType::SingleChoice | QuestionType::MultiChoice => json!({
            "options": b.options, "randomize": b.randomize, "max": b.max_choices
        }),
        QuestionType::Likert => {
            let s = b.scale.as_ref()?;
            json!({ "min": s.min, "max": s.max, "labels": {
                s.min.to_string(): s.min_label, s.max.to_string(): s.max_label } })
        }
        QuestionType::Numeric => {
            let r = b.numeric.as_ref()?;
            json!({ "min": r.min, "max": r.max, "unit": r.unit })
        }
        QuestionType::OpenEnded => return None,
    };
    Some(v.to_string())
}

fn body_from(text: String, qtype: &str, options: Option<String>) -> QuestionBody {
    let question_type = QuestionType::from_db(qtype).unwrap_or(QuestionType::OpenEnded);
    let o: Value = options
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let mut b = QuestionBody {
        text,
        question_type,
        options: Vec::new(),
        randomize: false,
        max_choices: None,
        scale: None,
        numeric: None,
    };
    match question_type {
        QuestionType::SingleChoice | QuestionType::MultiChoice => {
            b.options = serde_json::from_value(o["options"].clone()).unwrap_or_default();
            b.randomize = o["randomize"].as_bool().unwrap_or(false);
            b.max_choices = o["max"].as_u64().map(|m| m as u32);
        }
        QuestionType::Likert => {
            let min = o["min"].as_i64().unwrap_or(1) as i32;
            let max = o["max"].as_i64().unwrap_or(5) as i32;
            let label = |k: i32| {
                o["labels"][k.to_string()]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            };
            b.scale = Some(Scale {
                min,
                max,
                min_label: label(min),
                max_label: label(max),
            });
        }
        QuestionType::Numeric => {
            b.numeric = Some(NumericRange {
                min: o["min"].as_f64().unwrap_or(0.0),
                max: o["max"].as_f64().unwrap_or(100.0),
                unit: o["unit"].as_str().unwrap_or_default().to_string(),
            });
        }
        QuestionType::OpenEnded => {}
    }
    b
}

const QUESTION_COLS: &str = "id, code, order_index, question_text, question_type, options_json, is_active, origin, review_status, objective, rationale, critic_json";

fn question_from_row(r: &Row) -> rusqlite::Result<Question> {
    let origin: String = r.get(7)?;
    let review: String = r.get(8)?;
    Ok(Question {
        id: r.get(0)?,
        code: r.get(1)?,
        order_index: r.get(2)?,
        body: body_from(r.get(3)?, &r.get::<_, String>(4)?, r.get(5)?),
        is_active: r.get::<_, i64>(6)? == 1,
        origin: match origin.as_str() {
            "ai" => QuestionOrigin::Ai,
            "ai_edited" => QuestionOrigin::AiEdited,
            _ => QuestionOrigin::Human,
        },
        review_status: match review.as_str() {
            "suggested" => ReviewStatus::Suggested,
            "accepted" => ReviewStatus::Accepted,
            "rejected" => ReviewStatus::Rejected,
            _ => ReviewStatus::Pending,
        },
        objective: r.get(9)?,
        rationale: r.get(10)?,
        critique: r
            .get::<_, Option<String>>(11)?
            .and_then(|j| serde_json::from_str(&j).ok()),
    })
}

pub fn question(conn: &Connection, id: i64) -> AppResult<Question> {
    conn.query_row(
        &format!("SELECT {QUESTION_COLS} FROM questions WHERE id = ?1"),
        [id],
        question_from_row,
    )
    .optional()?
    .ok_or_else(|| AppError::not_found(format!("question {id} not found")))
}

fn survey_of_question(conn: &Connection, id: i64) -> AppResult<i64> {
    conn.query_row("SELECT survey_id FROM questions WHERE id = ?1", [id], |r| {
        r.get(0)
    })
    .optional()?
    .ok_or_else(|| AppError::not_found(format!("question {id} not found")))
}

/// The project's survey, created empty on first use.
pub fn for_project(conn: &Connection, project_id: i64) -> AppResult<Survey> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM surveys WHERE project_id = ?1 ORDER BY id DESC LIMIT 1",
            [project_id],
            |r| r.get(0),
        )
        .optional()?;
    let id = match id {
        Some(id) => id,
        None => {
            let title: String = conn
                .query_row(
                    "SELECT title FROM projects WHERE id = ?1",
                    [project_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| AppError::not_found(format!("project {project_id} not found")))?;
            conn.execute(
                "INSERT INTO surveys(project_id, title) VALUES (?1, ?2)",
                params![project_id, title],
            )?;
            conn.last_insert_rowid()
        }
    };
    get(conn, id)
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Survey> {
    let (project_id, title, intro, status, draft_status, draft_error): (
        i64,
        String,
        Option<String>,
        String,
        String,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT project_id, title, intro_text, status, draft_status, draft_error FROM surveys WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("survey {id} not found")))?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {QUESTION_COLS} FROM questions WHERE survey_id = ?1 ORDER BY order_index"
    ))?;
    let all = stmt
        .query_map([id], question_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    let (questions, rest): (Vec<_>, Vec<_>) = all.into_iter().partition(|q| q.is_active);
    let suggestions = rest
        .into_iter()
        .filter(|q| q.review_status == ReviewStatus::Suggested)
        .collect();
    Ok(Survey {
        id,
        project_id,
        title,
        intro: intro.unwrap_or_default(),
        status: match status.as_str() {
            "approved" => SurveyStatus::Approved,
            "in_review" => SurveyStatus::InReview,
            _ => SurveyStatus::Draft,
        },
        draft_status: match draft_status.as_str() {
            "generating" => DraftStatus::Generating,
            "ready" => DraftStatus::Ready,
            "failed" => DraftStatus::Failed,
            _ => DraftStatus::None,
        },
        draft_error,
        questions,
        suggestions,
    })
}

fn next_order(conn: &Connection, survey_id: i64) -> AppResult<u32> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(order_index), 0) + 1 FROM questions WHERE survey_id = ?1",
        [survey_id],
        |r| r.get(0),
    )?)
}

/// A code unique in the survey, from `wanted` ("Q3_PRICE") or "Q<n>".
fn unique_code(conn: &Connection, survey_id: i64, wanted: &str) -> AppResult<String> {
    let base: String = wanted
        .to_uppercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(24)
        .collect();
    let base = if base.trim_matches('_').is_empty() {
        format!("Q{}", next_order(conn, survey_id)?)
    } else {
        base
    };
    let taken = |c: &str| -> AppResult<bool> {
        Ok(conn
            .query_row(
                "SELECT 1 FROM questions WHERE survey_id = ?1 AND code = ?2",
                params![survey_id, c],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    };
    if !taken(&base)? {
        return Ok(base);
    }
    for n in 2.. {
        let c = format!("{base}_{n}");
        if !taken(&c)? {
            return Ok(c);
        }
    }
    unreachable!()
}

/// The text of every question the survey has, active, suggested or retired; "Suggest more"
/// shows it to Gemini and removes duplicates of it.
pub fn all_texts(conn: &Connection, survey_id: i64) -> AppResult<Vec<String>> {
    Ok(conn
        .prepare("SELECT question_text FROM questions WHERE survey_id = ?1 ORDER BY order_index")?
        .query_map([survey_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?)
}

/// A drafted question, as saved by the draft job.
pub struct NewQuestion {
    pub code: String,
    pub body: QuestionBody,
    pub objective: Option<String>,
    pub rationale: Option<String>,
}

/// Saves AI questions: core ones active and `pending`, suggestions inactive and `suggested`.
pub fn insert_ai(
    conn: &Connection,
    survey_id: i64,
    q: &NewQuestion,
    suggestion: bool,
) -> AppResult<i64> {
    let body = normalise(q.body.clone())?;
    let code = unique_code(conn, survey_id, &q.code)?;
    let original = json!({ "body": body, "objective": q.objective, "rationale": q.rationale });
    conn.execute(
        "INSERT INTO questions(survey_id, order_index, code, question_text, question_type, options_json,
            is_active, origin, review_status, objective, rationale, original_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'ai', ?8, ?9, ?10, ?11)",
        params![
            survey_id,
            next_order(conn, survey_id)?,
            code,
            body.text,
            body.question_type.as_db(),
            options_json(&body),
            !suggestion,
            if suggestion { "suggested" } else { "pending" },
            q.objective,
            q.rationale,
            original.to_string()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// `+ New`: a blank single-choice question written by a person.
pub fn add_question(conn: &Connection, survey_id: i64) -> AppResult<Question> {
    let code = unique_code(conn, survey_id, "")?;
    conn.execute(
        "INSERT INTO questions(survey_id, order_index, code, question_text, question_type, options_json, origin, review_status)
         VALUES (?1, ?2, ?3, 'New question', 'single_choice', ?4, 'human', 'pending')",
        params![
            survey_id,
            next_order(conn, survey_id)?,
            code,
            json!({ "options": [
                { "code": "A", "label": "Option 1" },
                { "code": "B", "label": "Option 2" }
            ], "randomize": true, "max": null })
            .to_string()
        ],
    )?;
    reopen(conn, survey_id)?;
    question(conn, conn.last_insert_rowid())
}

/// Edit type, text or options. AI questions become `ai_edited` (the draft stays in
/// `original_json`), and any edit needs approving again.
pub fn update_question(conn: &Connection, id: i64, body: QuestionBody) -> AppResult<Question> {
    let body = normalise(body)?;
    let survey_id = survey_of_question(conn, id)?;
    // A run's report reads its questions from here; editing one it answered would rewrite it.
    if answered(conn, id)? {
        return Err(AppError::invalid(
            "a simulation run already has answers to this question; delete it (the run keeps it for its report) and add a new one",
        ));
    }
    conn.execute(
        "UPDATE questions SET question_text = ?2, question_type = ?3, options_json = ?4,
            origin = CASE origin WHEN 'ai' THEN 'ai_edited' ELSE origin END,
            review_status = CASE review_status WHEN 'suggested' THEN 'suggested' ELSE 'pending' END,
            reviewed_at = NULL, critic_json = NULL
         WHERE id = ?1",
        params![
            id,
            body.text,
            body.question_type.as_db(),
            options_json(&body)
        ],
    )?;
    reopen(conn, survey_id)?;
    question(conn, id)
}

/// The wording a critique judged: text, type and options exactly as stored. A critique is
/// saved only if the question still reads the same, so a slow check can't overwrite a newer one.
const WORDING: &str =
    "question_text || char(31) || question_type || char(31) || COALESCE(options_json, '')";

/// A question queued for the critic.
#[derive(Debug, Clone, PartialEq)]
pub struct CriticTarget {
    pub question_id: i64,
    pub body: QuestionBody,
    pub wording: String,
}

/// Marks the question as being checked and returns what the critic should judge.
pub fn start_critique(conn: &Connection, id: i64, prompt_version: &str) -> AppResult<CriticTarget> {
    let q = question(conn, id)?;
    let checking = Critique {
        status: CriticStatus::Checking,
        flags: Vec::new(),
        error: None,
        prompt_version: prompt_version.to_string(),
    };
    conn.execute(
        "UPDATE questions SET critic_json = ?2 WHERE id = ?1",
        params![id, serde_json::to_string(&checking).unwrap_or_default()],
    )?;
    let wording = conn.query_row(
        &format!("SELECT {WORDING} FROM questions WHERE id = ?1"),
        [id],
        |r| r.get(0),
    )?;
    Ok(CriticTarget {
        question_id: id,
        body: q.body,
        wording,
    })
}

/// Stores the critic's result. Returns false (and stores nothing) when the question has been
/// edited or deleted since the check started.
pub fn save_critique(conn: &Connection, target: &CriticTarget, c: &Critique) -> AppResult<bool> {
    Ok(conn.execute(
        &format!("UPDATE questions SET critic_json = ?2 WHERE id = ?1 AND {WORDING} = ?3"),
        params![
            target.question_id,
            serde_json::to_string(c).unwrap_or_default(),
            target.wording
        ],
    )? == 1)
}

/// Rewrites `order_index` for the active questions in the given order. Runs in one
/// transaction; indexes go negative first so the UNIQUE constraint holds throughout.
pub fn reorder(conn: &Connection, survey_id: i64, ordered_ids: &[i64]) -> AppResult<Survey> {
    let current = get(conn, survey_id)?;
    let mut want: Vec<i64> = ordered_ids.to_vec();
    let mut have: Vec<i64> = current.questions.iter().map(|q| q.id).collect();
    want.sort_unstable();
    have.sort_unstable();
    if want != have {
        return Err(AppError::invalid(
            "the new order must list every question in the survey once",
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE questions SET order_index = -order_index - 1 WHERE survey_id = ?1",
        [survey_id],
    )?;
    for (i, id) in ordered_ids.iter().enumerate() {
        tx.execute(
            "UPDATE questions SET order_index = ?2 WHERE id = ?1",
            params![id, i as i64 + 1],
        )?;
    }
    // Inactive rows (suggestions, retired questions) go after the survey, keeping their order.
    let rest: Vec<i64> = tx
        .prepare(
            "SELECT id FROM questions WHERE survey_id = ?1 AND order_index < 0 ORDER BY order_index DESC",
        )?
        .query_map([survey_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for (i, id) in rest.iter().enumerate() {
        tx.execute(
            "UPDATE questions SET order_index = ?2 WHERE id = ?1",
            params![id, (ordered_ids.len() + i) as i64 + 1],
        )?;
    }
    tx.commit()?;
    get(conn, survey_id)
}

fn answered(conn: &Connection, question_id: i64) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM responses WHERE question_id = ?1 LIMIT 1",
            [question_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Deletes a question, or retires it (inactive) if a run already has answers to it.
pub fn delete_question(conn: &Connection, id: i64) -> AppResult<Survey> {
    let survey_id = survey_of_question(conn, id)?;
    if answered(conn, id)? {
        conn.execute(
            "UPDATE questions SET is_active = 0, review_status = 'rejected' WHERE id = ?1",
            [id],
        )?;
    } else {
        conn.execute("DELETE FROM questions WHERE id = ?1", [id])?;
    }
    reopen(conn, survey_id)?;
    get(conn, survey_id)
}

pub fn approve_question(conn: &Connection, id: i64) -> AppResult<Question> {
    let q = question(conn, id)?;
    if !q.is_active {
        return Err(AppError::invalid("add the suggestion to the survey first"));
    }
    conn.execute(
        "UPDATE questions SET review_status = 'accepted', reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        [id],
    )?;
    question(conn, id)
}

/// `+ Add to survey`: a suggestion joins the end of the survey, still needing approval.
pub fn add_suggestion(conn: &Connection, id: i64) -> AppResult<Survey> {
    let q = question(conn, id)?;
    if q.review_status != ReviewStatus::Suggested {
        return Err(AppError::invalid("this question is not a suggestion"));
    }
    let survey_id = survey_of_question(conn, id)?;
    let tx = conn.unchecked_transaction()?;
    let last: u32 = tx.query_row(
        "SELECT COALESCE(MAX(order_index), 0) FROM questions WHERE survey_id = ?1 AND is_active = 1",
        [survey_id],
        |r| r.get(0),
    )?;
    // Make room right after the last active question.
    tx.execute(
        "UPDATE questions SET order_index = -order_index - 1 WHERE survey_id = ?1 AND order_index > ?2 AND id <> ?3",
        params![survey_id, last, id],
    )?;
    tx.execute(
        "UPDATE questions SET is_active = 1, review_status = 'pending', order_index = ?2 WHERE id = ?1",
        params![id, last + 1],
    )?;
    tx.execute(
        "UPDATE questions SET order_index = -order_index + 1 WHERE survey_id = ?1 AND order_index < 0",
        [survey_id],
    )?;
    tx.commit()?;
    reopen(conn, survey_id)?;
    get(conn, survey_id)
}

/// Any change to the questions sends an approved survey back to review.
fn reopen(conn: &Connection, survey_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE surveys SET status = 'in_review', approved_at = NULL WHERE id = ?1 AND status = 'approved'",
        [survey_id],
    )?;
    Ok(())
}

pub fn set_draft_status(
    conn: &Connection,
    survey_id: i64,
    status: DraftStatus,
    error: Option<&str>,
) -> AppResult<()> {
    let s = match status {
        DraftStatus::None => "none",
        DraftStatus::Generating => "generating",
        DraftStatus::Ready => "ready",
        DraftStatus::Failed => "failed",
    };
    conn.execute(
        "UPDATE surveys SET draft_status = ?2, draft_error = ?3,
            status = CASE WHEN ?2 = 'ready' AND status = 'draft' THEN 'in_review' ELSE status END
         WHERE id = ?1",
        params![survey_id, s, error],
    )?;
    Ok(())
}

/// Redraft: removes AI questions nobody has touched (pending drafts and open suggestions),
/// keeping anything a person wrote, edited or approved.
pub fn clear_untouched_ai(conn: &Connection, survey_id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM questions WHERE survey_id = ?1 AND origin = 'ai' AND review_status IN ('pending','suggested')
            AND NOT EXISTS (SELECT 1 FROM responses WHERE question_id = questions.id)",
        [survey_id],
    )?;
    Ok(())
}

/// The draft's intro, unless a person has already edited the survey's title or intro.
pub fn set_drafted_intro(conn: &Connection, survey_id: i64, intro: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE surveys SET intro_text = ?2 WHERE id = ?1 AND text_edited = 0",
        params![survey_id, intro],
    )?;
    Ok(())
}

pub const MAX_TITLE: usize = 200;
pub const MAX_INTRO: usize = 2_000;

/// Step 3: the reviewer edits the survey title and the intro respondents see before Q1.
/// Respondents see the intro, so changing it sends an approved survey back to review, and
/// it can't change under a run that isn't finished (resuming would mix two intros).
pub fn update_text(
    conn: &Connection,
    survey_id: i64,
    title: &str,
    intro: &str,
) -> AppResult<Survey> {
    let (title, intro) = (title.trim(), intro.trim());
    if title.is_empty() {
        return Err(AppError::invalid("the survey needs a title"));
    }
    if title.chars().count() > MAX_TITLE {
        return Err(AppError::invalid(format!(
            "the title is over {MAX_TITLE} characters"
        )));
    }
    if intro.chars().count() > MAX_INTRO {
        return Err(AppError::invalid(format!(
            "the intro is over {MAX_INTRO} characters"
        )));
    }
    let current = get(conn, survey_id)?;
    if current.title == title && current.intro == intro {
        return Ok(current);
    }
    if current.intro != intro {
        let unfinished = conn
            .query_row(
                "SELECT 1 FROM simulation_runs WHERE survey_id = ?1 AND status IN ('queued','running','paused')",
                [survey_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if unfinished {
            return Err(AppError::invalid(
                "a simulation run on this survey isn't finished; stop it before changing the intro respondents see",
            ));
        }
        reopen(conn, survey_id)?;
    }
    conn.execute(
        "UPDATE surveys SET title = ?2, intro_text = ?3, text_edited = 1 WHERE id = ?1",
        params![survey_id, title, intro],
    )?;
    get(conn, survey_id)
}

pub fn set_generation(
    conn: &Connection,
    survey_id: i64,
    model: &str,
    prompt_version: &str,
    brief: &Value,
) -> AppResult<()> {
    conn.execute(
        "UPDATE surveys SET generation_model = ?2, generation_prompt_version = ?3, brief_json = ?4 WHERE id = ?1",
        params![survey_id, model, prompt_version, brief.to_string()],
    )?;
    Ok(())
}

/// Stable hash of the active questions (FNV-1a over their JSON), stored on each run.
pub fn survey_hash(survey: &Survey) -> String {
    let text = serde_json::to_string(
        &survey
            .questions
            .iter()
            .map(|q| (&q.code, &q.body))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChoiceOption;

    fn choice(text: &str, labels: &[&str]) -> QuestionBody {
        QuestionBody {
            text: text.into(),
            question_type: QuestionType::SingleChoice,
            options: labels
                .iter()
                .map(|l| ChoiceOption {
                    code: String::new(),
                    label: l.to_string(),
                })
                .collect(),
            randomize: true,
            max_choices: None,
            scale: None,
            numeric: None,
        }
    }

    fn setup() -> (Connection, i64) {
        let conn = crate::db::open_in_memory();
        conn.execute("INSERT INTO projects(title) VALUES ('p')", [])
            .unwrap();
        let s = for_project(&conn, 1).unwrap();
        (conn, s.id)
    }

    fn ai(conn: &Connection, survey: i64, code: &str, suggestion: bool) -> i64 {
        insert_ai(
            conn,
            survey,
            &NewQuestion {
                code: code.into(),
                body: choice(&format!("Question {code}?"), &["Yes", "No"]),
                objective: Some("obj".into()),
                rationale: None,
            },
            suggestion,
        )
        .unwrap()
    }

    #[test]
    fn normalise_fills_codes_and_rejects_bad_questions() {
        let b = normalise(choice(" Which? ", &["One", " Two ", ""])).unwrap();
        assert_eq!(b.text, "Which?");
        assert_eq!(
            b.options
                .iter()
                .map(|o| o.code.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert!(normalise(choice("Q", &["Only"])).is_err());
        assert!(normalise(choice("Q", &["Same", "same"])).is_err());
        assert!(normalise(choice("", &["a", "b"])).is_err());
        let mut likert = choice("Agree?", &[]);
        likert.question_type = QuestionType::Likert;
        let l = normalise(likert.clone()).unwrap();
        assert_eq!(l.scale.as_ref().unwrap().max, 5);
        assert!(l.options.is_empty());
        likert.scale = Some(Scale {
            min: 1,
            max: 20,
            min_label: String::new(),
            max_label: String::new(),
        });
        assert!(normalise(likert).is_err());
        assert_eq!(option_code(27), "AB");
    }

    #[test]
    fn bodies_round_trip_through_the_database() {
        let (conn, s) = setup();
        let id = ai(&conn, s, "q1 price", false);
        let q = question(&conn, id).unwrap();
        assert_eq!(q.code, "Q1_PRICE");
        assert_eq!(q.body.options.len(), 2);
        assert!(q.body.randomize);
        let mut b = q.body.clone();
        b.question_type = QuestionType::Likert;
        b.scale = Some(Scale {
            min: 1,
            max: 7,
            min_label: "Strongly disagree".into(),
            max_label: "Strongly agree".into(),
        });
        let q2 = update_question(&conn, id, b).unwrap();
        assert_eq!(q2.body.scale.unwrap().max_label, "Strongly agree");
        assert_eq!(q2.origin, QuestionOrigin::AiEdited);
        // The AI draft is kept.
        let original: String = conn
            .query_row(
                "SELECT original_json FROM questions WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(original.contains("single_choice"));
    }

    #[test]
    fn codes_are_unique_per_survey() {
        let (conn, s) = setup();
        ai(&conn, s, "Q_A", false);
        let id = ai(&conn, s, "q a", false);
        assert_eq!(question(&conn, id).unwrap().code, "Q_A_2");
    }

    #[test]
    fn suggestions_stay_out_until_added_then_need_approval() {
        let (conn, s) = setup();
        let a = ai(&conn, s, "A", false);
        let sug = ai(&conn, s, "S", true);
        let b = ai(&conn, s, "B", false);
        let survey = get(&conn, s).unwrap();
        assert_eq!(
            survey.questions.iter().map(|q| q.id).collect::<Vec<_>>(),
            [a, b]
        );
        assert_eq!(survey.suggestions.len(), 1);
        assert!(approve_question(&conn, sug).is_err());
        let survey = add_suggestion(&conn, sug).unwrap();
        assert_eq!(
            survey.questions.iter().map(|q| q.id).collect::<Vec<_>>(),
            [a, b, sug]
        );
        assert_eq!(survey.questions[2].review_status, ReviewStatus::Pending);
        assert!(survey.suggestions.is_empty());
    }

    #[test]
    fn reorder_rewrites_the_order_and_rejects_partial_lists() {
        let (conn, s) = setup();
        let a = ai(&conn, s, "A", false);
        let sug = ai(&conn, s, "S", true);
        let b = ai(&conn, s, "B", false);
        let c = ai(&conn, s, "C", false);
        let survey = reorder(&conn, s, &[c, a, b]).unwrap();
        assert_eq!(
            survey.questions.iter().map(|q| q.id).collect::<Vec<_>>(),
            [c, a, b]
        );
        assert_eq!(
            survey
                .questions
                .iter()
                .map(|q| q.order_index)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(survey.suggestions[0].id, sug);
        assert!(reorder(&conn, s, &[c, a]).is_err());
        assert!(reorder(&conn, s, &[c, a, a]).is_err());
    }

    #[test]
    fn editing_an_accepted_question_needs_approval_again_and_reopens_the_survey() {
        let (conn, s) = setup();
        let a = ai(&conn, s, "A", false);
        approve_question(&conn, a).unwrap();
        conn.execute("UPDATE surveys SET status = 'approved'", [])
            .unwrap();
        let q = question(&conn, a).unwrap();
        let mut body = q.body.clone();
        body.text = "Changed?".into();
        let q = update_question(&conn, a, body).unwrap();
        assert_eq!(q.review_status, ReviewStatus::Pending);
        assert_eq!(get(&conn, s).unwrap().status, SurveyStatus::InReview);
    }

    #[test]
    fn redraft_keeps_what_people_touched() {
        let (conn, s) = setup();
        let untouched = ai(&conn, s, "A", false);
        let approved = ai(&conn, s, "B", false);
        approve_question(&conn, approved).unwrap();
        let edited = ai(&conn, s, "C", false);
        let mut body = question(&conn, edited).unwrap().body;
        body.text = "Edited?".into();
        update_question(&conn, edited, body).unwrap();
        ai(&conn, s, "S", true);
        let human = add_question(&conn, s).unwrap().id;
        clear_untouched_ai(&conn, s).unwrap();
        let survey = get(&conn, s).unwrap();
        let ids: Vec<i64> = survey.questions.iter().map(|q| q.id).collect();
        assert_eq!(ids, [approved, edited, human]);
        assert!(!ids.contains(&untouched));
        assert!(survey.suggestions.is_empty());
    }

    #[test]
    fn a_critique_is_stored_with_its_question_and_cleared_by_an_edit() {
        use crate::model::{CriticFlag, CriticIssue};
        let (conn, s) = setup();
        let a = ai(&conn, s, "A", false);
        assert!(question(&conn, a).unwrap().critique.is_none());
        let target = start_critique(&conn, a, "critic.v1").unwrap();
        assert_eq!(target.body.text, "Question A?");
        let q = question(&conn, a).unwrap();
        assert_eq!(q.critique.unwrap().status, CriticStatus::Checking);
        let done = Critique {
            status: CriticStatus::Done,
            flags: vec![CriticFlag {
                issue: CriticIssue::DoubleBarrelled,
                note: "Asks two things.".into(),
            }],
            error: None,
            prompt_version: "critic.v1".into(),
        };
        assert!(save_critique(&conn, &target, &done).unwrap());
        assert_eq!(question(&conn, a).unwrap().critique.unwrap(), done);
        // Flags never block approval.
        assert_eq!(
            approve_question(&conn, a).unwrap().review_status,
            ReviewStatus::Accepted
        );

        // An edit clears the flags, and a check of the old wording can't bring them back.
        let stale = start_critique(&conn, a, "critic.v1").unwrap();
        let mut body = stale.body.clone();
        body.text = "Question A, reworded?".into();
        let q = update_question(&conn, a, body).unwrap();
        assert!(q.critique.is_none());
        assert!(!save_critique(&conn, &stale, &done).unwrap());
        assert!(question(&conn, a).unwrap().critique.is_none());

        // A check cut short by closing the app shows as failed, so it can be run again.
        start_critique(&conn, a, "critic.v1").unwrap();
        crate::db::runs::recover_on_launch(&conn).unwrap();
        let c = question(&conn, a).unwrap().critique.unwrap();
        assert_eq!(c.status, CriticStatus::Failed);
        assert!(c.error.unwrap().contains("app closed"));
    }

    #[test]
    fn title_and_intro_are_saved_and_redraft_keeps_them() {
        let (conn, s) = setup();
        set_drafted_intro(&conn, s, "Drafted intro.").unwrap();
        assert_eq!(get(&conn, s).unwrap().intro, "Drafted intro.");
        assert!(update_text(&conn, s, "  ", "x").is_err());
        assert!(update_text(&conn, s, "T", &"x".repeat(MAX_INTRO + 1)).is_err());
        conn.execute("UPDATE surveys SET status = 'approved'", [])
            .unwrap();
        // The title is for the researcher only; renaming keeps the approval.
        let survey = update_text(&conn, s, " Phone study ", "Drafted intro.").unwrap();
        assert_eq!(survey.title, "Phone study");
        assert_eq!(survey.status, SurveyStatus::Approved);
        // Respondents read the intro, so changing it needs approving again.
        let survey = update_text(&conn, s, "Phone study", " Thanks for helping. ").unwrap();
        assert_eq!(survey.intro, "Thanks for helping.");
        assert_eq!(survey.status, SurveyStatus::InReview);
        // A later draft doesn't overwrite what the person wrote.
        set_drafted_intro(&conn, s, "Another drafted intro.").unwrap();
        assert_eq!(get(&conn, s).unwrap().intro, "Thanks for helping.");
    }

    #[test]
    fn the_intro_cannot_change_under_an_unfinished_run() {
        let (conn, s) = setup();
        let a = ai(&conn, s, "A", false);
        approve_question(&conn, a).unwrap();
        conn.execute_batch(
            "INSERT INTO cohorts(project_id, name, config_json, status) VALUES (1, 'c', '{}', 'locked');
             UPDATE surveys SET status = 'approved';",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO simulation_runs(project_id, survey_id, cohort_id, survey_hash, provider, model, temperature, answer_mode, prompt_version, seed, max_concurrency, status)
             VALUES (1, ?1, 1, 'h', 'gemini', 'm', 1.0, 'whole_survey', 'v', 1, 4, 'paused')",
            [s],
        )
        .unwrap();
        assert!(update_text(&conn, s, "Renamed", "").is_ok());
        let err = update_text(&conn, s, "Renamed", "New intro").unwrap_err();
        assert!(err.message.contains("stop it"));
        conn.execute("UPDATE simulation_runs SET status = 'stopped'", [])
            .unwrap();
        assert_eq!(
            update_text(&conn, s, "Renamed", "New intro").unwrap().intro,
            "New intro"
        );
    }

    #[test]
    fn delete_retires_questions_that_have_answers() {
        let (conn, s) = setup();
        let a = ai(&conn, s, "A", false);
        let b = ai(&conn, s, "B", false);
        let survey = delete_question(&conn, a).unwrap();
        assert_eq!(survey.questions.len(), 1);
        assert!(question(&conn, a).is_err());
        // Pretend b was answered in a run.
        conn.execute_batch(
            "INSERT INTO cohorts(project_id, name, config_json, status) VALUES (1, 'c', '{}', 'locked');
             INSERT INTO respondents(cohort_id, ordinal, quota_cell, psychographic_summary, persona_json) VALUES (1, 1, '', 's', '{}');
             UPDATE questions SET review_status = 'accepted'; UPDATE surveys SET status = 'approved';",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO simulation_runs(project_id, survey_id, cohort_id, survey_hash, provider, model, temperature, answer_mode, prompt_version, seed, max_concurrency)
             VALUES (1, ?1, 1, 'h', 'gemini', 'm', 1.0, 'whole_survey', 'v', 1, 4)",
            [s],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO responses(run_id, question_id, respondent_id, answer_json) VALUES (1, ?1, 1, '{}')",
            [b],
        )
        .unwrap();
        let survey = delete_question(&conn, b).unwrap();
        assert!(survey.questions.is_empty());
        assert!(!question(&conn, b).unwrap().is_active);
    }
}
