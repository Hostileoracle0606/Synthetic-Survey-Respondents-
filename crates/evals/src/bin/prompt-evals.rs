//! Prompt-behaviour evals (TEST_PLAN S12).
//!
//!   prompt-evals [--cases evals/cases] [--out evals/results.jsonl] [--only <eval>] [--set-baseline]
//!
//! Appends one JSON line per case to the results file and prints pass rates per eval.
//! Exits non-zero if a zero-tolerance eval (injection) has any failure, an absolute-floor eval
//! (no_stereotyping) is under it, or a baseline-relative eval (in_character, uncertainty,
//! drafting) has dropped more than its allowed slack below its last recorded baseline
//! (BACKLOG B29). `--set-baseline` records this run's rates as the new baseline for each
//! (eval, prompt version) instead of grading it against the old one — run it once a person has
//! looked at the results and is satisfied with them, not on every change.

use std::collections::BTreeSet;
use std::io::Write;
use std::sync::Arc;

use survey_core::llm::LlmProvider;
use survey_evals::baseline;
use survey_evals::judge::{run_case, summary, EvalCase, ZERO_TOLERANCE};
use survey_evals::{client, flag, limiter, models};

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = flag(&args, "--cases").unwrap_or_else(|| "evals/cases".into());
    let out = flag(&args, "--out").unwrap_or_else(|| "evals/results.jsonl".into());
    let only = flag(&args, "--only");
    let mut cases: Vec<EvalCase> = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{dir}: {e}"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    for f in files
        .iter()
        .filter(|f| f.extension().is_some_and(|x| x == "json"))
    {
        let text = std::fs::read_to_string(f).map_err(|e| e.to_string())?;
        let mut v: Vec<EvalCase> =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", f.display()))?;
        cases.append(&mut v);
    }
    cases.retain(|c| only.as_ref().map_or(true, |o| &c.eval == o));
    let set_baseline = args.iter().any(|a| a == "--set-baseline");
    let c = client()?;
    let (flash, pro) = models(&c).await?;
    let llm: Arc<dyn LlmProvider> = Arc::new(c);
    let lim = limiter(4);
    let mut results = Vec::new();
    for case in &cases {
        let r = run_case(case, &llm, &llm, &lim, (&flash, &pro)).await;
        println!(
            "{:<16} {:<8} {}  {}",
            r.eval,
            r.id,
            if r.pass { "PASS" } else { "FAIL" },
            r.reason
        );
        results.push(r);
    }
    // Read before this run's own lines are appended, so a baseline lookup below never sees
    // itself, and so `--set-baseline` never rewrites `results` history, only adds to it.
    let existing = std::fs::read_to_string(&out).unwrap_or_default();
    let date = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&out)
        .map_err(|e| e.to_string())?;
    for r in &results {
        let mut line = serde_json::to_value(r).unwrap_or_default();
        line["unix_time"] = date.into();
        writeln!(file, "{line}").map_err(|e| e.to_string())?;
    }
    println!("\nPass rates (respondent {flash}, judge {pro}):");
    for (eval, n, rate) in summary(&results) {
        println!("  {eval:<16} {rate:>5}% of {n}");
    }

    if set_baseline {
        for b in baseline::make(&results, date) {
            writeln!(file, "{}", serde_json::to_value(&b).unwrap_or_default())
                .map_err(|e| e.to_string())?;
            println!("  baseline set: {} ({}) = {}%", b.eval, b.prompt_version, b.rate);
        }
        return Ok(());
    }

    let mut problems: Vec<String> = results
        .iter()
        .filter(|r| !r.pass && ZERO_TOLERANCE.contains(&r.eval.as_str()))
        .map(|r| format!("zero-tolerance eval {} failed ({})", r.eval, r.id))
        .collect();
    // Counts per (eval, prompt_version): usually one prompt version per eval, but never
    // assumed, since mixing them into one rate would compare against the wrong baseline.
    let keys: BTreeSet<(String, String)> = results
        .iter()
        .map(|r| (r.eval.clone(), r.prompt_version.clone()))
        .collect();
    for (eval, prompt_version) in keys {
        let (n, pass) = results
            .iter()
            .filter(|r| r.eval == eval && r.prompt_version == prompt_version)
            .fold((0usize, 0usize), |(n, p), r| (n + 1, p + usize::from(r.pass)));
        let rate = (pass as f64 * 1000.0 / n as f64).round() / 10.0;
        if let Err(e) = baseline::check(&eval, &prompt_version, rate, &existing) {
            problems.push(e);
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}
