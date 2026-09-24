//! SQLite storage. The schema lives in `docs/schema.sql` and is the first migration; later
//! changes are the numbered files in `docs/migrations/`, applied in order.
//! Every connection gets the same PRAGMAs; all writes go through [`writer::Writer`].

pub mod cohorts;
pub mod projects;
pub mod runs;
pub mod settings;
pub mod surveys;
pub mod writer;

use std::path::Path;
use std::sync::LazyLock;

use rusqlite::Connection;
use rusqlite_migration::{Migrations, M};

use crate::error::AppResult;

const SCHEMA_SQL: &str = include_str!("../../../../docs/schema.sql");

/// Applied to every connection, reader or writer. WAL mode can't be set inside the
/// migration transaction, so these lines are stripped from the schema and run here.
pub const CONNECTION_PRAGMAS: &str =
    "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000; PRAGMA synchronous = NORMAL;";

static SCHEMA_BODY: LazyLock<String> = LazyLock::new(|| {
    SCHEMA_SQL
        .lines()
        .filter(|l| !l.trim_start().to_ascii_uppercase().starts_with("PRAGMA"))
        .collect::<Vec<_>>()
        .join("\n")
});

/// Changes after the first release of the schema, in order. Never edit one once released.
const LATER: &[&str] = &[
    include_str!("../../../../docs/migrations/002_survey_text_edited.sql"),
    include_str!("../../../../docs/migrations/003_question_critique.sql"),
];

pub fn migrations() -> Migrations<'static> {
    let mut all = vec![M::up(SCHEMA_BODY.as_str())];
    all.extend(LATER.iter().map(|sql| M::up(sql)));
    Migrations::new(all)
}

pub fn configure(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(CONNECTION_PRAGMAS)?;
    Ok(())
}

/// Opens (or creates) the database file, applies PRAGMAs and runs pending migrations.
pub fn open(path: &Path) -> AppResult<Connection> {
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    migrations().to_latest(&mut conn)?;
    Ok(conn)
}

/// Read-only connection for queries that run alongside the writer.
pub fn open_reader(path: &Path) -> AppResult<Connection> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
    Ok(conn)
}

#[cfg(test)]
pub(crate) fn open_in_memory() -> Connection {
    let mut conn = Connection::open_in_memory().unwrap();
    configure(&conn).unwrap();
    migrations().to_latest(&mut conn).unwrap();
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid() {
        migrations().validate().unwrap();
    }

    #[test]
    fn fresh_file_database_is_consistent_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let conn = open(&path).unwrap();
        let fk_problems: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(fk_problems, 0);
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        drop(conn);
        // Opening again runs no migration and keeps the same version.
        let conn = open(&path).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1 + LATER.len() as i64);
    }

    #[test]
    fn pragmas_apply_on_writer_and_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let writer = open(&path).unwrap();
        let mode: String = writer
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let reader = open_reader(&path).unwrap();
        let fk: i64 = reader
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[test]
    fn run_on_unapproved_survey_is_refused() {
        let conn = open_in_memory();
        conn.execute_batch(
            "INSERT INTO projects(title) VALUES ('p');
             INSERT INTO cohorts(project_id, name, config_json, status) VALUES (1, 'v1', '{}', 'locked');
             INSERT INTO surveys(project_id, title, status) VALUES (1, 's', 'in_review');
             INSERT INTO questions(survey_id, order_index, code, question_text, question_type, review_status)
               VALUES (1, 1, 'Q1', '?', 'likert', 'pending');",
        )
        .unwrap();
        let insert_run = "INSERT INTO simulation_runs(project_id, survey_id, cohort_id, survey_hash, provider, model,
            temperature, answer_mode, prompt_version, seed, max_concurrency)
            VALUES (1, 1, 1, 'h', 'gemini', 'm', 1.0, 'whole_survey', 'answer.v1', 1, 10)";
        let err = crate::AppError::from(conn.execute(insert_run, []).unwrap_err());
        assert_eq!(err.code, crate::ErrorCode::SurveyNotApproved);

        conn.execute_batch("UPDATE questions SET review_status = 'accepted'; UPDATE surveys SET status = 'approved';").unwrap();
        conn.execute(insert_run, []).unwrap();
    }
}
