//! Step 4 persistence: simulation runs and their answers.

use rusqlite::{params, Connection, OptionalExtension};

use crate::db::{cohorts, surveys};
use crate::engine::answer::{self, Checked};
use crate::error::{AppError, AppResult};
use crate::llm::Usage;
use crate::model::{
    CohortStatus, CostEstimate, ModelPrice, Question, QuestionType, RespondentDetail, RunConfig,
    RunStatus, SimulationRun,
};
use crate::pricing;

/// Everything fixed when a run starts.
pub struct RunSettings<'a> {
    pub model: &'a str,
    pub max_concurrency: u32,
    /// Gemini requests still available today; the run is refused if it can't finish.
    pub requests_left_today: u32,
    /// From [`estimate`], stored so Step 4 can show actual against estimated.
    pub est_cost_usd: Option<f64>,
}

/// Run Survey Simulation: in one transaction, approve the survey and insert the run. The
/// `trg_runs_require_approved_survey` trigger refuses it (and the approval rolls back) if any
/// active question hasn't been accepted by a person.
pub fn start(
    conn: &Connection,
    project_id: i64,
    config: &RunConfig,
    s: &RunSettings,
) -> AppResult<SimulationRun> {
    let cohort = cohorts::latest(conn, project_id)?
        .filter(|c| c.status == CohortStatus::Locked)
        .ok_or_else(|| AppError::invalid("lock a cohort in Step 2 first"))?;
    let survey = surveys::for_project(conn, project_id)?;
    if survey.questions.is_empty() {
        return Err(AppError::invalid("the survey has no questions"));
    }
    let running: bool = conn
        .query_row(
            "SELECT 1 FROM simulation_runs WHERE project_id = ?1 AND status IN ('queued','running')",
            [project_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if running {
        return Err(AppError::invalid(
            "a simulation is already running for this project",
        ));
    }
    let respondents = kept_respondents(conn, cohort.id)?;
    if respondents == 0 {
        return Err(AppError::invalid("the cohort has no respondents"));
    }
    if s.requests_left_today < respondents {
        return Err(AppError::invalid(format!(
            "this run needs about {respondents} Gemini requests but only {} are left today; try a smaller cohort or wait for the daily reset",
            s.requests_left_today
        )));
    }
    let seed = config.seed.unwrap_or(cohort.config.seed);
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE surveys SET status = 'approved', approved_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?1",
        [survey.id],
    )?;
    tx.execute(
        "INSERT INTO simulation_runs(project_id, survey_id, cohort_id, survey_hash, provider, model, temperature,
            answer_mode, prompt_version, seed, max_concurrency, status, est_cost_usd)
         VALUES (?1, ?2, ?3, ?4, 'gemini', ?5, ?6, 'whole_survey', ?7, ?8, ?9, 'queued', ?10)",
        params![
            project_id,
            survey.id,
            cohort.id,
            surveys::survey_hash(&survey),
            s.model,
            f64::from(answer::TEMPERATURE),
            answer::PROMPT_VERSION,
            seed as i64,
            s.max_concurrency,
            s.est_cost_usd
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE projects SET wizard_step = MAX(wizard_step, 4) WHERE id = ?1",
        [project_id],
    )?;
    tx.commit()?;
    get(conn, id)
}

fn kept_respondents(conn: &Connection, cohort_id: i64) -> AppResult<u32> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM respondents WHERE cohort_id = ?1 AND screen_status <> 'failed'",
        [cohort_id],
        |r| r.get(0),
    )?)
}

