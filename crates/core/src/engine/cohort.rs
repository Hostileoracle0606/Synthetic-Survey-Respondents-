//! The persona job (Step 1 → Step 2, docs/DATA_FLOW.md §3): draw skeletons, enrich them
//! with Gemini in parallel batches, replace people who fail screening from the same quota
//! cell, and save each batch as it completes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::params;
use tokio::task::JoinSet;

use super::limiter::RateLimiter;
use super::persona::{build_request, parse_reply, BatchInput, EnrichedPersona};
use crate::db::cohorts;
use crate::db::writer::Writer;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::llm::{LlmError, LlmProvider, StructuredResponse};
use crate::model::{CohortConfig, CohortProgress, CohortStatus};
use crate::sampling::{Sampler, Skeleton};

pub struct CohortJob {
    pub cohort_id: i64,
    pub countries: Vec<String>,
    pub category: Option<String>,
    pub config: CohortConfig,
    pub model: String,
    pub batch_size: usize,
    /// Screening redraws per ordinal before a persona is kept as "flagged".
    pub max_screen_attempts: u32,
}

pub type ProgressFn = Arc<dyn Fn(CohortProgress) + Send + Sync>;

#[derive(Default)]
struct Shared {
    done: u32,
    replaced: u32,
    /// Short summaries per quota cell, fed back as "already written" to avoid near-duplicates.
    written: HashMap<String, Vec<String>>,
}

struct Ctx {
    job: CohortJob,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
    sampler: Sampler,
    shared: Mutex<Shared>,
    progress: ProgressFn,
    total: u32,
}

/// Runs the whole job. On success the cohort is `ready`; on failure it is `failed` with the
/// reason stored, and the error is returned.
pub async fn run(
    job: CohortJob,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
    progress: ProgressFn,
) -> AppResult<()> {
    let cohort_id = job.cohort_id;
    let outcome = run_inner(job, llm, limiter, writer.clone(), progress).await;
    let (status, error) = match &outcome {
        Ok(()) => (CohortStatus::Ready, None),
        Err(e) => (CohortStatus::Failed, Some(e.message.clone())),
    };
    writer
        .write(Box::new(move |c| {
            cohorts::set_status(c, cohort_id, status, error.as_deref())
        }))
        .await?;
    outcome
}

async fn run_inner(
    job: CohortJob,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
    progress: ProgressFn,
) -> AppResult<()> {
    let sampler = Sampler::new(&job.config, &job.countries)?;
    let skeletons = sampler.draw()?;
    let total = skeletons.len() as u32;
    let batch_size = job.batch_size.max(1);
    let ctx = Arc::new(Ctx {
        job,
        llm,
        limiter,
        writer,
        sampler,
        shared: Mutex::default(),
        progress,
        total,
    });
    (ctx.progress)(CohortProgress {
        done: 0,
        total,
        replaced: 0,
    });

    let mut tasks = JoinSet::new();
    for chunk in skeletons.chunks(batch_size) {
        let ctx = ctx.clone();
        let chunk = chunk.to_vec();
        tasks.spawn(async move { process_batch(&ctx, chunk).await });
    }
    while let Some(res) = tasks.join_next().await {
        let res =
            res.map_err(|e| AppError::new(ErrorCode::Internal, format!("batch task failed: {e}")))?;
        if let Err(e) = res {
            tasks.abort_all();
            return Err(e);
        }
    }
    Ok(())
}

