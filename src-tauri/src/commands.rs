//! Tauri commands. Names and payloads follow docs/DATA_FLOW.md; types are generated into
//! `src/types/gen/` from `survey-core`.

use tauri::State;

use survey_core::llm::gemini::GeminiClient;
use survey_core::model::{CohortConfig, CountryOption, Project, SurveyInfo};
use survey_core::{db, sampling, AppError, AppResult, ErrorCode};

use crate::{keychain, AppState};

fn lock(state: &State<'_, AppState>) -> AppResult<std::sync::MutexGuard<'_, rusqlite::Connection>> {
    state
        .db
        .lock()
        .map_err(|_| AppError::new(ErrorCode::Internal, "database lock poisoned"))
}

#[tauri::command]
pub fn list_countries() -> Vec<CountryOption> {
    survey_core::countries::list().to_vec()
}

#[tauri::command]
pub fn save_survey_info(
    state: State<'_, AppState>,
    project_id: Option<i64>,
    info: SurveyInfo,
) -> AppResult<Project> {
    db::projects::save_survey_info(&lock(&state)?, project_id, &info)
}

#[tauri::command]
pub fn get_project(state: State<'_, AppState>, project_id: i64) -> AppResult<Project> {
    db::projects::get_project(&lock(&state)?, project_id)
}

/// Step 1 → Step 2. Validates the gate now; starting the persona and draft jobs lands in M2/M3.
#[tauri::command]
pub fn generate_cohort(
    state: State<'_, AppState>,
    project_id: i64,
    config: CohortConfig,
) -> AppResult<()> {
    let project = db::projects::get_project(&lock(&state)?, project_id)?;
    if project.research_type.is_none() {
        return Err(AppError::invalid("choose a research type first"));
    }
    if project.countries.is_empty() {
        return Err(AppError::invalid("choose at least one country"));
    }
    sampling::validate(&config)?;
    Err(AppError::not_implemented("Persona generation", "M2"))
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

/// Settings → "Test connection": lists the Gemini models the stored key can use.
#[tauri::command]
pub async fn test_connection() -> AppResult<Vec<String>> {
    let key = keychain::get()?.ok_or_else(|| AppError::invalid("no Gemini API key stored"))?;
    Ok(GeminiClient::new(key).list_models().await?)
}
