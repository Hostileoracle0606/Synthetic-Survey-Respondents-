//! Offline exports (docs/DATA_FLOW.md §3, Step 5). Both are built in memory and written by
//! the app to a path the user picks; nothing is uploaded. The API key never appears: it is
//! only ever in the OS keychain, never in the database these come from.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::{load, synthesis, Person};
use crate::db::{cohorts, projects, runs, surveys};
use crate::error::AppResult;
use crate::model::QuestionType;

/// Indented JSON for the export file.
pub fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// One CSV cell. Quotes when needed; text that Excel would run as a formula gets a leading
/// apostrophe (AI-written answers must never execute in a spreadsheet).
fn cell(s: &str, is_text: bool) -> String {
    let guarded = if is_text && s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if guarded.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

fn num(v: Option<f64>) -> String {
    v.map(|x| {
        if x.fract() == 0.0 {
            format!("{}", x as i64)
        } else {
            format!("{x}")
        }
    })
    .unwrap_or_default()
}

/// One row per respondent with saved answers: ID, demographics, then one column per question
/// code (multiple choice: one 0/1 column per option). UTF-8 with BOM and CRLF, for Excel.
/// Invalid and refused answers are left empty.
pub fn csv(conn: &Connection, run_id: i64) -> AppResult<String> {
    let data = load(conn, run_id)?;
    let mut header: Vec<String> = [
        "respondent",
        "name",
        "age",
        "gender",
        "country",
        "region",
        "household_income",
        "occupation",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for q in &data.questions {
        if q.body.question_type == QuestionType::MultiChoice {
            header.extend(
                q.body
                    .options
                    .iter()
                    .map(|o| format!("{}_{}", q.code, o.code)),
            );
        } else {
            header.push(q.code.clone());
        }
    }
    let mut out = String::from("\u{feff}");
    out.push_str(
        &header
            .iter()
            .map(|h| cell(h, true))
            .collect::<Vec<_>>()
            .join(","),
    );
    out.push_str("\r\n");

    let mut ids: Vec<i64> = data.answers.iter().map(|a| a.respondent_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut ids: Vec<(u32, i64)> = ids
        .into_iter()
        .map(|id| (data.people.get(&id).map_or(0, |p| p.ordinal), id))
        .collect();
    ids.sort_unstable();
    for (_, id) in ids {
        let p = data.people.get(&id).cloned().unwrap_or_default();
        let Person {
            ordinal,
            name,
            age,
            gender,
            country,
            region,
            income,
            occupation,
        } = p;
        let mut row = vec![
            ordinal.to_string(),
            cell(&name, true),
            age.to_string(),
            cell(&gender, true),
            cell(&country, true),
            cell(&region, true),
            cell(&income, true),
            cell(&occupation, true),
        ];
        for q in &data.questions {
            let a = data
                .answers
                .iter()
                .find(|a| a.respondent_id == id && a.question_id == q.id && a.status == "valid");
            match q.body.question_type {
                QuestionType::SingleChoice => {
                    let label = a
                        .and_then(|a| a.answer["code"].as_str())
                        .and_then(|c| q.body.options.iter().find(|o| o.code == c))
                        .map(|o| o.label.as_str())
                        .unwrap_or_default();
                    row.push(cell(label, true));
                }
                QuestionType::MultiChoice => {
                    let chosen: Vec<&str> = a
                        .and_then(|a| a.answer["codes"].as_array())
                        .map(|v| v.iter().filter_map(|c| c.as_str()).collect())
                        .unwrap_or_default();
                    for o in &q.body.options {
                        row.push(match a {
                            Some(_) => if chosen.contains(&o.code.as_str()) {
                                "1"
                            } else {
                                "0"
                            }
                            .to_string(),
                            None => String::new(),
                        });
                    }
                }
                QuestionType::Likert | QuestionType::Numeric => {
                    row.push(num(a.and_then(|a| a.answer["value"].as_f64())))
                }
                QuestionType::OpenEnded => row.push(cell(
                    a.and_then(|a| a.answer["text"].as_str())
                        .unwrap_or_default(),
                    true,
                )),
            }
        }
        out.push_str(&row.join(","));
        out.push_str("\r\n");
    }
    Ok(out)
}

/// Everything needed to audit or reproduce the run: project, cohort (config and people),
/// survey (questions and review history), run settings, every answer with its reason, the
/// theme coding and the synthesis.
pub fn json(conn: &Connection, run_id: i64) -> AppResult<Value> {
    let run = runs::get(conn, run_id)?;
    let project = projects::get_project(conn, run.project_id)?;
    let cohort = cohorts::get(conn, run.cohort_id)?;
    let settings: Value = conn.query_row(
        "SELECT survey_hash, provider, model, temperature, answer_mode, prompt_version, seed, max_concurrency, status, started_at, finished_at
         FROM simulation_runs WHERE id = ?1",
        [run_id],
        |r| {
            Ok(json!({
                "surveyHash": r.get::<_, String>(0)?, "provider": r.get::<_, String>(1)?, "model": r.get::<_, String>(2)?,
                "temperature": r.get::<_, f64>(3)?, "answerMode": r.get::<_, String>(4)?, "promptVersion": r.get::<_, String>(5)?,
                "seed": r.get::<_, i64>(6)?, "maxConcurrency": r.get::<_, i64>(7)?, "status": r.get::<_, String>(8)?,
                "startedAt": r.get::<_, Option<String>>(9)?, "finishedAt": r.get::<_, Option<String>>(10)?,
            }))
        },
    )?;
    let respondents: Vec<Value> = conn
        .prepare("SELECT id FROM respondents WHERE cohort_id = ?1 ORDER BY ordinal")?
        .query_map([run.cohort_id], |r| r.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|id| cohorts::detail(conn, id).map(|d| serde_json::to_value(d).unwrap_or_default()))
        .collect::<AppResult<_>>()?;
    let survey = surveys::get(conn, run.survey_id)?;
    let history: Vec<Value> = conn
        .prepare(
            "SELECT id, code, origin, review_status, reviewed_at, original_json, objective, rationale FROM questions WHERE survey_id = ?1 ORDER BY order_index",
        )?
        .query_map([run.survey_id], |r| {
            let original: Option<String> = r.get(5)?;
            Ok(json!({
                "questionId": r.get::<_, i64>(0)?, "code": r.get::<_, String>(1)?, "origin": r.get::<_, String>(2)?,
                "reviewStatus": r.get::<_, String>(3)?, "reviewedAt": r.get::<_, Option<String>>(4)?,
                "aiOriginal": original.and_then(|o| serde_json::from_str::<Value>(&o).ok()),
                "objective": r.get::<_, Option<String>>(6)?, "rationale": r.get::<_, Option<String>>(7)?,
            }))
        })?
        .collect::<Result<_, _>>()?;
    let answers: Vec<Value> = conn
        .prepare(
            "SELECT p.ordinal, q.code, x.status, x.answer_json, x.reasoning, x.shown_options_json
             FROM responses x JOIN respondents p ON p.id = x.respondent_id JOIN questions q ON q.id = x.question_id
             WHERE x.run_id = ?1 ORDER BY p.ordinal, q.order_index",
        )?
        .query_map([run_id], |r| {
            let parse = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok());
            Ok(json!({
                "respondent": r.get::<_, u32>(0)?, "question": r.get::<_, String>(1)?, "status": r.get::<_, String>(2)?,
                "answer": parse(r.get(3)?), "reason": r.get::<_, Option<String>>(4)?, "shownOptions": parse(r.get(5)?),
            }))
        })?
        .collect::<Result<_, _>>()?;
    let themes: Vec<Value> = conn
        .prepare(
            "SELECT t.id, q.code, t.label, t.description,
                (SELECT COUNT(*) FROM response_themes rt WHERE rt.theme_id = t.id)
             FROM themes t JOIN questions q ON q.id = t.question_id WHERE t.run_id = ?1 ORDER BY t.id",
        )?
        .query_map(params![run_id], |r| {
            Ok(json!({ "id": r.get::<_, i64>(0)?, "question": r.get::<_, String>(1)?, "label": r.get::<_, String>(2)?,
                "description": r.get::<_, Option<String>>(3)?, "count": r.get::<_, i64>(4)? }))
        })?
        .collect::<Result<_, _>>()?;
    Ok(json!({
        "format": "synthetic-survey-export/1",
        "disclosure": format!(
            "Synthetic respondents — directional only; not calibrated against real survey data. Every answer was written by an AI model playing a persona; these are not real people. Model {}, prompt {}.",
            run.model, run.prompt_version
        ),
        "project": project,
        "cohort": { "id": cohort.id, "name": cohort.name, "config": cohort.config, "respondents": respondents },
        "survey": { "id": survey.id, "title": survey.title, "intro": survey.intro, "questions": survey.questions, "review": history },
        "run": { "id": run_id, "settings": settings, "respondentsDone": run.respondents_done },
        "answers": answers,
        "themes": themes,
        "synthesis": synthesis(conn, run_id)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::fixture::Fixture;
    use super::*;

    #[test]
    fn csv_has_bom_crlf_one_row_per_respondent_and_0_1_columns() {
        let f = Fixture::build();
        let text = csv(&f.conn, f.run_id).unwrap();
        assert!(text.starts_with('\u{feff}'));
        let lines: Vec<&str> = text
            .trim_start_matches('\u{feff}')
            .split("\r\n")
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(lines.len(), 41);
        assert_eq!(
            lines[0],
            "respondent,name,age,gender,country,region,household_income,occupation,BRAND,DRIVERS_A,DRIVERS_B,DRIVERS_C,INTENT,BUDGET,WHY"
        );
        assert_eq!(
            lines[1],
            "1,Person 1,25,Female,Canada,Ontario,Under $50k,Service,Brand one,1,1,0,1,400,Answer 1"
        );
        // Invalid and refused answers are empty; every row has the same number of cells.
        assert!(lines[39].starts_with("39,Person 39,70,Male,Canada,Ontario,Under $50k,Service,,"));
        assert!(lines.iter().all(|l| l.split(',').count() == 15));
    }

    #[test]
    fn csv_cells_are_quoted_and_formulas_neutralised() {
        assert_eq!(cell("plain", true), "plain");
        assert_eq!(cell("a, b", true), "\"a, b\"");
        assert_eq!(cell("say \"hi\"", true), "\"say \"\"hi\"\"\"");
        assert_eq!(cell("line\nbreak", true), "\"line\nbreak\"");
        assert_eq!(
            cell("=HYPERLINK(\"x\")", true),
            "\"'=HYPERLINK(\"\"x\"\")\""
        );
        assert_eq!(cell("-5", false), "-5");
        assert_eq!(cell("@SUM(A1)", true), "'@SUM(A1)");
    }

    #[test]
    fn json_export_holds_everything_needed_to_audit_the_run() {
        let f = Fixture::build();
        let v = json(&f.conn, f.run_id).unwrap();
        assert_eq!(v["answers"].as_array().unwrap().len(), 200);
        assert_eq!(v["cohort"]["respondents"].as_array().unwrap().len(), 40);
        assert_eq!(v["survey"]["questions"].as_array().unwrap().len(), 5);
        assert_eq!(v["run"]["settings"]["seed"], 7);
        assert_eq!(v["themes"][0]["count"], 24);
        assert!(v["disclosure"]
            .as_str()
            .unwrap()
            .contains("not real people"));
    }
}