async fn process_batch(ctx: &Ctx, batch: Vec<Skeleton>) -> AppResult<()> {
    let mut todo = batch;
    let mut attempt = 0;
    while !todo.is_empty() {
        let enriched = enrich(ctx, todo).await?;
        let mut redo = Vec::new();
        let mut rows = Vec::new();
        {
            let mut sh = ctx.shared.lock().unwrap();
            for (s, p) in enriched {
                let status = if p.passes_screen {
                    "passed"
                } else if attempt >= ctx.job.max_screen_attempts {
                    "flagged"
                } else {
                    "failed"
                };
                if status == "failed" {
                    sh.replaced += 1;
                    redo.push(ctx.sampler.redraw(&s, attempt + 1)?);
                } else {
                    sh.done += 1;
                    let short: String = p
                        .summary
                        .split_whitespace()
                        .take(25)
                        .collect::<Vec<_>>()
                        .join(" ");
                    sh.written
                        .entry(s.quota_cell.clone())
                        .or_default()
                        .push(short);
                }
                rows.push((s, p, status));
            }
        }
        let cohort_id = ctx.job.cohort_id;
        ctx.writer
            .write(Box::new(move |c| {
                for (s, p, status) in &rows {
                    cohorts::insert_respondent(c, cohort_id, s, p, status)?;
                }
                Ok(())
            }))
            .await?;
        let (done, replaced) = {
            let sh = ctx.shared.lock().unwrap();
            (sh.done, sh.replaced)
        };
        (ctx.progress)(CohortProgress {
            done,
            total: ctx.total,
            replaced,
        });
        todo = redo;
        attempt += 1;
    }
    Ok(())
}

/// Enriches a group of skeletons. A malformed reply is retried once with the problem stated;
/// if it fails again the group is split and each person is tried alone. A single person whose
/// reply keeps failing (or is blocked) is redrawn from the same quota cell.
async fn enrich(ctx: &Ctx, skels: Vec<Skeleton>) -> AppResult<Vec<(Skeleton, EnrichedPersona)>> {
    match call_with_repair(ctx, &skels).await {
        Ok(ps) => Ok(zip(skels, ps)),
        Err(LlmError::SchemaViolation(_) | LlmError::Blocked(_)) if skels.len() > 1 => {
            let mut out = Vec::new();
            for s in skels {
                out.extend(Box::pin(enrich(ctx, vec![s])).await?);
            }
            Ok(out)
        }
        Err(e @ (LlmError::SchemaViolation(_) | LlmError::Blocked(_))) => {
            let mut s = skels.into_iter().next().expect("one skeleton");
            for attempt in 1..=2 {
                s = ctx.sampler.redraw(&s, 100 + attempt)?;
                if let Ok(ps) = call_with_repair(ctx, std::slice::from_ref(&s)).await {
                    return Ok(zip(vec![s], ps));
                }
            }
            Err(AppError::from(e))
        }
        Err(e) => Err(AppError::from(e)),
    }
}

fn zip(skels: Vec<Skeleton>, personas: Vec<EnrichedPersona>) -> Vec<(Skeleton, EnrichedPersona)> {
    skels.into_iter().zip(personas).collect()
}

async fn call_with_repair(ctx: &Ctx, skels: &[Skeleton]) -> Result<Vec<EnrichedPersona>, LlmError> {
    match call(ctx, skels, None).await {
        Err(LlmError::SchemaViolation(problem)) => call(ctx, skels, Some(&problem)).await,
        other => other,
    }
}

/// One logical request, with waiting and retries for rate limits and temporary failures.
async fn call(
    ctx: &Ctx,
    skels: &[Skeleton],
    problem: Option<&str>,
) -> Result<Vec<EnrichedPersona>, LlmError> {
    let already: Vec<String> = {
        let sh = ctx.shared.lock().unwrap();
        let mut v = Vec::new();
        for s in skels {
            if let Some(list) = sh.written.get(&s.quota_cell) {
                v.extend(list.iter().rev().take(3).cloned());
            }
        }
        v.truncate(8);
        v
    };
    let mut req = build_request(
        &ctx.job.model,
        &BatchInput {
            skeletons: skels,
            category: ctx.job.category.as_deref(),
            screening: &ctx.job.config.screening,
            already_written: &already,
        },
    );
    if let Some(p) = problem {
        req.prompt.push_str(&format!(
            "\nYour previous reply was rejected: {p}. Follow the format exactly.\n"
        ));
    }
    let ordinals: Vec<u32> = skels.iter().map(|s| s.ordinal).collect();
    let estimate = ((req.system.len() + req.prompt.len()) / 4) as u32;
    for attempt in 1..=6u32 {
        let permit = ctx
            .limiter
            .acquire(estimate)
            .await
            .map_err(|e| LlmError::InvalidRequest(e.message))?;
        let result = ctx.llm.complete_structured(&req).await;
        log_call(ctx, attempt, &result);
        match result {
            Ok(resp) => {
                ctx.limiter
                    .record_actual(&permit, resp.usage.input_tokens)
                    .await;
                return parse_reply(&resp.json, &ordinals, ctx.job.category.as_deref());
            }
            Err(LlmError::RateLimited { retry_after }) if attempt < 6 => {
                drop(permit);
                tokio::time::sleep(
                    retry_after.unwrap_or(Duration::from_secs(5 * u64::from(attempt))),
                )
                .await;
            }
            Err(LlmError::Transient(_)) if attempt < 6 => {
                drop(permit);
                tokio::time::sleep(Duration::from_secs(1 << attempt.min(5))).await;
            }
            Err(e) => return Err(e),
        }
    }
    Err(LlmError::Transient(
        "gave up after repeated failures".into(),
    ))
}

