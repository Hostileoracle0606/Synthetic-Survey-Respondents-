//! Golden fixture for report tests: 40 respondents with hand-set answers.
//!
//! - Respondents 1–19 are women, 20–40 men. Ages: 1–10 → 25, 11–20 → 35, 21–30 → 50,
//!   31–40 → 70. Odd ordinals earn "Under $50k", even "$50k–$100k". All in Ontario, Canada.
//! - BRAND (single A/B/C): 1–19 A, 20–31 B, 32–38 C; 39 invalid, 40 refused →
//!   n 38: A 19 (50.0%), B 12 (31.6%), C 7 (18.4%).
//! - DRIVERS (multi A/B/C): A for 1–30, B for 1–20, C for 31–40 → 75%, 50%, 25%.
//! - INTENT (likert 1–5): ((ordinal − 1) mod 5) + 1 → 8 of each point, mean 3.0.
//! - BUDGET (numeric 0–2000 CAD): 1–11 → 400, 12–20 → 600, 21 → 700, 22–31 → 900,
//!   32–40 → 1500 → median 650, Q1 400, Q3 900.
//! - WHY (open): themes "Price" on 1–24 (60%), "Battery is fine" on 25–34 (25%).

use rusqlite::{params, Connection};
use serde_json::json;

use crate::db::surveys::{self, NewQuestion};
use crate::model::{ChoiceOption, NumericRange, QuestionBody, QuestionType, Scale};

pub struct Fixture {
    pub conn: Connection,
    pub run_id: i64,
}

fn body(text: &str, t: QuestionType) -> QuestionBody {
    QuestionBody {
        text: text.into(),
        question_type: t,
        options: ["Brand one", "Brand two", "Brand three"]
            .iter()
            .enumerate()
            .map(|(i, l)| ChoiceOption {
                code: surveys::option_code(i),
                label: l.to_string(),
            })
            .collect(),
        randomize: true,
        max_choices: Some(3),
        scale: Some(Scale {
            min: 1,
            max: 5,
            min_label: "Not likely".into(),
            max_label: "Very likely".into(),
        }),
        numeric: Some(NumericRange {
            min: 0.0,
            max: 2000.0,
            unit: "CAD".into(),
        }),
    }
}

impl Fixture {
    pub fn q(&self, code: &str) -> i64 {
        self.conn
            .query_row("SELECT id FROM questions WHERE code = ?1", [code], |r| {
                r.get(0)
            })
            .unwrap()
    }

    pub fn build() -> Self {
        Self::build_on(crate::db::open_in_memory())
    }

