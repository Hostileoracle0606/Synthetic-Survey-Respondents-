//! Tauri commands. Names and payloads follow docs/DATA_FLOW.md; types are generated into
//! `src/types/gen/` from `survey-core`.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use survey_core::db::{cohorts, projects, runs, surveys};
use survey_core::engine::cohort::{self, CohortJob};
use survey_core::engine::draft::{self, DraftBrief, DraftJob};
use survey_core::engine::persona::PROMPT_VERSION;
use survey_core::engine::run::{self as sim, Mode};
use survey_core::llm::gemini::{newest_stable, GeminiClient};
use survey_core::llm::LlmProvider;
use survey_core::model::{
    Cohort, CohortConfig, CohortProgress, CohortSummary, CountryOption, DraftStatus, Project,
    Question, QuestionBody, QuotaGroup, RespondentDetail, RespondentPage, RunConfig, RunProgress,
    RunStatus, SimulationRun, Survey, SurveyInfo,
};
use survey_core::{sampling, AppError, AppResult, ErrorCode};

use crate::{keychain, AppState};

fn lock<'a>(
    state: &'a State<'_, AppState>,
) -> AppResult<std::sync::MutexGuard<'a, rusqlite::Connection>> {
    state
        .db
        .lock()
        .map_err(|_| AppError::new(ErrorCode::Internal, "database lock poisoned"))
}

fn gemini() -> AppResult<GeminiClient> {
    let key = keychain::get()?.ok_or_else(|| AppError::invalid("no Gemini API key stored"))?;
    Ok(GeminiClient::new(key))
}

async fn models(state: &State<'_, AppState>, client: &GeminiClient) -> AppResult<Vec<String>> {
    let mut cached = state.models.lock().await;
    if let Some(m) = cached.as_ref() {
        return Ok(m.clone());
    }
    let list = client.list_models().await?;
    *cached = Some(list.clone());
    Ok(list)
}

/// Newest stable Flash: personas and answering.
async fn flash_model(state: &State<'_, AppState>, client: &GeminiClient) -> AppResult<String> {
    newest_stable(&models(state, client).await?, "flash").ok_or_else(|| {
        AppError::new(
            ErrorCode::Llm,
            "no stable Gemini Flash model is available to this key",
        )
    })
}

/// Newest stable Pro for drafting, falling back to Flash when the key has no Pro access.
async fn draft_model(state: &State<'_, AppState>, client: &GeminiClient) -> AppResult<String> {
    match newest_stable(&models(state, client).await?, "pro") {
        Some(m) => Ok(m),
        None => flash_model(state, client).await,
    }
}

#[tauri::command]
pub fn list_countries() -> Vec<CountryOption> {
    survey_core::countries::list().to_vec()
}

/// Step 1 quota groups for a country selection: census shares where a table exists.
#[tauri::command]
pub fn default_quotas(countries: Vec<String>) -> Vec<QuotaGroup> {
    sampling::default_quotas(&countries)
}

#[tauri::command]
pub fn save_survey_info(
    state: State<'_, AppState>,
    project_id: Option<i64>,
    info: SurveyInfo,
) -> AppResult<Project> {
    projects::save_survey_info(&*lock(&state)?, project_id, &info)
}

#[tauri::command]
pub fn get_project(state: State<'_, AppState>, project_id: i64) -> AppResult<Project> {
    projects::get_project(&*lock(&state)?, project_id)
}

/// Step 1 → Step 2: checks the gate, creates the cohort and starts the persona job in the
/// background. Progress arrives on `on_progress`; the cohort's status says when it is done.
#[tauri::command]
pub async fn generate_cohort(
    state: State<'_, AppState>,
    project_id: i64,
    config: CohortConfig,
    on_progress: Channel<CohortProgress>,
) -> AppResult<Cohort> {
    let cohort = start_cohort(&state, project_id, config, None, on_progress).await?;
    // The questionnaire is drafted alongside the personas, once per project.
    let survey = surveys::for_project(&*lock(&state)?, project_id)?;
    if survey.draft_status == DraftStatus::None && survey.questions.is_empty() {
        start_draft(&state, &survey).await?;
    }
    Ok(cohort)
}