fn log_call(ctx: &Ctx, attempt: u32, result: &Result<StructuredResponse, LlmError>) {
    let cohort_id = ctx.job.cohort_id;
    let (usage, latency, error) = match result {
        Ok(r) => (Some(r.usage), Some(r.latency_ms as i64), None),
        Err(e) => (None, None, Some(e.to_string())),
    };
    let _ = ctx.writer.send(Box::new(move |c| {
        c.execute(
            "INSERT INTO llm_calls(cohort_id, purpose, attempt, input_tokens, cached_tokens, output_tokens, latency_ms, error)
             VALUES (?1, 'persona', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                cohort_id,
                attempt,
                usage.map(|u| u.input_tokens),
                usage.map(|u| u.cached_tokens),
                usage.map(|u| u.output_tokens),
                latency,
                error
            ],
        )?;
        Ok(())
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::limiter::Limits;
    use crate::engine::persona::{ordinals_in_prompt, reply_for_ordinals, PROMPT_VERSION};
    use crate::llm::scripted::ScriptedLlm;
    use crate::model::{QuotaGroup, QuotaRow};
    use std::collections::HashSet;

    struct Env {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        writer: Writer,
        cohort_id: i64,
        config: CohortConfig,
    }

    fn config(size: u32) -> CohortConfig {
        CohortConfig {
            size,
            seed: 99,
            quotas: vec![
                QuotaGroup {
                    key: "age".into(),
                    label: "Age".into(),
                    rows: vec![
                        QuotaRow {
                            label: "18–29".into(),
                            percent: 25,
                        },
                        QuotaRow {
                            label: "30–44".into(),
                            percent: 30,
                        },
                        QuotaRow {
                            label: "45–59".into(),
                            percent: 25,
                        },
                        QuotaRow {
                            label: "60+".into(),
                            percent: 20,
                        },
                    ],
                },
                QuotaGroup {
                    key: "income".into(),
                    label: "Income".into(),
                    rows: vec![
                        QuotaRow {
                            label: "Under $50k".into(),
                            percent: 30,
                        },
                        QuotaRow {
                            label: "$50k–$100k".into(),
                            percent: 40,
                        },
                        QuotaRow {
                            label: "Over $100k".into(),
                            percent: 30,
                        },
                    ],
                },
            ],
            screening: "Owns a smartphone.".into(),
        }
    }

    fn env(size: u32) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let conn = crate::db::open(&path).unwrap();
        conn.execute("INSERT INTO projects(title) VALUES ('p')", [])
            .unwrap();
        let cfg = config(size);
        let cohort = cohorts::create(&conn, 1, &cfg, None, "test-flash", PROMPT_VERSION).unwrap();
        let (writer, _h) = Writer::spawn(
            crate::db::open(&path).unwrap(),
            50,
            Duration::from_millis(20),
        );
        Env {
            _dir: dir,
            path,
            writer,
            cohort_id: cohort.id,
            config: cfg,
        }
    }

    fn job(e: &Env) -> CohortJob {
        CohortJob {
            cohort_id: e.cohort_id,
            countries: vec!["CA".into()],
            category: Some("mobile_phone".into()),
            config: e.config.clone(),
            model: "test-flash".into(),
            batch_size: 8,
            max_screen_attempts: 3,
        }
    }

    fn limiter(conc: u32) -> Arc<RateLimiter> {
        Arc::new(RateLimiter::new(
            Limits {
                requests_per_minute: 100_000,
                tokens_per_minute: 100_000_000,
                requests_per_day: 100_000,
                max_concurrency: conc,
            },
            0,
        ))
    }

    fn count(e: &Env, sql: &str) -> i64 {
        crate::db::open(&e.path)
            .unwrap()
            .query_row(sql, [], |r| r.get(0))
            .unwrap()
    }

    fn good_llm() -> ScriptedLlm {
        ScriptedLlm::new(|req, _| {
            Ok(reply_for_ordinals(
                &ordinals_in_prompt(&req.prompt),
                Some("mobile_phone"),
                &[],
            ))
        })
    }

    #[tokio::test]
    async fn happy_path_saves_every_persona_with_exact_quotas() {
        let e = env(20);
        let llm = good_llm();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        run(
            job(&e),
            Arc::new(llm.clone()),
            limiter(4),
            e.writer.clone(),
            Arc::new(move |p| s2.lock().unwrap().push(p)),
        )
        .await
        .unwrap();
        assert_eq!(llm.calls(), 3); // 8 + 8 + 4
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM respondents WHERE screen_status = 'passed'"
            ),
            20
        );
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM respondents WHERE income_bracket = '$50k–$100k'"
            ),
            8
        );
        assert_eq!(
            count(&e, "SELECT COUNT(*) FROM cohorts WHERE status = 'ready'"),
            1
        );
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM llm_calls WHERE purpose = 'persona'"
            ),
            3
        );
        assert_eq!(seen.lock().unwrap().last().unwrap().done, 20);
        // The research objective never reaches persona prompts; the screening criteria do.
        assert!(llm
            .requests()
            .iter()
            .all(|r| r.prompt.contains("Owns a smartphone.")));
    }

    #[tokio::test]
    async fn screening_failures_are_kept_and_replaced_from_the_same_quota_cell() {
        let e = env(20);
        let failed_once = Arc::new(Mutex::new(HashSet::new()));
        let f = failed_once.clone();
        let llm = ScriptedLlm::new(move |req, _| {
            let ords = ordinals_in_prompt(&req.prompt);
            let mut set = f.lock().unwrap();
            let fail: Vec<u32> = ords
                .iter()
                .copied()
                .filter(|o| o % 5 == 0 && set.insert(*o))
                .collect();
            Ok(reply_for_ordinals(&ords, Some("mobile_phone"), &fail))
        });
        run(
            job(&e),
            Arc::new(llm),
            limiter(4),
            e.writer.clone(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM respondents WHERE screen_status = 'failed'"
            ),
            4
        );
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM respondents WHERE screen_status <> 'failed'"
            ),
            20
        );
        assert_eq!(count(&e, "SELECT COUNT(*) FROM respondents WHERE screen_status <> 'failed' AND income_bracket = 'Under $50k'"), 6);
        // Each replacement has the same quota cell as the persona it replaced.
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM respondents f JOIN respondents k ON k.cohort_id = f.cohort_id AND k.ordinal = f.ordinal
                 WHERE f.screen_status = 'failed' AND k.screen_status <> 'failed' AND k.quota_cell = f.quota_cell"
            ),
            4
        );
    }

    #[tokio::test]
    async fn malformed_reply_is_retried_once_then_accepted() {
        let e = env(8);
        let llm = ScriptedLlm::new(|req, i| {
            if i == 0 {
                return Ok(serde_json::json!({ "personas": [] }));
            }
            Ok(reply_for_ordinals(
                &ordinals_in_prompt(&req.prompt),
                Some("mobile_phone"),
                &[],
            ))
        });
        run(
            job(&e),
            Arc::new(llm.clone()),
            limiter(1),
            e.writer.clone(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert_eq!(llm.calls(), 2);
        assert!(llm.requests()[1]
            .prompt
            .contains("previous reply was rejected"));
        assert_eq!(count(&e, "SELECT COUNT(*) FROM respondents"), 8);
    }

    #[tokio::test]
    async fn batches_that_keep_failing_are_split_into_single_people() {
        let e = env(8);
        let llm = ScriptedLlm::new(|req, _| {
            let ords = ordinals_in_prompt(&req.prompt);
            if ords.len() > 1 {
                return Ok(serde_json::json!({ "personas": [] }));
            }
            Ok(reply_for_ordinals(&ords, Some("mobile_phone"), &[]))
        });
        run(
            job(&e),
            Arc::new(llm.clone()),
            limiter(1),
            e.writer.clone(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert_eq!(llm.calls(), 2 + 8);
        assert_eq!(
            count(
                &e,
                "SELECT COUNT(*) FROM respondents WHERE screen_status = 'passed'"
            ),
            8
        );
    }

    #[tokio::test]
    async fn an_invalid_key_fails_the_cohort_with_the_reason() {
        let e = env(8);
        let llm = ScriptedLlm::new(|_, _| Err(LlmError::Auth("API key not valid".into())));
        let err = run(
            job(&e),
            Arc::new(llm),
            limiter(2),
            e.writer.clone(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("API key not valid"));
        let conn = crate::db::open(&e.path).unwrap();
        let c = cohorts::get(&conn, e.cohort_id).unwrap();
        assert_eq!(c.status, CohortStatus::Failed);
        assert!(c.error.unwrap().contains("API key not valid"));
        assert_eq!(c.config, e.config);
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limits_are_waited_out() {
        let e = env(8);
        let llm = ScriptedLlm::new(|req, i| {
            if i < 2 {
                return Err(LlmError::RateLimited {
                    retry_after: Some(Duration::from_secs(3)),
                });
            }
            Ok(reply_for_ordinals(
                &ordinals_in_prompt(&req.prompt),
                Some("mobile_phone"),
                &[],
            ))
        });
        run(
            job(&e),
            Arc::new(llm.clone()),
            limiter(1),
            e.writer.clone(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert_eq!(llm.calls(), 3);
    }

    #[tokio::test]
    async fn concurrency_stays_within_the_limiter() {
        let e = env(40);
        let llm = good_llm().with_delay(Duration::from_millis(30));
        run(
            job(&e),
            Arc::new(llm.clone()),
            limiter(2),
            e.writer.clone(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert!(
            llm.max_in_flight() <= 2,
            "max in flight {}",
            llm.max_in_flight()
        );
        assert_eq!(count(&e, "SELECT COUNT(*) FROM respondents"), 40);
    }

    #[tokio::test]
    async fn summary_list_and_detail_read_back_the_cohort() {
        let e = env(20);
        run(
            job(&e),
            Arc::new(good_llm()),
            limiter(4),
            e.writer.clone(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        let conn = crate::db::open(&e.path).unwrap();
        let s = cohorts::summary(&conn, e.cohort_id, Some("mobile_phone")).unwrap();
        assert_eq!(s.respondents, 20);
        assert_eq!(s.top_trigger_label, "Upgrade trigger");
        assert!(s.top_trigger.is_some());
        let page = cohorts::list(&conn, e.cohort_id, "", 0, 12).unwrap();
        assert_eq!((page.items.len(), page.total), (12, 20));
        let found = cohorts::list(&conn, e.cohort_id, "Person 7", 0, 12).unwrap();
        assert_eq!(found.total, 1);
        let d = cohorts::detail(&conn, found.items[0].id).unwrap();
        assert_eq!(d.ordinal, 7);
        assert!(d.category_facts.iter().any(|(k, _)| k == "Upgrade trigger"));
        let locked = cohorts::lock(&conn, e.cohort_id).unwrap();
        assert_eq!(locked.status, CohortStatus::Locked);
    }
}
