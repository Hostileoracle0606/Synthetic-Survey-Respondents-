//! Tauri shell: opens the database, registers commands, starts the window.
//! All logic lives in `survey-core`; commands here only validate, delegate and map errors.

mod commands;
mod keychain;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use survey_core::db::writer::Writer;
use survey_core::engine::limiter::{Limits, RateLimiter};
use tauri::Manager;

pub struct AppState {
    /// Connection for command reads and small writes. Background jobs write through `writer`.
    pub db: Mutex<rusqlite::Connection>,
    pub db_path: PathBuf,
    pub writer: Writer,
    /// One limiter for every Gemini call (limits apply per Google Cloud project).
    pub limiter: Arc<RateLimiter>,
    /// Flash model picked from `list_models` on first use.
    pub flash_model: tokio::sync::Mutex<Option<String>>,
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("data.db");
            let conn = survey_core::db::open(&path)?;
            // Requests already made today (approximate Pacific-time day), so restarts keep the daily budget.
            let used_today: u32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM llm_calls WHERE created_at >= strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '-8 hours', 'start of day', '+8 hours')",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            let (writer, _handle) = Writer::spawn(survey_core::db::open(&path)?, 50, Duration::from_millis(250));
            app.manage(AppState {
                db: Mutex::new(conn),
                db_path: path,
                writer,
                limiter: Arc::new(RateLimiter::new(Limits::default(), used_today)),
                flash_model: tokio::sync::Mutex::new(None),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_countries,
            commands::default_quotas,
            commands::save_survey_info,
            commands::get_project,
            commands::generate_cohort,
            commands::regenerate_cohort,
            commands::get_latest_cohort,
            commands::get_cohort_summary,
            commands::list_respondents,
            commands::get_respondent,
            commands::lock_cohort,
            commands::set_api_key,
            commands::has_api_key,
            commands::delete_api_key,
            commands::test_connection,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Synthetic Survey app");
}
