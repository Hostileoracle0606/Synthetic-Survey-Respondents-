//! Step 1 persistence: the Survey Info fields of a project.

use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::error::{AppError, AppResult};
use crate::model::{Project, ResearchType, SurveyInfo};

fn validate(info: &SurveyInfo) -> AppResult<()> {
    if info.title.trim().is_empty() {
        return Err(AppError::invalid("project title is required"));
    }
    for code in &info.countries {
        if crate::countries::find(code).is_none() {
            return Err(AppError::invalid(format!("unknown country code {code}")));
        }
    }
    Ok(())
}

/// Creates the project on first save (`id = None`), otherwise updates it.
pub fn save_survey_info(
    conn: &Connection,
    id: Option<i64>,
    info: &SurveyInfo,
) -> AppResult<Project> {
    validate(info)?;
    let countries = serde_json::to_string(&info.countries)?;
    let research_type = info.research_type.map(ResearchType::as_db);
    let id = match id {
        None => {
            conn.execute(
                "INSERT INTO projects(title, research_type, product_category, countries_json, research_goal)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![info.title.trim(), research_type, info.product_category, countries, info.research_goal],
            )?;
            conn.last_insert_rowid()
        }
        Some(id) => {
            let changed = conn.execute(
                "UPDATE projects SET title = ?2, research_type = ?3, product_category = ?4, countries_json = ?5,
                   research_goal = ?6, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 WHERE id = ?1",
                params![id, info.title.trim(), research_type, info.product_category, countries, info.research_goal],
            )?;
            if changed == 0 {
                return Err(AppError::not_found(format!("project {id} not found")));
            }
            id
        }
    };
    get_project(conn, id)
}

pub fn get_project(conn: &Connection, id: i64) -> AppResult<Project> {
    conn.query_row(
        "SELECT id, title, research_type, product_category, countries_json, research_goal, wizard_step,
                created_at, updated_at
         FROM projects WHERE id = ?1",
        [id],
        from_row,
    )
    .optional()?
    .ok_or_else(|| AppError::not_found(format!("project {id} not found")))
}

/// The most recently updated project, if any. Used to reopen the app on the last project.
pub fn latest(conn: &Connection) -> AppResult<Option<Project>> {
    Ok(conn
        .query_row(
            "SELECT id, title, research_type, product_category, countries_json, research_goal, wizard_step,
                    created_at, updated_at
             FROM projects ORDER BY updated_at DESC, id DESC LIMIT 1",
            [],
            from_row,
        )
        .optional()?)
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<Project> {
    let research_type: Option<String> = r.get(2)?;
    let countries_json: String = r.get(4)?;
    Ok(Project {
        id: r.get(0)?,
        title: r.get(1)?,
        research_type: research_type.as_deref().and_then(ResearchType::from_db),
        product_category: r.get(3)?,
        countries: serde_json::from_str(&countries_json).unwrap_or_default(),
        research_goal: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        wizard_step: r.get(6)?,
        created_at: r.get(7)?,
        updated_at: r.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> SurveyInfo {
        SurveyInfo {
            title: "US Smartphone Upgrade Intent Study".into(),
            research_type: Some(ResearchType::MarketResponse),
            product_category: Some("mobile_phone".into()),
            countries: vec!["US".into()],
            research_goal: "Understand upgrade drivers".into(),
        }
    }

    #[test]
    fn create_then_update_round_trips() {
        let conn = crate::db::open_in_memory();
        let p = save_survey_info(&conn, None, &info()).unwrap();
        assert_eq!(p.research_type, Some(ResearchType::MarketResponse));
        assert_eq!(p.countries, vec!["US"]);
        assert_eq!(p.wizard_step, 1);

        let mut changed = info();
        changed.countries.push("CA".into());
        changed.research_type = None;
        let p2 = save_survey_info(&conn, Some(p.id), &changed).unwrap();
        assert_eq!(p2.id, p.id);
        assert_eq!(p2.countries, vec!["US", "CA"]);
        assert_eq!(p2.research_type, None);
    }

    #[test]
    fn latest_returns_the_most_recently_updated_project() {
        // strftime('%f') has millisecond resolution, so a short sleep between writes keeps
        // their `updated_at` values distinct and the ordering deterministic.
        let step = || std::thread::sleep(std::time::Duration::from_millis(5));

        let conn = crate::db::open_in_memory();
        assert!(latest(&conn).unwrap().is_none());
        let p1 = save_survey_info(&conn, None, &info()).unwrap();
        assert_eq!(latest(&conn).unwrap().unwrap().id, p1.id);
        step();
        let mut other = info();
        other.title = "TV Purchase Study".into();
        let p2 = save_survey_info(&conn, None, &other).unwrap();
        assert_eq!(latest(&conn).unwrap().unwrap().id, p2.id);
        step();
        save_survey_info(&conn, Some(p1.id), &info()).unwrap();
        assert_eq!(latest(&conn).unwrap().unwrap().id, p1.id);
    }

    #[test]
    fn rejects_blank_title_unknown_country_and_missing_project() {
        let conn = crate::db::open_in_memory();
        let mut bad = info();
        bad.title = "  ".into();
        assert!(save_survey_info(&conn, None, &bad).is_err());
        let mut bad = info();
        bad.countries = vec!["XX".into()];
        assert!(save_survey_info(&conn, None, &bad).is_err());
        assert_eq!(
            save_survey_info(&conn, Some(99), &info()).unwrap_err().code,
            crate::ErrorCode::NotFound
        );
    }
}
