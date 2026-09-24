//! Step 2 persistence: cohorts and their respondents.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::{json, Value};

use crate::engine::persona::{category_profile_schema, EnrichedPersona};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::model::{
    Cohort, CohortConfig, CohortStatus, CohortSummary, RespondentCard, RespondentDetail,
    RespondentPage, Share,
};
use crate::sampling::Skeleton;

/// Creates a cohort in `generating` state. `model` and `prompt_version` are stored with the
/// config so the cohort can be traced and reproduced.
pub fn create(
    conn: &Connection,
    project_id: i64,
    config: &CohortConfig,
    parent: Option<i64>,
    model: &str,
    prompt_version: &str,
) -> AppResult<Cohort> {
    let version: i64 = conn.query_row(
        "SELECT COUNT(*) + 1 FROM cohorts WHERE project_id = ?1",
        [project_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO cohorts(project_id, name, config_json, status, parent_cohort_id) VALUES (?1, ?2, ?3, 'generating', ?4)",
        params![
            project_id,
            format!("Cohort v{version}"),
            json!({ "config": config, "model": model, "promptVersion": prompt_version }).to_string(),
            parent
        ],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "UPDATE projects SET wizard_step = MAX(wizard_step, 2) WHERE id = ?1",
        [project_id],
    )?;
    get(conn, id)
}

pub fn get(conn: &Connection, id: i64) -> AppResult<Cohort> {
    conn.query_row(
        "SELECT id, project_id, name, status, config_json, created_at FROM cohorts WHERE id = ?1",
        [id],
        from_row,
    )
    .optional()?
    .ok_or_else(|| AppError::not_found(format!("cohort {id} not found")))
}

/// The newest cohort of a project, if any.
pub fn latest(conn: &Connection, project_id: i64) -> AppResult<Option<Cohort>> {
    Ok(conn
        .query_row(
            "SELECT id, project_id, name, status, config_json, created_at FROM cohorts
             WHERE project_id = ?1 ORDER BY id DESC LIMIT 1",
            [project_id],
            from_row,
        )
        .optional()?)
}

fn from_row(r: &Row<'_>) -> rusqlite::Result<Cohort> {
    let config_json: String = r.get(4)?;
    let raw: Value = serde_json::from_str(&config_json).unwrap_or(Value::Null);
    let error = raw.get("error").and_then(Value::as_str).map(str::to_string);
    let config: CohortConfig = serde_json::from_value(
        raw.get("config").cloned().unwrap_or(raw.clone()),
    )
    .unwrap_or(CohortConfig {
        size: 0,
        seed: 0,
        quotas: vec![],
        screening: String::new(),
        non_binary_share: 0,
        countries: vec![],
    });
    Ok(Cohort {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        status: CohortStatus::from_db(&r.get::<_, String>(3)?),
        config,
        error,
        created_at: r.get(5)?,
    })
}

pub fn set_status(
    conn: &Connection,
    id: i64,
    status: CohortStatus,
    error: Option<&str>,
) -> AppResult<()> {
    let stored: String =
        conn.query_row("SELECT config_json FROM cohorts WHERE id = ?1", [id], |r| {
            r.get(0)
        })?;
    let mut doc: Value = serde_json::from_str(&stored)?;
    if let Some(obj) = doc.as_object_mut() {
        match error {
            Some(e) => {
                obj.insert("error".into(), json!(e));
            }
            None => {
                obj.remove("error");
            }
        }
    }
    conn.execute(
        "UPDATE cohorts SET status = ?2, config_json = ?3 WHERE id = ?1",
        params![id, status.as_db(), doc.to_string()],
    )?;
    Ok(())
}

/// Proceed to Questionnaire: a ready cohort becomes locked (immutable).
pub fn lock(conn: &Connection, id: i64) -> AppResult<Cohort> {
    let c = get(conn, id)?;
    match c.status {
        CohortStatus::Locked => {}
        CohortStatus::Ready => {
            set_status(conn, id, CohortStatus::Locked, None)?;
            conn.execute(
                "UPDATE projects SET wizard_step = MAX(wizard_step, 3) WHERE id = ?1",
                [c.project_id],
            )?;
        }
        _ => {
            return Err(AppError::invalid(
                "the cohort must finish generating before it can be locked",
            ))
        }
    }
    get(conn, id)
}

/// Saves one enriched persona. `screen_status`: "passed", "failed" or "flagged".
pub fn insert_respondent(
    conn: &Connection,
    cohort_id: i64,
    s: &Skeleton,
    p: &EnrichedPersona,
    screen_status: &str,
) -> AppResult<()> {
    if get(conn, cohort_id)?.status == CohortStatus::Locked {
        return Err(AppError::new(
            ErrorCode::CohortLocked,
            "the cohort is locked",
        ));
    }
    let persona = json!({ "skeleton": {
        "country": s.country, "age_band": s.age_band, "region": s.region, "income": s.income, "occupation": s.occupation
    }, "persona": p });
    conn.execute(
        "INSERT INTO respondents(cohort_id, ordinal, quota_cell, display_name, age, gender, occupation, income_bracket,
            location, country, psychographic_summary, persona_json, category_profile_json, screen_status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            cohort_id,
            s.ordinal,
            s.quota_cell,
            p.name,
            s.age,
            s.gender,
            s.occupation,
            s.income,
            s.region,
            s.country,
            p.summary,
            persona.to_string(),
            Value::Object(p.category_profile.clone()).to_string(),
            screen_status
        ],
    )?;
    Ok(())
}