pub fn get(conn: &Connection, id: i64) -> AppResult<SimulationRun> {
    let row = conn
        .query_row(
            "SELECT r.project_id, r.survey_id, r.cohort_id, r.status, r.model, r.error, r.created_at, r.prompt_version, r.est_cost_usd,
                (SELECT COUNT(*) FROM questions q WHERE q.survey_id = r.survey_id AND q.is_active = 1),
                (SELECT COUNT(*) FROM responses x WHERE x.run_id = r.id),
                (SELECT COUNT(DISTINCT x.respondent_id) FROM responses x WHERE x.run_id = r.id)
             FROM simulation_runs r WHERE r.id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, u32>(9)?,
                    r.get::<_, u32>(10)?,
                    r.get::<_, u32>(11)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, Option<f64>>(8)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("run {id} not found")))?;
    let cost_usd = pricing::run_price(conn)?
        .map(|p| pricing::run_cost(conn, id, &p))
        .transpose()?;
    Ok(SimulationRun {
        id,
        project_id: row.0,
        survey_id: row.1,
        cohort_id: row.2,
        status: RunStatus::from_db(&row.3),
        model: row.4,
        prompt_version: row.10,
        respondents: kept_respondents(conn, row.2)?,
        questions: row.7,
        answered: row.8,
        respondents_done: row.9,
        error: row.5,
        created_at: row.6,
        est_cost_usd: row.11,
        cost_usd,
    })
}

/// Rule of thumb for output tokens when no earlier run on the model says otherwise: the
/// reply's JSON wrapper and some thinking per call, plus an answer and a one-sentence reason
/// per question (open answers are longer).
const OUTPUT_PER_CALL: u64 = 300;
const OUTPUT_PER_QUESTION: u64 = 45;
const OUTPUT_PER_OPEN_QUESTION: u64 = 100;

/// The estimate shown before Run Survey Simulation: one call per kept respondent, input
/// tokens counted from the real prompts (≈ 4 characters per token, schema included), output
/// tokens from earlier runs on the same model when there are any. Every input token is
/// priced as uncached, so implicit cache hits only make the run cheaper.
pub fn estimate(conn: &Connection, project_id: i64, model: &str) -> AppResult<CostEstimate> {
    let cohort = cohorts::latest(conn, project_id)?
        .filter(|c| c.status == CohortStatus::Locked)
        .ok_or_else(|| AppError::invalid("lock a cohort in Step 2 first"))?;
    let survey = surveys::for_project(conn, project_id)?;
    let seed = cohort.config.seed;
    let ids: Vec<i64> = conn
        .prepare(
            "SELECT id FROM respondents WHERE cohort_id = ?1 AND screen_status <> 'failed' ORDER BY ordinal",
        )?
        .query_map([cohort.id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut input_tokens = 0u64;
    for id in &ids {
        let d = cohorts::detail(conn, *id)?;
        let pairs: Vec<(&Question, Vec<String>)> = survey
            .questions
            .iter()
            .map(|q| (q, answer::shown_order(q, seed, d.ordinal)))
            .collect();
        let req = answer::build_request(model, &survey.intro, &pairs, &answer::persona_text(&d));
        let chars = req.system.len() + req.prompt.len() + req.schema.to_string().len();
        input_tokens += chars.div_ceil(4) as u64;
    }
    let calls = ids.len() as u32;
    // Output tokens per answer stored, from earlier runs on this model.
    let history: Option<f64> = conn.query_row(
        "SELECT CAST(SUM(c.output_tokens) AS REAL) / NULLIF((SELECT COUNT(*) FROM responses x
                 JOIN simulation_runs rr ON rr.id = x.run_id WHERE rr.model = ?1), 0)
         FROM llm_calls c JOIN simulation_runs r ON r.id = c.run_id
         WHERE r.model = ?1 AND c.purpose = 'answer' AND c.output_tokens IS NOT NULL",
        [model],
        |r| r.get(0),
    )?;
    let questions = survey.questions.len() as u64;
    let (output_tokens, output_from_history) = match history {
        Some(per_answer) if per_answer > 0.0 => (
            (per_answer * (questions * u64::from(calls)) as f64).ceil() as u64,
            true,
        ),
        _ => {
            let per_call = OUTPUT_PER_CALL
                + survey
                    .questions
                    .iter()
                    .map(|q| match q.body.question_type {
                        QuestionType::OpenEnded => OUTPUT_PER_OPEN_QUESTION,
                        _ => OUTPUT_PER_QUESTION,
                    })
                    .sum::<u64>();
            (per_call * u64::from(calls), false)
        }
    };
    let cost_usd = pricing::run_price(conn)?.map(|p| {
        pricing::cost(
            &p,
            &Usage {
                input_tokens: input_tokens.min(u64::from(u32::MAX)) as u32,
                cached_tokens: 0,
                output_tokens: output_tokens.min(u64::from(u32::MAX)) as u32,
            },
        )
    });
    Ok(CostEstimate {
        model: model.to_string(),
        calls,
        input_tokens,
        output_tokens,
        cost_usd,
        output_from_history,
    })
}

