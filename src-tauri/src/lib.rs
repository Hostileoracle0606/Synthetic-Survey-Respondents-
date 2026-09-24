//! Tauri shell: opens the database, registers commands, starts the window.
//! All logic lives in `survey-core`; commands here only validate, delegate and map errors.

mod commands;
mod keychain;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use survey_core::db::writer::Writer;
use survey_core::engine::limiter::{Limits, RateLimiter};
use survey_core::engine::run::Mode;
use tauri::Manager;

pub struct AppState {
    /// Connection for command reads and small writes. Background jobs write through `writer`.
    pub db: Mutex<rusqlite::Connection>,
    pub db_path: PathBuf,
    pub writer: Writer,
    /// One limiter for every Gemini call (limits apply per Google Cloud project).
    pub limiter: Arc<RateLimiter>,
    /// Models the key can use, from `list_models` on first use.
    pub models: tokio::sync::Mutex<Option<Vec<String>>>,
    /// Pause/Stop switches for the simulation runs in progress, by run id.
    pub runs: Arc<Mutex<HashMap<i64, tokio::sync::watch::Sender<Mode>>>>,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("data.db");
            let conn = survey_core::db::open(&path)?;
            // Runs left running by a crash or a closed window show as paused and can be resumed.
            survey_core::db::runs::recover_on_launch(&conn)?;
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
                models: tokio::sync::Mutex::new(None),
                runs: Arc::new(Mutex::new(HashMap::new())),
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
            commands::get_survey,
            commands::redraft_survey,
            commands::update_survey_text,
            commands::update_question,
            commands::reorder_questions,
            commands::add_question,
            commands::delete_question,
            commands::approve_question,
            commands::add_suggestion,
            commands::start_simulation,
            commands::get_latest_run,
            commands::pause_run,
            commands::resume_run,
            commands::stop_run,
            commands::get_report,
            commands::get_crosstab,
            commands::regenerate_synthesis,
            commands::export_run,
            commands::set_api_key,
            commands::has_api_key,
            commands::delete_api_key,
            commands::test_connection,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Synthetic Survey app");
}