pub fn summary(
    conn: &Connection,
    cohort_id: i64,
    category: Option<&str>,
) -> AppResult<CohortSummary> {
    let status = get(conn, cohort_id)?.status;
    let kept = "screen_status <> 'failed'";
    let (respondents, average_age): (u32, Option<f64>) = conn.query_row(
        &format!("SELECT COUNT(*), AVG(age) FROM respondents WHERE cohort_id = ?1 AND {kept}"),
        [cohort_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let replaced: u32 = conn.query_row(
        "SELECT COUNT(*) FROM respondents WHERE cohort_id = ?1 AND screen_status = 'failed'",
        [cohort_id],
        |r| r.get(0),
    )?;
    let pct = |n: u32| {
        if respondents == 0 {
            0.0
        } else {
            (f64::from(n) * 1000.0 / f64::from(respondents)).round() / 10.0
        }
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT gender, COUNT(*) FROM respondents WHERE cohort_id = ?1 AND {kept} GROUP BY gender ORDER BY COUNT(*) DESC, gender"
    ))?;
    let gender = stmt
        .query_map([cohort_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(label, n)| Share {
            label,
            percent: pct(n),
        })
        .collect();
    let (_, field) = category_profile_schema(category);
    let top: Option<(String, u32)> = conn
        .query_row(
            &format!(
                "SELECT CAST(json_extract(category_profile_json, '$.' || ?2) AS TEXT) AS v, COUNT(*) FROM respondents
                 WHERE cohort_id = ?1 AND {kept} AND v IS NOT NULL GROUP BY v ORDER BY COUNT(*) DESC, v LIMIT 1"
            ),
            params![cohort_id, field],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(CohortSummary {
        status,
        respondents,
        average_age: average_age.map(|a| (a * 10.0).round() / 10.0),
        gender,
        top_trigger: top.map(|(label, n)| Share {
            label: humanise(&label),
            percent: pct(n),
        }),
        top_trigger_label: humanise(field),
        replaced_at_screening: replaced,
    })
}

pub fn list(
    conn: &Connection,
    cohort_id: i64,
    query: &str,
    offset: u32,
    limit: u32,
) -> AppResult<RespondentPage> {
    let like = format!("%{}%", query.trim().replace('%', "\\%").replace('_', "\\_"));
    let filter = "cohort_id = ?1 AND screen_status <> 'failed' AND (?2 = '%%' OR display_name LIKE ?2 ESCAPE '\\'
                  OR occupation LIKE ?2 ESCAPE '\\' OR json_extract(persona_json, '$.persona.biases') LIKE ?2 ESCAPE '\\')";
    let total: u32 = conn.query_row(
        &format!("SELECT COUNT(*) FROM respondents WHERE {filter}"),
        params![cohort_id, like],
        |r| r.get(0),
    )?;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, ordinal, display_name, age, gender, occupation, json_extract(persona_json, '$.persona.biases')
         FROM respondents WHERE {filter} ORDER BY ordinal LIMIT ?3 OFFSET ?4"
    ))?;
    let items = stmt
        .query_map(params![cohort_id, like, limit, offset], |r| {
            let biases: Option<String> = r.get(6)?;
            Ok(RespondentCard {
                id: r.get(0)?,
                ordinal: r.get(1)?,
                name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                age: r.get(3)?,
                gender: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                occupation: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                biases: biases
                    .and_then(|b| serde_json::from_str(&b).ok())
                    .unwrap_or_default(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RespondentPage { items, total })
}

pub fn detail(conn: &Connection, id: i64) -> AppResult<RespondentDetail> {
    let row = conn
        .query_row(
            "SELECT id, ordinal, age, gender, country, location, income_bracket, occupation, persona_json, screen_status
             FROM respondents WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, u32>(1)?,
                    r.get::<_, u32>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::not_found(format!("respondent {id} not found")))?;
    let doc: Value = serde_json::from_str(&row.8)?;
    let p: EnrichedPersona = serde_json::from_value(doc["persona"].clone())?;
    let category_facts = p
        .category_profile
        .iter()
        .map(|(k, v)| (humanise(k), humanise(v.to_string().trim_matches('"'))))
        .collect();
    Ok(RespondentDetail {
        id: row.0,
        ordinal: row.1,
        name: p.name,
        age: row.2,
        gender: row.3.unwrap_or_default(),
        country: row.4,
        region: row.5.unwrap_or_default(),
        income: row.6.unwrap_or_default(),
        occupation: row.7.unwrap_or_default(),
        summary: p.summary,
        values: p.values,
        habits: p.habits,
        media_habits: p.media_habits,
        brand_loyalties: p.brand_loyalties,
        category_attitudes: p.category_attitudes,
        price_sensitivity: p.price_sensitivity,
        biases: p.biases,
        category_facts,
        screen_status: row.9,
        screen_reason: p.screen_reason,
    })
}

/// "upgrade_trigger" → "Upgrade trigger"; "true" → "Yes".
fn humanise(s: &str) -> String {
    match s {
        "true" => return "Yes".into(),
        "false" => return "No".into(),
        _ => {}
    }
    let t = s.replace('_', " ");
    let mut c = t.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::QuotaGroup;

    fn config() -> CohortConfig {
        CohortConfig {
            size: 10,
            seed: 1,
            quotas: vec![QuotaGroup {
                key: "age".into(),
                label: "Age".into(),
                rows: vec![],
            }],
            screening: String::new(),
            non_binary_share: 5,
            countries: vec!["CA".into(), "US".into()],
        }
    }

    #[test]
    fn create_then_get_round_trips_the_config() {
        let conn = crate::db::open_in_memory();
        conn.execute("INSERT INTO projects(title) VALUES ('p')", [])
            .unwrap();
        let cfg = config();
        let c = create(&conn, 1, &cfg, None, "flash", "v1").unwrap();
        assert_eq!(c.config, cfg);
        assert_eq!(get(&conn, c.id).unwrap().config, cfg);
        assert_eq!(latest(&conn, 1).unwrap().unwrap().config, cfg);
    }

    /// DATA_FLOW §2: Step 2's "Proceed to Questionnaire" only locks a `ready` cohort.
    #[test]
    fn lock_refuses_before_ready() {
        let conn = crate::db::open_in_memory();
        conn.execute("INSERT INTO projects(title) VALUES ('p')", [])
            .unwrap();
        let c = create(&conn, 1, &config(), None, "flash", "v1").unwrap();
        assert_eq!(c.status, CohortStatus::Generating);
        assert!(lock(&conn, c.id).is_err());

        set_status(&conn, c.id, CohortStatus::Ready, None).unwrap();
        assert_eq!(lock(&conn, c.id).unwrap().status, CohortStatus::Locked);
        // Locking an already-locked cohort is a no-op, not an error.
        assert_eq!(lock(&conn, c.id).unwrap().status, CohortStatus::Locked);
    }
}
