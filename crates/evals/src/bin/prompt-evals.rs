//! Prompt-behaviour evals (TEST_PLAN S12).
//!
//!   prompt-evals [--cases evals/cases] [--out evals/results.jsonl] [--only <eval>]
//!
//! Appends one JSON line per case to the results file and prints pass rates per eval.
//! Exits non-zero if a zero-tolerance eval (injection) has any failure.

use std::io::Write;
use std::sync::Arc;

use survey_core::llm::LlmProvider;
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
    let broken: Vec<&str> = results
        .iter()
        .filter(|r| !r.pass && ZERO_TOLERANCE.contains(&r.eval.as_str()))
        .map(|r| r.id.as_str())
        .collect();
    if broken.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "zero-tolerance evals failed: {}",
            broken.join(", ")
        ))
    }
}