    /// Builds the fixture in `conn` (a file database when a test needs the writer too).
    pub fn build_on(conn: Connection) -> Self {
        conn.execute_batch(
            "INSERT INTO projects(title, research_goal) VALUES ('Fixture', 'Find what drives upgrades');
             INSERT INTO cohorts(project_id, name, config_json, status) VALUES (1, 'Cohort v1', '{\"config\":{\"size\":40,\"seed\":1,\"quotas\":[],\"screening\":\"\"}}', 'locked');",
        )
        .unwrap();
        for o in 1..=40u32 {
            let age = [25, 35, 50, 70][((o - 1) / 10) as usize];
            conn.execute(
                "INSERT INTO respondents(cohort_id, ordinal, quota_cell, display_name, age, gender, occupation, income_bracket, location, country, psychographic_summary, persona_json)
                 VALUES (1, ?1, '', ?2, ?3, ?4, 'Service', ?5, 'Ontario', 'CA', 'Summary', ?6)",
                params![
                    o,
                    format!("Person {o}"),
                    age,
                    if o <= 19 { "Female" } else { "Male" },
                    if o % 2 == 1 { "Under $50k" } else { "$50k–$100k" },
                    json!({ "skeleton": {}, "persona": {
                        "ordinal": o, "name": format!("Person {o}"), "summary": "Summary", "values": ["Family"],
                        "habits": "", "media_habits": "", "brand_loyalties": "", "category_attitudes": "",
                        "price_sensitivity": 3, "biases": ["Status quo"], "category_profile": {},
                        "passes_screen": true, "screen_reason": ""
                    }})
                    .to_string()
                ],
            )
            .unwrap();
        }
        let survey = surveys::for_project(&conn, 1).unwrap();
        for (code, t) in [
            ("BRAND", QuestionType::SingleChoice),
            ("DRIVERS", QuestionType::MultiChoice),
            ("INTENT", QuestionType::Likert),
            ("BUDGET", QuestionType::Numeric),
            ("WHY", QuestionType::OpenEnded),
        ] {
            let id = surveys::insert_ai(
                &conn,
                survey.id,
                &NewQuestion {
                    code: code.into(),
                    body: body(&format!("{code}?"), t),
                    objective: None,
                    rationale: None,
                },
                false,
            )
            .unwrap();
            surveys::approve_question(&conn, id).unwrap();
        }
        conn.execute("UPDATE surveys SET status = 'approved'", [])
            .unwrap();
        conn.execute(
            "INSERT INTO simulation_runs(project_id, survey_id, cohort_id, survey_hash, provider, model, temperature, answer_mode, prompt_version, seed, max_concurrency, status)
             VALUES (1, ?1, 1, 'h', 'gemini', 'flash', 1.0, 'whole_survey', 'answer.v1', 7, 4, 'completed')",
            [survey.id],
        )
        .unwrap();
        let run_id = conn.last_insert_rowid();
        let f = Fixture { conn, run_id };
        let (brand, drivers, intent, budget, why) = (
            f.q("BRAND"),
            f.q("DRIVERS"),
            f.q("INTENT"),
            f.q("BUDGET"),
            f.q("WHY"),
        );
        let insert = |q: i64, o: u32, status: &str, answer: Option<serde_json::Value>| {
            f.conn
                .execute(
                    "INSERT INTO responses(run_id, question_id, respondent_id, answer_json, answer_code, answer_value, reasoning, status)
                     VALUES (?1, ?2, (SELECT id FROM respondents WHERE ordinal = ?3), ?4, ?5, ?6, 'because', ?7)",
                    params![
                        run_id,
                        q,
                        o,
                        answer.as_ref().map(|a| a.to_string()),
                        answer.as_ref().and_then(|a| a["code"].as_str().map(str::to_string)),
                        answer.as_ref().and_then(|a| a["value"].as_f64()),
                        status
                    ],
                )
                .unwrap();
            f.conn.last_insert_rowid()
        };
        let mut why_ids = Vec::new();
        for o in 1..=40u32 {
            match o {
                39 => insert(brand, o, "invalid", None),
                40 => insert(brand, o, "refused", None),
                _ => insert(
                    brand,
                    o,
                    "valid",
                    Some(
                        json!({ "code": if o <= 19 { "A" } else if o <= 31 { "B" } else { "C" } }),
                    ),
                ),
            };
            let mut codes = Vec::new();
            if o <= 30 {
                codes.push("A");
            }
            if o <= 20 {
                codes.push("B");
            }
            if o > 30 {
                codes.push("C");
            }
            insert(drivers, o, "valid", Some(json!({ "codes": codes })));
            insert(
                intent,
                o,
                "valid",
                Some(json!({ "value": ((o - 1) % 5 + 1) as f64 })),
            );
            let v = match o {
                1..=11 => 400.0,
                12..=20 => 600.0,
                21 => 700.0,
                22..=31 => 900.0,
                _ => 1500.0,
            };
            insert(budget, o, "valid", Some(json!({ "value": v })));
            why_ids.push((
                o,
                insert(
                    why,
                    o,
                    "valid",
                    Some(json!({ "text": format!("Answer {o}") })),
                ),
            ));
        }
        f.conn
            .execute_batch(&format!(
                "INSERT INTO themes(run_id, question_id, label, description) VALUES ({run_id}, {why}, 'Price', 'Too expensive');
                 INSERT INTO themes(run_id, question_id, label, description) VALUES ({run_id}, {why}, 'Battery is fine', 'No need yet');"
            ))
            .unwrap();
        for (o, resp) in why_ids {
            let theme = match o {
                1..=24 => Some(1),
                25..=34 => Some(2),
                _ => None,
            };
            if let Some(t) = theme {
                f.conn
                    .execute(
                        "INSERT INTO response_themes(response_id, theme_id) VALUES (?1, ?2)",
                        params![resp, t],
                    )
                    .unwrap();
            }
        }
        f
    }
}
