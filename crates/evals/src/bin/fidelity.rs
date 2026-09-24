//! Fidelity benchmark tool (TEST_PLAN S13).
//!
//!   fidelity check <pack.json> [--release]
//!   fidelity candidates --category mobile_phone [--count 30] [--version fidelity.v2] --out <file>
//!   fidelity run --pack <pack.json> (--cohort-size 100 | --db <data.db> --cohort <id>)
//!                [--seed 11 | --seeds 11,17,23] [--compare-model gemini-x] [--concurrency 4]
//!                [--release] [--min-score 0] [--out <report.json>]
//!
//! `candidates` asks Gemini for questions only; a person writes the rules (decision D1).
//! `run --cohort-size` generates a Canadian census cohort with the app's persona job first.
//! `--release` refuses packs that are not frozen and reviewed.
//! `--seeds a,b,c` runs the same cohort under each seed and reports TEST_PLAN S13
//! "run-to-run stability" (per-question TVD between seeds, flagged at ≥ 0.1); the first seed
//! is also used to build the cohort and is the one `--min-score` and the printed report use.
//! `--compare-model` answers the same cohort again with a second model and reports TEST_PLAN
//! S13 "model comparison" against the primary model, side by side.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use survey_core::db::{cohorts, writer::Writer};
use survey_core::engine::cohort::{self, CohortJob};
use survey_core::fidelity::{self, candidates, runner};
use survey_core::llm::LlmProvider;
use survey_core::model::CohortConfig;
use survey_evals::{client, err, flag, limiter, models};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match args.first().map(String::as_str) {
        Some("check") => check(&args),
        Some("candidates") => generate(&args).await,
        Some("run") => run(&args).await,
        _ => Err("usage: fidelity check|candidates|run … (see the source header)".into()),
    };
    if let Err(e) = outcome {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn check(args: &[String]) -> Result<(), String> {
    let path = args.get(1).ok_or("fidelity check <pack.json>")?;
    let pack = fidelity::read_pack_file(Path::new(path)).map_err(err)?;
    match fidelity::validate(&pack, args.iter().any(|a| a == "--release")) {
        Ok(()) => {
            println!(
                "{}: {} questions, status {}: OK",
                pack.version,
                pack.questions.len(),
                pack.status
            );
            Ok(())
        }
        Err(problems) => Err(format!(
            "{} problem(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        )),
    }
}

async fn generate(args: &[String]) -> Result<(), String> {
    let category = flag(args, "--category").unwrap_or_else(|| "mobile_phone".into());
    let count: usize = flag(args, "--count")
        .and_then(|c| c.parse().ok())
        .unwrap_or(30);
    let version = flag(args, "--version").unwrap_or_else(|| "fidelity.v2".into());
    let out = flag(args, "--out").ok_or("--out <file> is required")?;
    let c = client()?;
    let (_, pro) = models(&c).await?;
    let reply = c
        .complete_structured(&candidates::request(&pro, &category, count))
        .await
        .map_err(|e| e.to_string())?;
    let pack = candidates::draft_pack(&reply.json, &category, &version);
    std::fs::write(
        &out,
        serde_json::to_string_pretty(&pack).unwrap_or_default(),
    )
    .map_err(|e| e.to_string())?;
    println!("{} candidate questions written to {out} (model {pro}). Write the rules by hand before use.", pack.questions.len());
    Ok(())
}

async fn run(args: &[String]) -> Result<(), String> {
    let pack_path = flag(args, "--pack").unwrap_or_else(|| "benchmarks/fidelity.v1.json".into());
    let pack = fidelity::read_pack_file(Path::new(&pack_path)).map_err(err)?;
    let release = args.iter().any(|a| a == "--release");
    fidelity::validate(&pack, release).map_err(|p| p.join("; "))?;
    let seeds: Vec<u64> = match flag(args, "--seeds") {
        Some(list) => list
            .split(',')
            .map(|s| s.trim().parse::<u64>().map_err(|_| format!("bad seed {s:?}")))
            .collect::<Result<_, _>>()?,
        None => vec![flag(args, "--seed")
            .and_then(|s| s.parse().ok())
            .unwrap_or(11)],
    };
    let concurrency: u32 = flag(args, "--concurrency")
        .and_then(|s| s.parse().ok())
        .unwrap_or(4);
    let c = client()?;
    let (flash, _) = models(&c).await?;
    let llm: Arc<dyn LlmProvider> = Arc::new(c);
    let lim = limiter(concurrency);

    let _tmp;
    let (db, cohort_id): (PathBuf, i64) = match (
        flag(args, "--db"),
        flag(args, "--cohort"),
        flag(args, "--cohort-size"),
    ) {
        (Some(db), Some(id), _) => (
            PathBuf::from(db),
            id.parse().map_err(|_| "--cohort must be a number")?,
        ),
        (_, _, Some(size)) => {
            let size: u32 = size.parse().map_err(|_| "--cohort-size must be a number")?;
            let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
            let path = dir.path().join("fidelity.db");
            _tmp = dir;
            let id = make_cohort(
                &path,
                size,
                seeds[0],
                pack.category.as_deref(),
                &flash,
                llm.clone(),
                lim.clone(),
            )
            .await?;
            (path, id)
        }
        _ => return Err("give --cohort-size N, or --db <file> and --cohort <id>".into()),
    };
    let people =
        fidelity::load_people(&survey_core::db::open(&db).map_err(err)?, cohort_id).map_err(err)?;

    let mut runs: Vec<(u64, runner::BenchRun)> = Vec::new();
    for &seed in &seeds {
        eprintln!(
            "answering {} questions for {} respondents with {flash} (seed {seed})…",
            pack.questions.len(),
            people.len()
        );
        let bench = runner::run(
            &pack,
            &people,
            llm.clone(),
            lim.clone(),
            &flash,
            seed,
            concurrency as usize,
        )
        .await
        .map_err(err)?;
        runs.push((seed, bench));
    }
    let report = fidelity::score(
        &pack,
        &people,
        &runs[0].1.answers,
        (&flash, survey_core::engine::answer::PROMPT_VERSION, seeds[0]),
    );

    println!(
        "Fidelity {} ({} pack {}, {} respondents, model {flash}, seed {})",
        report.score, report.pack_status, report.pack_version, report.respondents, seeds[0]
    );
    println!(
        "Attribute sensitivity: {}% of questions (target ≥ 80%)",
        report.sensitive_share
    );
    for q in &report.questions {
        println!(
            "  {:<18} {:<18} TVD {:.3}  sensitive {:<5} {}",
            q.code,
            q.attribute,
            q.tvd,
            q.sensitive.map_or("n/a".into(), |s| s.to_string()),
            q.validity_flags.join(" | ")
        );
    }
    for s in &report.worst_subgroups {
        println!(
            "  worst: {} = {} (n={}) {}{}",
            s.dimension,
            s.group,
            s.n,
            s.score,
            if s.flagged { " FLAGGED" } else { "" }
        );
    }
    let total_invalid: u32 = runs.iter().map(|(_, b)| b.invalid).sum();
    let total_failed: u32 = runs.iter().map(|(_, b)| b.failed).sum();
    println!("{total_invalid} invalid answers, {total_failed} failed respondents. {}", report.disclosure);

    // TEST_PLAN S13 "run-to-run stability": only meaningful with more than one seed.
    let stability = (runs.len() > 1).then(|| {
        let by_seed: Vec<(u64, Vec<fidelity::BenchAnswer>)> = runs
            .iter()
            .map(|(seed, bench)| (*seed, bench.answers.clone()))
            .collect();
        let s = fidelity::stability(&pack, &people, &flash, &by_seed);
        println!(
            "\nStability across seeds {:?}: {} (max TVD {:.3}, threshold 0.1)",
            s.seeds,
            if s.stable { "stable" } else { "UNSTABLE" },
            s.max_tvd
        );
        for q in s.questions.iter().filter(|q| q.flagged) {
            println!("  {:<18} TVD {:.3} FLAGGED", q.code, q.tvd);
        }
        s
    });

    // TEST_PLAN S13 "model comparison": only run when a second model is given.
    let comparison = match flag(args, "--compare-model") {
        Some(other) => {
            eprintln!("answering the same cohort with {other} for comparison…");
            let other_bench = runner::run(
                &pack,
                &people,
                llm.clone(),
                lim.clone(),
                &other,
                seeds[0],
                concurrency as usize,
            )
            .await
            .map_err(err)?;
            let other_report = fidelity::score(
                &pack,
                &people,
                &other_bench.answers,
                (&other, survey_core::engine::answer::PROMPT_VERSION, seeds[0]),
            );
            let cmp = fidelity::compare_models(&report, &other_report);
            println!(
                "\nModel comparison: {} {} vs {} {} (informs the default model choice, not pass/fail)",
                cmp.a_model, cmp.a_score, cmp.b_model, cmp.b_score
            );
            for row in &cmp.rows {
                println!(
                    "  {:<18} {:<18} TVD {:.3} vs {:.3}",
                    row.code, row.attribute, row.a_tvd, row.b_tvd
                );
            }
            Some(cmp)
        }
        None => None,
    };

    if let Some(out) = flag(args, "--out") {
        let payload = serde_json::json!({
            "report": report,
            "stability": stability,
            "model_comparison": comparison,
        });
        std::fs::write(
            &out,
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(min) = flag(args, "--min-score").and_then(|m| m.parse::<f64>().ok()) {
        if report.score < min {
            return Err(format!(
                "fidelity {} is below --min-score {min}",
                report.score
            ));
        }
    }
    Ok(())
}

/// A Canadian census cohort made by the app's own persona job.
async fn make_cohort(
    path: &Path,
    size: u32,
    seed: u64,
    category: Option<&str>,
    model: &str,
    llm: Arc<dyn LlmProvider>,
    lim: Arc<survey_core::engine::limiter::RateLimiter>,
) -> Result<i64, String> {
    let conn = survey_core::db::open(path).map_err(err)?;
    conn.execute(
        "INSERT INTO projects(title, research_type, product_category, countries_json) VALUES ('Fidelity benchmark', 'market_response', ?1, '[\"CA\"]')",
        [category],
    )
    .map_err(|e| e.to_string())?;
    let config = CohortConfig {
        size,
        seed,
        quotas: survey_core::sampling::census_default_quotas("CA").ok_or("no Canada table")?,
        screening: String::new(),
        non_binary_share: 0,
        countries: vec!["CA".into()],
    };
    let cohort = cohorts::create(
        &conn,
        1,
        &config,
        None,
        model,
        survey_core::engine::persona::PROMPT_VERSION,
    )
    .map_err(err)?;
    let (writer, _h) = Writer::spawn(
        survey_core::db::open(path).map_err(err)?,
        50,
        Duration::from_millis(50),
    );
    eprintln!("generating {size} personas with {model}…");
    cohort::run(
        CohortJob {
            cohort_id: cohort.id,
            countries: vec!["CA".into()],
            category: category.map(str::to_string),
            config,
            model: model.to_string(),
            batch_size: 8,
            max_screen_attempts: 3,
        },
        llm,
        lim,
        writer,
        Arc::new(|_| {}),
    )
    .await
    .map_err(err)?;
    Ok(cohort.id)
}
