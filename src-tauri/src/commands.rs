//! Tauri commands. Names and payloads follow docs/DATA_FLOW.md; types are generated into
//! `src/types/gen/` from `survey-core`.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use survey_core::db::{cohorts, projects, runs, settings as settings_db, surveys};
use survey_core::engine::cohort::{self, CohortJob};
use survey_core::engine::draft::{self, DraftBrief, DraftJob};
use survey_core::engine::persona::PROMPT_VERSION;
use survey_core::engine::run::{self as sim, Mode};
use survey_core::engine::synthesis::{self, SynthesisJob};
use survey_core::llm::gemini::{drafting_model, newest_stable, GeminiClient};
use survey_core::llm::LlmProvider;
use survey_core::model::{
    Cohort, CohortConfig, CohortProgress, CohortSummary, CountryOption, CrossTab, DraftStatus,
    ExportFormat, Project, Question, QuestionBody, QuotaGroup, Report, RespondentDetail,
    RespondentPage, RunConfig, RunProgress, RunStatus, Settings, SimulationRun, Survey, SurveyInfo,
    SynthesisStatus,
};
use survey_core::report::{self, export};
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

/// Newest stable Flash: personas and answering. Settings (BACKLOG B1) can override the pick.
async fn flash_model(state: &State<'_, AppState>, client: &GeminiClient) -> AppResult<String> {
    if let Some(m) = settings_db::get(&*lock(state)?)?.flash_model {
        return Ok(m);
    }
    newest_stable(&models(state, client).await?, "flash").ok_or_else(|| {
        AppError::new(
            ErrorCode::Llm,
            "no stable Gemini Flash model is available to this key",
        )
    })
}