/// New cohort version with the same audience and a new seed (different people).
#[tauri::command]
pub async fn regenerate_cohort(
    state: State<'_, AppState>,
    cohort_id: i64,
    on_progress: Channel<CohortProgress>,
) -> AppResult<Cohort> {
    let old = cohorts::get(&*lock(&state)?, cohort_id)?;
    let mut config = old.config.clone();
    config.seed = config
        .seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
        >> 11;
    start_cohort(&state, old.project_id, config, Some(old.id), on_progress).await
}

async fn start_cohort(
    state: &State<'_, AppState>,
    project_id: i64,
    config: CohortConfig,
    parent: Option<i64>,
    on_progress: Channel<CohortProgress>,
) -> AppResult<Cohort> {
    let project = projects::get_project(&*lock(state)?, project_id)?;
    if project.research_type.is_none() {
        return Err(AppError::invalid("choose a research type first"));
    }
    // Validates quotas and countries before anything is saved.
    sampling::Sampler::new(&config, &project.countries)?;
    let client = gemini()?;
    let model = flash_model(state, &client).await?;
    let cohort = cohorts::create(
        &*lock(state)?,
        project_id,
        &config,
        parent,
        &model,
        PROMPT_VERSION,
    )?;

    let job = CohortJob {
        cohort_id: cohort.id,
        countries: project.countries.clone(),
        category: project.product_category.clone(),
        config,
        model,
        batch_size: 8,
        max_screen_attempts: 3,
    };
    let llm: Arc<dyn LlmProvider> = Arc::new(client);
    let limiter = state.limiter.clone();
    let writer = state.writer.clone();
    tauri::async_runtime::spawn(async move {
        let progress = Arc::new(move |p: CohortProgress| {
            let _ = on_progress.send(p);
        });
        // The outcome is stored on the cohort (ready, or failed with the reason).
        let _ = cohort::run(job, llm, limiter, writer, progress).await;
    });
    Ok(cohort)
}

#[tauri::command]
pub fn get_latest_cohort(state: State<'_, AppState>, project_id: i64) -> AppResult<Option<Cohort>> {
    cohorts::latest(&*lock(&state)?, project_id)
}

#[tauri::command]
pub fn get_cohort_summary(state: State<'_, AppState>, cohort_id: i64) -> AppResult<CohortSummary> {
    let conn = lock(&state)?;
    let cohort = cohorts::get(&conn, cohort_id)?;
    let project = projects::get_project(&conn, cohort.project_id)?;
    cohorts::summary(&conn, cohort_id, project.product_category.as_deref())
}

#[tauri::command]
pub fn list_respondents(
    state: State<'_, AppState>,
    cohort_id: i64,
    query: String,
    offset: u32,
    limit: u32,
) -> AppResult<RespondentPage> {
    cohorts::list(&*lock(&state)?, cohort_id, &query, offset, limit.min(200))
}

#[tauri::command]
pub fn get_respondent(
    state: State<'_, AppState>,
    respondent_id: i64,
) -> AppResult<RespondentDetail> {
    cohorts::detail(&*lock(&state)?, respondent_id)
}

/// Proceed to Questionnaire: locks the cohort so it can't change under a survey run.
#[tauri::command]
pub fn lock_cohort(state: State<'_, AppState>, cohort_id: i64) -> AppResult<Cohort> {
    cohorts::lock(&*lock(&state)?, cohort_id)
}

