//! Tauri commands. Names and payloads follow docs/DATA_FLOW.md; types are generated into
//! `src/types/gen/` from `survey-core`.

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

use survey_core::db::{cohorts, projects};
use survey_core::engine::cohort::{self, CohortJob};
use survey_core::engine::persona::PROMPT_VERSION;
use survey_core::llm::gemini::{newest_stable_flash, GeminiClient};
use survey_core::llm::LlmProvider;
use survey_core::model::{
    Cohort, CohortConfig, CohortProgress, CohortSummary, CountryOption, Project, QuotaGroup,
    RespondentDetail, RespondentPage, SurveyInfo,
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

async fn flash_model(state: &State<'_, AppState>, client: &GeminiClient) -> AppResult<String> {
    let mut cached = state.flash_model.lock().await;
    if let Some(m) = cached.as_ref() {
        return Ok(m.clone());
    }
    let models = client.list_models().await?;
    let m = newest_stable_flash(&models).ok_or_else(|| {
        AppError::new(
            ErrorCode::Llm,
            "no stable Gemini Flash model is available to this key",
        )
    })?;
    *cached = Some(m.clone());
    Ok(m)
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
    start_cohort(&state, project_id, config, None, on_progress).await
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
