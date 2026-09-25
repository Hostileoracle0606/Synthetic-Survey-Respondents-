//! Creates an old-schema database and verifies that a packaged app migrated it without losing
//! its marker data. Used by `scripts/release/upgrade-keeps-data.ps1` (TEST_PLAN S14).
//!
//!   release-db seed-v1 --db <data.db> --marker <text>
//!   release-db verify --db <data.db> --marker <text>

use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use survey_evals::flag;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("seed-v1") => seed_v1(&args[1..]),
        Some("verify") => verify(&args[1..]),
        _ => Err("usage: release-db <seed-v1|verify> --db <data.db> --marker <text>".into()),
    };
    if let Err(error) = result {
        eprintln!("release-db: {error}");
        std::process::exit(1);
    }
}

fn required(args: &[String], name: &str) -> Result<String, String> {
    flag(args, name).ok_or_else(|| format!("{name} is required"))
}

fn seed_v1(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(required(args, "--db")?);
    let marker = required(args, "--marker")?;
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }

    let mut connection = Connection::open(&path).map_err(|error| error.to_string())?;
    survey_core::db::configure(&connection).map_err(|error| error.to_string())?;
    survey_core::db::migrations()
        .to_version(&mut connection, 1)
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO projects(title, research_type, product_category, countries_json, research_goal, wizard_step)
             VALUES (?1, 'market_response', 'mobile_phone', '[\"CA\"]', 'Upgrade preservation fixture', 3)",
            [&marker],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO surveys(project_id, title, intro_text, status)
             VALUES (1, 'Upgrade fixture survey', 'This text must survive the upgrade.', 'in_review')",
            [],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO questions(survey_id, order_index, code, question_text, question_type, review_status)
             VALUES (1, 1, 'Q1_KEEP', 'Did this question survive?', 'likert', 'accepted')",
            [],
        )
        .map_err(|error| error.to_string())?;
    println!("seeded schema v1 database at {}", path.display());
    Ok(())
}

fn verify(args: &[String]) -> Result<(), String> {
    let path = PathBuf::from(required(args, "--db")?);
    let marker = required(args, "--marker")?;
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| error.to_string())?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|error| error.to_string())?;

    let pending = survey_core::db::migrations()
        .pending_migrations(&connection)
        .map_err(|error| error.to_string())?;
    if pending != 0 {
        return Err(format!("the packaged app left {pending} pending migrations"));
    }

    let saved: String = connection
        .query_row("SELECT title FROM projects WHERE id = 1", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if saved != marker {
        return Err(format!("project marker changed: expected {marker:?}, got {saved:?}"));
    }
    let question: String = connection
        .query_row("SELECT question_text FROM questions WHERE code = 'Q1_KEEP'", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if question != "Did this question survive?" {
        return Err("the fixture question changed during upgrade".into());
    }
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if integrity != "ok" {
        return Err(format!("SQLite integrity_check returned {integrity}"));
    }
    let foreign_key_errors: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if foreign_key_errors != 0 {
        return Err(format!("foreign_key_check found {foreign_key_errors} errors"));
    }
    println!("schema and marker data survived the packaged-app upgrade");
    Ok(())
}