pub fn latest(conn: &Connection, project_id: i64) -> AppResult<Option<SimulationRun>> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM simulation_runs WHERE project_id = ?1 ORDER BY id DESC LIMIT 1",
            [project_id],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|id| get(conn, id)).transpose()
}

pub fn set_status(
    conn: &Connection,
    id: i64,
    status: RunStatus,
    error: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE simulation_runs SET status = ?2, error = ?3,
            started_at = CASE WHEN ?2 = 'running' THEN COALESCE(started_at, strftime('%Y-%m-%dT%H:%M:%fZ','now')) ELSE started_at END,
            finished_at = CASE WHEN ?2 IN ('completed','stopped','failed','cancelled') THEN strftime('%Y-%m-%dT%H:%M:%fZ','now') ELSE finished_at END
         WHERE id = ?1",
        params![id, status.as_db(), error],
    )?;
    Ok(())
}

/// On launch, after the app closed or crashed: runs left `running` become `paused`, and
/// survey drafts, wording checks or report syntheses left unfinished become `failed`, so their
/// Retry, Check again and Regenerate buttons work again. Returns the number of runs paused.
pub fn recover_on_launch(conn: &Connection) -> AppResult<usize> {
    conn.execute(
        "UPDATE surveys SET draft_status = 'failed', draft_error = 'The app closed while the draft was being written.'
         WHERE draft_status = 'generating'",
        [],
    )?;
    conn.execute(
        "UPDATE questions SET critic_json = json_set(critic_json, '$.status', 'failed', '$.error', 'The app closed while the wording was being checked.')
         WHERE json_extract(critic_json, '$.status') = 'checking'",
        [],
    )?;
    conn.execute(
        "UPDATE simulation_runs SET synthesis_status = 'failed', synthesis_error = 'The app closed while the summary was being written.'
         WHERE synthesis_status = 'generating'",
        [],
    )?;
    Ok(conn.execute(
        "UPDATE simulation_runs SET status = 'paused', error = 'The app closed during the run. Resume to continue.'
         WHERE status IN ('running','queued')",
        [],
    )?)
}

pub struct PlannedRespondent {
    pub id: i64,
    pub ordinal: u32,
    pub persona: String,
}

/// What the engine needs to (re)start a run: the questions and the respondents still to do.
pub struct RunPlan {
    pub run_id: i64,
    pub model: String,
    pub seed: u64,
    pub max_concurrency: u32,
    pub intro: String,
    pub questions: Vec<Question>,
    pub todo: Vec<PlannedRespondent>,
    pub respondents_total: u32,
    pub respondents_done: u32,
    pub answered: u32,
    /// The saved Flash price (BACKLOG B1), if any: every simulation run answers with Flash.
    /// Without one the live cost stays "$—" until the user fills in Settings.
    pub price: Option<ModelPrice>,
    /// Cost of the calls already made (a resumed run continues from here).
    pub cost_so_far: f64,
}

