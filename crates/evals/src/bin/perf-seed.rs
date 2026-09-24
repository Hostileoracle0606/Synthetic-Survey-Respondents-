//! Seeds an app database for the release performance run (TEST_PLAN S7 `memory_1000`,
//! S10 `stream_fps_1000`; BACKLOG B14): one project with a locked Canadian census cohort of
//! 1,000 respondents and 20 approved questions of every type, ready for Run Survey Simulation.
//!
//!   perf-seed --db <data.db> [--respondents 1000] [--questions 20]
//!
//! The file must not exist yet. Prints the project id.

use std::path::PathBuf;

use survey_core::db::surveys::{self, NewQuestion};
use survey_core::db::{cohorts, open, projects};
use survey_core::engine::persona;
use survey_core::model::{
    ChoiceOption, CohortConfig, CohortStatus, NumericRange, QuestionBody, QuestionType,
    ResearchType, Scale, SurveyInfo,
};
use survey_core::sampling::{census_default_quotas, Sampler};
use survey_evals::{err, flag};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match seed(&args) {
        Ok(project) => println!("{project}"),
        Err(e) => {
            eprintln!("perf-seed: {e}");
            std::process::exit(1);
        }
    }
}

fn seed(args: &[String]) -> Result<i64, String> {
    let path = PathBuf::from(flag(args, "--db").ok_or("--db <data.db> is required")?);
    let size: u32 =
        flag(args, "--respondents").map_or(Ok(1000), |v| v.parse().map_err(|_| "--respondents"))?;
    let questions: usize =
        flag(args, "--questions").map_or(Ok(20), |v| v.parse().map_err(|_| "--questions"))?;
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let conn = open(&path).map_err(err)?;
    let project = projects::save_survey_info(
        &conn,
        None,
        &SurveyInfo {
            title: format!("Performance run: {size} × {questions}"),
            research_type: Some(ResearchType::MarketResponse),
            product_category: Some("mobile_phone".into()),
            countries: vec!["CA".into()],
            research_goal: "Measure how the app behaves while a large run streams in.".into(),
        },
    )
    .map_err(err)?
    .id;

    let config = CohortConfig {
        size,
        seed: 3,
        quotas: census_default_quotas("CA").ok_or("no census table for CA")?,
        screening: String::new(),
        non_binary_share: 0,
        countries: vec!["CA".into()],
    };
    let cohort = cohorts::create(
        &conn,
        project,
        &config,
        None,
        "flash",
        persona::PROMPT_VERSION,
    )
    .map_err(err)?;
    let skels = Sampler::new(&config, &["CA".into()])
        .map_err(err)?
        .draw()
        .map_err(err)?;
    let ordinals: Vec<u32> = skels.iter().map(|s| s.ordinal).collect();
    let people = persona::parse_reply(
        &persona::sample_reply(&skels, Some("mobile_phone"), &[]),
        &ordinals,
        Some("mobile_phone"),
    )
    .map_err(|e| e.to_string())?;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for (s, p) in skels.iter().zip(&people) {
        cohorts::insert_respondent(&tx, cohort.id, s, p, "passed").map_err(err)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    cohorts::set_status(&conn, cohort.id, CohortStatus::Ready, None).map_err(err)?;
    cohorts::lock(&conn, cohort.id).map_err(err)?;

    let survey = surveys::for_project(&conn, project).map_err(err)?;
    for i in 0..questions {
        let id = surveys::insert_ai(
            &conn,
            survey.id,
            &NewQuestion {
                code: format!("Q{}", i + 1),
                body: body(i),
                objective: None,
                rationale: None,
            },
            false,
        )
        .map_err(err)?;
        surveys::approve_question(&conn, id).map_err(err)?;
    }
    Ok(project)
}

/// Cycles through every question type, so every chart and cross-tab kind is drawn.
fn body(i: usize) -> QuestionBody {
    let t = [
        QuestionType::SingleChoice,
        QuestionType::MultiChoice,
        QuestionType::Likert,
        QuestionType::Numeric,
        QuestionType::OpenEnded,
    ][i % 5];
    let choice = matches!(t, QuestionType::SingleChoice | QuestionType::MultiChoice);
    QuestionBody {
        text: format!("Question {} about choosing a phone?", i + 1),
        question_type: t,
        options: if choice {
            ["Apple", "Samsung", "Google", "Motorola", "Other"]
                .iter()
                .enumerate()
                .map(|(j, l)| ChoiceOption {
                    code: surveys::option_code(j),
                    label: l.to_string(),
                })
                .collect()
        } else {
            Vec::new()
        },
        randomize: choice,
        max_choices: (t == QuestionType::MultiChoice).then_some(2),
        scale: (t == QuestionType::Likert).then(|| Scale {
            min: 1,
            max: 7,
            min_label: "Low".into(),
            max_label: "High".into(),
        }),
        numeric: (t == QuestionType::Numeric).then(|| NumericRange {
            min: 0.0,
            max: 2500.0,
            unit: "CAD".into(),
        }),
    }
}