/// Marks the survey `generating` and starts the draft job; the outcome is stored on the survey.
async fn start_draft(state: &State<'_, AppState>, survey: &Survey) -> AppResult<()> {
    let client = gemini()?;
    let model = draft_model(state, &client).await?;
    let (brief, survey_id) = {
        let conn = lock(state)?;
        let p = projects::get_project(&conn, survey.project_id)?;
        let research_type = p
            .research_type
            .ok_or_else(|| AppError::invalid("choose a research type first"))?;
        let brief = DraftBrief {
            research_type,
            product_category: p.product_category.clone(),
            countries: p
                .countries
                .iter()
                .map(|c| {
                    survey_core::countries::find(c).map_or_else(|| c.clone(), |x| x.name.clone())
                })
                .collect(),
            title: p.title.clone(),
            objective: p.research_goal.clone(),
        };
        surveys::set_generation(
            &conn,
            survey.id,
            &model,
            draft::PROMPT_VERSION,
            &brief.to_json(),
        )?;
        surveys::set_draft_status(&conn, survey.id, DraftStatus::Generating, None)?;
        (brief, survey.id)
    };
    let llm: Arc<dyn LlmProvider> = Arc::new(client);
    let limiter = state.limiter.clone();
    let writer = state.writer.clone();
    tauri::async_runtime::spawn(async move {
        let _ = draft::run(
            DraftJob {
                survey_id,
                brief,
                model,
            },
            llm,
            limiter,
            writer,
        )
        .await;
    });
    Ok(())
}

#[tauri::command]
pub fn get_survey(state: State<'_, AppState>, project_id: i64) -> AppResult<Survey> {
    surveys::for_project(&*lock(&state)?, project_id)
}

/// Redraft: a new Gemini draft replaces the AI questions nobody has touched yet.
#[tauri::command]
pub async fn redraft_survey(state: State<'_, AppState>, project_id: i64) -> AppResult<Survey> {
    let survey = surveys::for_project(&*lock(&state)?, project_id)?;
    if survey.draft_status == DraftStatus::Generating {
        return Err(AppError::invalid("a draft is already being written"));
    }
    start_draft(&state, &survey).await?;
    surveys::get(&*lock(&state)?, survey.id)
}

#[tauri::command]
pub fn update_question(
    state: State<'_, AppState>,
    question_id: i64,
    body: QuestionBody,
) -> AppResult<Question> {
    surveys::update_question(&*lock(&state)?, question_id, body)
}

#[tauri::command]
pub fn reorder_questions(
    state: State<'_, AppState>,
    survey_id: i64,
    ordered_ids: Vec<i64>,
) -> AppResult<Survey> {
    surveys::reorder(&*lock(&state)?, survey_id, &ordered_ids)
}

#[tauri::command]
pub fn add_question(state: State<'_, AppState>, survey_id: i64) -> AppResult<Question> {
    surveys::add_question(&*lock(&state)?, survey_id)
}

#[tauri::command]
pub fn delete_question(state: State<'_, AppState>, question_id: i64) -> AppResult<Survey> {
    surveys::delete_question(&*lock(&state)?, question_id)
}

#[tauri::command]
pub fn approve_question(state: State<'_, AppState>, question_id: i64) -> AppResult<Question> {
    surveys::approve_question(&*lock(&state)?, question_id)
}

#[tauri::command]
pub fn add_suggestion(state: State<'_, AppState>, question_id: i64) -> AppResult<Survey> {
    surveys::add_suggestion(&*lock(&state)?, question_id)
}

/// Run Survey Simulation: approves the survey and creates the run in one transaction (the
/// database refuses it if any question is unreviewed), then starts answering.
#[tauri::command]
pub async fn start_simulation(
    state: State<'_, AppState>,
    project_id: i64,
    config: RunConfig,
    on_progress: Channel<RunProgress>,
) -> AppResult<SimulationRun> {
    let client = gemini()?;
    let model = flash_model(&state, &client).await?;
    let limits = state.limiter.limits();
    let left = limits
        .requests_per_day
        .saturating_sub(state.limiter.used_today().await);
    let run = runs::start(
        &*lock(&state)?,
        project_id,
        &config,
        &runs::RunSettings {
            model: &model,
            max_concurrency: limits.max_concurrency,
            requests_left_today: left,
        },
    )?;
    launch(&state, client, run.id, on_progress)?;
    Ok(run)
}