/// Drafting, theme coding and synthesis: newest stable Pro, unless Flash is a newer generation.
/// Settings (BACKLOG B1) can override the pick.
async fn draft_model(state: &State<'_, AppState>, client: &GeminiClient) -> AppResult<String> {
    if let Some(m) = settings_db::get(&*lock(state)?)?.pro_model {
        return Ok(m);
    }
    drafting_model(&models(state, client).await?).ok_or_else(|| {
        AppError::new(
            ErrorCode::Llm,
            "no stable Gemini model is available to this key",
        )
    })
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppResult<Settings> {
    settings_db::get(&*lock(&state)?)
}

/// Saves Settings (BACKLOG B1). The usage tier's new rate limits take effect on next launch;
/// a model override or price takes effect on the next Gemini call or run.
#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> AppResult<Settings> {
    settings_db::save(&*lock(&state)?, &settings)
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

/// The most recently updated project, if any: used to reopen the app on the last project.
#[tauri::command]
pub fn get_last_project(state: State<'_, AppState>) -> AppResult<Option<Project>> {
    projects::latest(&*lock(&state)?)
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
    mut config: CohortConfig,
    parent: Option<i64>,
    on_progress: Channel<CohortProgress>,
) -> AppResult<Cohort> {
    let project = projects::get_project(&*lock(state)?, project_id)?;
    if project.research_type.is_none() {
        return Err(AppError::invalid("choose a research type first"));
    }
    // Validates quotas and countries before anything is saved.
    sampling::Sampler::new(&config, &project.countries)?;
    // Snapshot the countries this cohort was generated for, so Step 2 can tell if Step 1's
    // audience has since drifted (BACKLOG B4).
    config.countries = project.countries.clone();
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
    if let Err(e) = launch(&state, client, run.id, on_progress).await {
        // Don't leave a queued run behind: it would block every later start.
        runs::set_status(&*lock(&state)?, run.id, RunStatus::Failed, Some(&e.message))?;
        return Err(e);
    }
    Ok(run)
}

/// Starts (or restarts) the engine for a run; it skips respondents already saved.
async fn launch(
    state: &State<'_, AppState>,
    client: GeminiClient,
    run_id: i64,
    on_progress: Channel<RunProgress>,
) -> AppResult<()> {
    let synthesis_model = draft_model(state, &client).await?;
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
    let db_path = state.db_path.clone();
    tauri::async_runtime::spawn(async move {
        let progress = Arc::new(move |p: RunProgress| {
            let _ = on_progress.send(p);
        });
        // The outcome (completed, paused with a reason, stopped) is stored on the run.
        let outcome = sim::run(
            plan,
            llm.clone(),
            limiter.clone(),
            writer.clone(),
            rx,
            progress,
        )
        .await;
        if let Ok(mut m) = active.lock() {
            m.remove(&run_id);
        }
        // The report's themes and AI synthesis follow a finished or stopped run.
        if let Ok(o) = outcome {
            if matches!(o.status, RunStatus::Completed | RunStatus::Stopped) {
                let job = SynthesisJob {
                    run_id,
                    model: synthesis_model,
                    db_path,
                    recode_themes: false,
                };
                let _ = synthesis::run(job, llm, limiter, writer).await;
            }
        }
    });
    Ok(())
}

/// Starts theme coding and synthesis in the background; the outcome is stored on the run.
async fn start_synthesis(
    state: &State<'_, AppState>,
    run_id: i64,
    recode_themes: bool,
) -> AppResult<()> {
    let client = gemini()?;
    let job = SynthesisJob {
        run_id,
        model: draft_model(state, &client).await?,
        db_path: state.db_path.clone(),
        recode_themes,
    };
    let llm: Arc<dyn LlmProvider> = Arc::new(client);
    let (limiter, writer) = (state.limiter.clone(), state.writer.clone());
    tauri::async_runtime::spawn(async move {
        let _ = synthesis::run(job, llm, limiter, writer).await;
    });
    Ok(())
}

#[tauri::command]
pub fn get_report(state: State<'_, AppState>, run_id: i64) -> AppResult<Report> {
    report::report(&*lock(&state)?, run_id)
}

#[tauri::command]
pub fn get_crosstab(
    state: State<'_, AppState>,
    run_id: i64,
    question_id: i64,
    dimension: String,
) -> AppResult<CrossTab> {
    report::crosstab(&*lock(&state)?, run_id, question_id, &dimension)
}

/// Regenerate: a new synthesis from the same numbers and themes.
#[tauri::command]
pub async fn regenerate_synthesis(state: State<'_, AppState>, run_id: i64) -> AppResult<Report> {
    let current = report::report(&*lock(&state)?, run_id)?;
    if current.synthesis_status == SynthesisStatus::Generating {
        return Err(AppError::invalid("a synthesis is already being written"));
    }
    start_synthesis(&state, run_id, false).await?;
    report::report(&*lock(&state)?, run_id)
}

/// Offline export through the system save dialog. Returns the saved path, or None if the
/// user cancelled. Nothing is uploaded.
#[tauri::command]
pub async fn export_run(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    run_id: i64,
    format: ExportFormat,
) -> AppResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (ext, label) = match format {
        ExportFormat::Csv => ("csv", "CSV (Excel)"),
        ExportFormat::Json => ("json", "JSON"),
    };
    let title = {
        let conn = lock(&state)?;
        let project_id = runs::get(&conn, run_id)?.project_id;
        projects::get_project(&conn, project_id)?.title
    };
    let name: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter(label, &[ext])
            .set_file_name(format!("{} - synthetic run {run_id}.{ext}", name.trim()))
            .blocking_save_file()
    })
    .await
    .map_err(|e| AppError::new(ErrorCode::Internal, e.to_string()))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else {
        return Ok(None);
    };
    let saving = |e: std::io::Error| {
        AppError::new(ErrorCode::Internal, format!("could not save the file: {e}"))
    };
    let conn = lock(&state)?;
    match format {
        ExportFormat::Csv => std::fs::write(&path, export::csv(&conn, run_id)?).map_err(saving)?,
        ExportFormat::Json => {
            // Streamed straight to the file, so large runs never sit in memory whole.
            let mut file = std::io::BufWriter::new(std::fs::File::create(&path).map_err(saving)?);
            export::write_json(&conn, run_id, &mut file)?;
        }
    }
    Ok(Some(path.display().to_string()))
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
    launch(&state, gemini()?, run_id, on_progress).await?;
    runs::get(&*lock(&state)?, run_id)
}

/// Stop & Save Progress: like pause, then final; the report uses what was answered.
#[tauri::command]
pub async fn stop_run(state: State<'_, AppState>, run_id: i64) -> AppResult<SimulationRun> {
    if !signal(&state, run_id, Mode::Stop)? {
        // Not running (e.g. paused): stop it directly, then write the report's synthesis.
        let stopped = {
            let conn = lock(&state)?;
            let run = runs::get(&conn, run_id)?;
            let stop = matches!(run.status, RunStatus::Paused | RunStatus::Queued);
            if stop {
                runs::set_status(&conn, run_id, RunStatus::Stopped, None)?;
            }
            stop
        };
        if stopped {
            start_synthesis(&state, run_id, false).await?;
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
