//! TEST_PLAN S7 `memory_1000`, Rust side: a 1,000-respondent × 20-question run, its report,
//! cross-tabs and both exports, measured as the peak resident memory of this process.
//! Its own test binary, so nothing else shares the process. Linux only (reads /proc).
//! The WebView2 half of the budget needs the Windows VMs (BACKLOG B14).

use std::sync::Arc;
use std::time::Duration;

use survey_core::db::runs::{self, RunSettings};
use survey_core::db::surveys::{self, NewQuestion};
use survey_core::db::{cohorts, open, writer::Writer};
use survey_core::engine::answer::reply_for_prompt;
use survey_core::engine::limiter::{Limits, RateLimiter};
use survey_core::engine::persona;
use survey_core::engine::run::{run, Mode};
use survey_core::llm::scripted::ScriptedLlm;
use survey_core::model::{
    ChoiceOption, CohortConfig, CohortStatus, NumericRange, QuestionBody, QuestionType, RunConfig,
    RunStatus, Scale,
};
use survey_core::report::{self, export};
use survey_core::sampling::{census_default_quotas, Sampler};

/// Rust process budget from SPEC §10 / decision 2026-09-23.
const BUDGET_MB: f64 = 50.0;

fn peak_rss_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb: f64 = status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(kb / 1024.0)
}

fn body(i: usize) -> QuestionBody {
    let t = [
        QuestionType::SingleChoice,
        QuestionType::MultiChoice,
        QuestionType::Likert,
        QuestionType::Numeric,
        QuestionType::OpenEnded,
    ][i % 5];
    QuestionBody {
        text: format!("Question {i} about phones?"),
        question_type: t,
        options: ["Apple", "Samsung", "Google", "Motorola", "Other"]
            .iter()
            .enumerate()
            .map(|(j, l)| ChoiceOption {
                code: surveys::option_code(j),
                label: l.to_string(),
            })
            .collect(),
        randomize: true,
        max_choices: Some(2),
        scale: Some(Scale {
            min: 1,
            max: 7,
            min_label: "Low".into(),
            max_label: "High".into(),
        }),
        numeric: Some(NumericRange {
            min: 0.0,
            max: 2500.0,
            unit: "CAD".into(),
        }),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thousand_by_twenty_stays_within_the_rust_memory_budget() {
    let Some(start) = peak_rss_mb() else {
        eprintln!("skipped: /proc not available on this platform");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mem.db");
    let conn = open(&path).unwrap();
    conn.execute("INSERT INTO projects(title, research_type, countries_json) VALUES ('mem', 'market_response', '[\"CA\"]')", []).unwrap();
    let config = CohortConfig {
        size: 1000,
        seed: 3,
        quotas: census_default_quotas("CA").unwrap(),
        screening: String::new(),
        non_binary_share: 0,
        countries: vec!["CA".into()],
    };
    let cohort =
        cohorts::create(&conn, 1, &config, None, "flash", persona::PROMPT_VERSION).unwrap();
    let skels = Sampler::new(&config, &["CA".into()])
        .unwrap()
        .draw()
        .unwrap();
    let ordinals: Vec<u32> = skels.iter().map(|s| s.ordinal).collect();
    let people = persona::parse_reply(
        &persona::sample_reply(&skels, Some("mobile_phone"), &[]),
        &ordinals,
        Some("mobile_phone"),
    )
    .unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    for (s, p) in skels.iter().zip(&people) {
        cohorts::insert_respondent(&tx, cohort.id, s, p, "passed").unwrap();
    }
    tx.commit().unwrap();
    cohorts::set_status(&conn, cohort.id, CohortStatus::Ready, None).unwrap();
    cohorts::lock(&conn, cohort.id).unwrap();
    let survey = surveys::for_project(&conn, 1).unwrap();
    for i in 0..20 {
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
        .unwrap();
        surveys::approve_question(&conn, id).unwrap();
    }
    let run_id = runs::start(
        &conn,
        1,
        &RunConfig { seed: None },
        &RunSettings {
            model: "flash",
            max_concurrency: 10,
            requests_left_today: 10_000,
            est_cost_usd: None,
        },
    )
    .unwrap()
    .id;

    let (writer, _h) = Writer::spawn(open(&path).unwrap(), 50, Duration::from_millis(20));
    let limiter = Arc::new(RateLimiter::new(
        Limits {
            requests_per_minute: 1_000_000,
            tokens_per_minute: u32::MAX,
            requests_per_day: 1_000_000,
            max_concurrency: 10,
        },
        0,
    ));
    let llm = ScriptedLlm::new(|req, _| Ok(reply_for_prompt(&req.prompt))).without_request_log();
    let (_ctl, rx) = tokio::sync::watch::channel(Mode::Run);
    let plan = runs::plan(&conn, run_id).unwrap();
    eprintln!("after setup {:.1} MB", peak_rss_mb().unwrap());
    let out = run(plan, Arc::new(llm), limiter, writer, rx, Arc::new(|_| {}))
        .await
        .unwrap();
    assert_eq!(out.status, RunStatus::Completed);

    eprintln!("after run {:.1} MB", peak_rss_mb().unwrap());
    let rep = report::report(&conn, run_id).unwrap();
    assert_eq!(rep.based_on_n, 1000);
    for q in &rep.questions {
        for d in &rep.dimensions {
            report::crosstab(&conn, run_id, q.question_id, &d.key).unwrap();
        }
    }
    eprintln!(
        "after report and cross-tabs {:.1} MB",
        peak_rss_mb().unwrap()
    );
    let csv = export::csv(&conn, run_id).unwrap();
    assert_eq!(csv.lines().count(), 1001);
    let out_path = dir.path().join("export.json");
    let mut file = std::io::BufWriter::new(std::fs::File::create(&out_path).unwrap());
    export::write_json(&conn, run_id, &mut file).unwrap();
    drop(file);
    let size = std::fs::metadata(&out_path).unwrap().len();
    assert!(size > 2_000_000, "export is only {size} bytes");
    drop((csv, rep));

    let peak = peak_rss_mb().unwrap();
    eprintln!(
        "peak resident memory {peak:.1} MB (process start {start:.1} MB), budget {BUDGET_MB} MB"
    );
    assert!(
        peak <= BUDGET_MB,
        "peak {peak:.1} MB is over the {BUDGET_MB} MB budget"
    );
}