/// Starts (or restarts) the engine for a run; it skips respondents already saved.
fn launch(
    state: &State<'_, AppState>,
    client: GeminiClient,
    run_id: i64,
    on_progress: Channel<RunProgress>,
) -> AppResult<()> {
    let plan = runs::plan(&*lock(state)?, run_id)?;
    let (tx, rx) = tokio::sync::watch::channel(Mode::Run);
    {
        let mut active = state
            .runs
            .lock()
            .map_err(|_| AppError::new(ErrorCode::Internal, "run table lock poisoned"))?;
        if active.contains_key(&run_id) {
            return Err(AppError::invalid("this run is already going"));
        }
        active.insert(run_id, tx);
    }
    let llm: Arc<dyn LlmProvider> = Arc::new(client);
    let limiter = state.limiter.clone();
    let writer = state.writer.clone();
    let active = state.runs.clone();
    tauri::async_runtime::spawn(async move {
        let progress = Arc::new(move |p: RunProgress| {
            let _ = on_progress.send(p);
        });
        // The outcome (completed, paused with a reason, stopped) is stored on the run.
        let _ = sim::run(plan, llm, limiter, writer, rx, progress).await;
        if let Ok(mut m) = active.lock() {
            m.remove(&run_id);
        }
    });
    Ok(())
}

#[tauri::command]
pub fn get_latest_run(
    state: State<'_, AppState>,
    project_id: i64,
) -> AppResult<Option<SimulationRun>> {
    runs::latest(&*lock(&state)?, project_id)
}

fn signal(state: &State<'_, AppState>, run_id: i64, mode: Mode) -> AppResult<bool> {
    let active = state
        .runs
        .lock()
        .map_err(|_| AppError::new(ErrorCode::Internal, "run table lock poisoned"))?;
    Ok(match active.get(&run_id) {
        Some(tx) => tx.send(mode).is_ok(),
        None => false,
    })
}

/// Pause Simulation: calls in flight finish and are saved; nothing new starts.
#[tauri::command]
pub fn pause_run(state: State<'_, AppState>, run_id: i64) -> AppResult<()> {
    signal(&state, run_id, Mode::Pause)?;
    Ok(())
}

#[tauri::command]
pub async fn resume_run(
    state: State<'_, AppState>,
    run_id: i64,
    on_progress: Channel<RunProgress>,
) -> AppResult<SimulationRun> {
    let run = runs::get(&*lock(&state)?, run_id)?;
    if run.status != RunStatus::Paused {
        return Err(AppError::invalid("only a paused run can be resumed"));
    }
    launch(&state, gemini()?, run_id, on_progress)?;
    runs::get(&*lock(&state)?, run_id)
}

/// Stop & Save Progress: like pause, then final; the report uses what was answered.
#[tauri::command]
pub fn stop_run(state: State<'_, AppState>, run_id: i64) -> AppResult<SimulationRun> {
    if !signal(&state, run_id, Mode::Stop)? {
        // Not running (e.g. paused): stop it directly.
        let conn = lock(&state)?;
        let run = runs::get(&conn, run_id)?;
        if matches!(run.status, RunStatus::Paused | RunStatus::Queued) {
            runs::set_status(&conn, run_id, RunStatus::Stopped, None)?;
        }
    }
    runs::get(&*lock(&state)?, run_id)
}

#[tauri::command]
pub fn set_api_key(key: String) -> AppResult<()> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AppError::invalid("the API key is empty"));
    }
    keychain::set(key)
}

#[tauri::command]
pub fn has_api_key() -> AppResult<bool> {
    Ok(keychain::get()?.is_some())
}

#[tauri::command]
pub fn delete_api_key() -> AppResult<()> {
    keychain::delete()
}

/// "Test connection": lists the Gemini models the stored key can use.
#[tauri::command]
pub async fn test_connection() -> AppResult<Vec<String>> {
    Ok(gemini()?.list_models().await?)
}
