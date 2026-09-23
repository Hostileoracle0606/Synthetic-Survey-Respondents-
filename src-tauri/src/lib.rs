//! Tauri shell: opens the database, registers commands, starts the window.
//! All logic lives in `survey-core`; commands here only validate, delegate and map errors.

mod commands;
mod keychain;

use std::sync::Mutex;

use tauri::Manager;

pub struct AppState {
    /// Single writer connection for now; background jobs will move to
    /// `survey_core::db::writer::Writer` in M2 (docs/IMPLEMENTATION_PLAN.md).
    pub db: Mutex<rusqlite::Connection>,
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let conn = survey_core::db::open(&dir.join("data.db"))?;
            app.manage(AppState {
                db: Mutex::new(conn),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_countries,
            commands::save_survey_info,
            commands::get_project,
            commands::generate_cohort,
            commands::set_api_key,
            commands::has_api_key,
            commands::delete_api_key,
            commands::test_connection,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Synthetic Survey app");
}