pub fn plan(conn: &Connection, run_id: i64) -> AppResult<RunPlan> {
    let run = get(conn, run_id)?;
    let (model, seed, max_concurrency): (String, i64, u32) = conn.query_row(
        "SELECT model, seed, max_concurrency FROM simulation_runs WHERE id = ?1",
        [run_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let survey = surveys::get(conn, run.survey_id)?;
    // Everyone in a run answers the survey that was approved when it started.
    let approved_hash: String = conn.query_row(
        "SELECT survey_hash FROM simulation_runs WHERE id = ?1",
        [run_id],
        |r| r.get(0),
    )?;
    if surveys::survey_hash(&survey) != approved_hash {
        return Err(AppError::invalid(
            "the survey changed after this run started, so it can't be resumed; stop it (its answers stay in the report) and run the new survey",
        ));
    }
    // A respondent's answers are saved together, so any answer means they are done.
    let ids: Vec<i64> = conn
        .prepare(
            "SELECT id FROM respondents r WHERE cohort_id = ?1 AND screen_status <> 'failed'
               AND NOT EXISTS (SELECT 1 FROM responses x WHERE x.run_id = ?2 AND x.respondent_id = r.id)
             ORDER BY ordinal",
        )?
        .query_map(params![run.cohort_id, run_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let todo = ids
        .into_iter()
        .map(|id| {
            let d: RespondentDetail = cohorts::detail(conn, id)?;
            Ok(PlannedRespondent {
                id,
                ordinal: d.ordinal,
                persona: answer::persona_text(&d),
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(RunPlan {
        run_id,
        model,
        seed: seed as u64,
        max_concurrency,
        intro: survey.intro,
        questions: survey.questions,
        todo,
        respondents_total: run.respondents,
        respondents_done: run.respondents_done,
        answered: run.answered,
        price: pricing::run_price(conn)?,
        cost_so_far: run.cost_usd.unwrap_or(0.0),
    })
}

/// One question's outcome for a respondent.
pub enum Outcome {
    Valid(Checked),
    Invalid { problem: String },
    Refused { reason: String },
}

pub struct SavedAnswer {
    pub question_id: i64,
    pub shown: Vec<String>,
    pub outcome: Outcome,
}

/// Saves a respondent's answers and the call that produced them, all at once, so a
/// respondent is either fully saved or not at all (resume relies on this).
pub fn save_respondent(
    conn: &Connection,
    run_id: i64,
    respondent_id: i64,
    call: Option<(u32, Usage, u64)>,
    answers: &[SavedAnswer],
) -> AppResult<()> {
    let call_id = match call {
        Some((attempt, u, latency)) => {
            conn.execute(
                "INSERT INTO llm_calls(run_id, respondent_id, purpose, attempt, input_tokens, cached_tokens, output_tokens, latency_ms)
                 VALUES (?1, ?2, 'answer', ?3, ?4, ?5, ?6, ?7)",
                params![run_id, respondent_id, attempt, u.input_tokens, u.cached_tokens, u.output_tokens, latency as i64],
            )?;
            Some(conn.last_insert_rowid())
        }
        None => None,
    };
    let mut stmt = conn.prepare_cached(
        "INSERT INTO responses(run_id, question_id, respondent_id, llm_call_id, answer_json, answer_value, answer_code,
            reasoning, shown_options_json, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for a in answers {
        let shown =
            (!a.shown.is_empty()).then(|| serde_json::to_string(&a.shown).unwrap_or_default());
        let (json, value, code, reasoning, status) = match &a.outcome {
            Outcome::Valid(c) => (
                Some(c.answer_json.to_string()),
                c.value,
                c.code.clone(),
                Some(c.reason.clone()),
                "valid",
            ),
            Outcome::Invalid { problem } => (None, None, None, Some(problem.clone()), "invalid"),
            Outcome::Refused { reason } => (None, None, None, Some(reason.clone()), "refused"),
        };
        stmt.execute(params![
            run_id,
            a.question_id,
            respondent_id,
            call_id,
            json,
            value,
            code,
            reasoning,
            shown,
            status
        ])?;
    }
    Ok(())
}
