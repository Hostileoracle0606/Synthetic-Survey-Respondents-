//! The simulation job (docs/DATA_FLOW.md §3, Step 4): one `whole_survey` Gemini call per
//! respondent, run by a worker pool whose width adapts to 429s. Each respondent's answers
//! are saved together, so pausing, stopping or a crash never leaves a respondent half done,
//! and resuming skips everyone already saved.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::params;
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::answer::{self, build_request, check_reply, shown_order};
use super::call::{backoff, MAX_ATTEMPTS};
use super::limiter::{AdaptiveConcurrency, RateLimiter};
use crate::db::runs::{self, Outcome, PlannedRespondent, RunPlan, SavedAnswer};
use crate::db::writer::Writer;
use crate::error::AppResult;
use crate::llm::{LlmError, LlmProvider, StructuredRequest, Usage};
use crate::model::{
    AnswerDelta, ConsoleLine, EventLevel, ModelPrice, Question, RunProgress, RunStatus,
};

/// What the run should be doing; the UI's Pause and Stop buttons change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Run,
    Pause,
    Stop,
}

pub type ProgressFn = Arc<dyn Fn(RunProgress) + Send + Sync>;

pub const FLUSH_EVERY: Duration = Duration::from_millis(250);
const CONSOLE_PER_MESSAGE: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct RunOutcome {
    pub status: RunStatus,
    pub error: Option<String>,
    /// Lowest concurrency the 429 controller reached.
    pub min_concurrency: u32,
}

enum Update {
    Done {
        answers: u32,
        latency_ms: Option<u32>,
        usage: Option<Usage>,
        deltas: Vec<AnswerDelta>,
        console: Vec<ConsoleLine>,
    },
    Event(EventLevel, String),
}

struct Ctx {
    plan: RunPlan,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
    adaptive: Arc<AdaptiveConcurrency>,
    queue: Mutex<VecDeque<PlannedRespondent>>,
    control: watch::Receiver<Mode>,
    /// Set when the run must pause itself (bad key, quota spent, Gemini down).
    halt: Mutex<Option<String>>,
    updates: mpsc::UnboundedSender<Update>,
}

impl Ctx {
    fn keep_going(&self) -> bool {
        *self.control.borrow() == Mode::Run && self.halt.lock().unwrap().is_none()
    }

    fn event(&self, level: EventLevel, text: String) {
        let _ = self.updates.send(Update::Event(level, text));
    }
}

/// Error that stops the whole run (it is paused with this message).
struct Halt(String);

pub async fn run(
    mut plan: RunPlan,
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    writer: Writer,
    control: watch::Receiver<Mode>,
    progress: ProgressFn,
) -> AppResult<RunOutcome> {
    let run_id = plan.run_id;
    writer
        .write(Box::new(move |c| {
            runs::set_status(c, run_id, RunStatus::Running, None)
        }))
        .await?;
    progress(RunProgress::Status {
        status: RunStatus::Running,
    });

    let workers = plan.max_concurrency.max(1);
    let adaptive = AdaptiveConcurrency::new(workers);
    let (tx, rx) = mpsc::unbounded_channel();
    let queue = Mutex::new(std::mem::take(&mut plan.todo).into());
    let totals = Totals {
        answered: plan.answered,
        respondents_done: plan.respondents_done,
        total_answers: plan.respondents_total * plan.questions.len() as u32,
        input_tokens: 0,
        output_tokens: 0,
    };
    let price = plan.price;
    let aggregator = tokio::spawn(aggregate(
        rx,
        totals,
        adaptive.clone(),
        progress.clone(),
        price,
    ));
    let ctx = Arc::new(Ctx {
        plan,
        llm,
        limiter,
        writer: writer.clone(),
        adaptive: adaptive.clone(),
        queue,
        control,
        halt: Mutex::new(None),
        updates: tx,
    });

    let mut pool = JoinSet::new();
    for _ in 0..workers {
        let ctx = ctx.clone();
        pool.spawn(async move { worker(&ctx).await });
    }
    while pool.join_next().await.is_some() {}

    let halt = ctx.halt.lock().unwrap().clone();
    let mode = *ctx.control.borrow();
    let left = ctx.queue.lock().unwrap().len();
    if let Some(h) = &halt {
        ctx.event(EventLevel::Error, format!("Run paused: {h}"));
    }
    drop(ctx);
    let _ = aggregator.await;

    let status = match (&halt, mode, left) {
        (Some(_), _, _) => RunStatus::Paused,
        (None, Mode::Stop, _) => RunStatus::Stopped,
        (None, _, 0) => RunStatus::Completed,
        (None, _, _) => RunStatus::Paused,
    };
    let err = halt.clone();
    writer
        .write(Box::new(move |c| {
            runs::set_status(c, run_id, status, err.as_deref())
        }))
        .await?;
    progress(RunProgress::Status { status });
    Ok(RunOutcome {
        status,
        error: halt,
        min_concurrency: adaptive.min_seen(),
    })
}

async fn worker(ctx: &Ctx) {
    loop {
        if !ctx.keep_going() {
            return;
        }
        let Some(r) = ctx.queue.lock().unwrap().pop_front() else {
            return;
        };
        let slot = ctx.adaptive.acquire().await;
        if !ctx.keep_going() {
            ctx.queue.lock().unwrap().push_front(r);
            return;
        }
        let result = respondent(ctx, &r).await;
        drop(slot);
        if let Err(Halt(msg)) = result {
            ctx.queue.lock().unwrap().push_front(r);
            ctx.halt.lock().unwrap().get_or_insert(msg);
            return;
        }
    }
}

enum Reply {
    Got {
        json: Value,
        call: Option<(u32, Usage, u64)>,
    },
    Blocked(String),
}

/// Asks, checks, repairs once, saves. Returns only after the answers are committed.
async fn respondent(ctx: &Ctx, r: &PlannedRespondent) -> Result<(), Halt> {
    let p = &ctx.plan;
    let pairs: Vec<(&Question, Vec<String>)> = p
        .questions
        .iter()
        .map(|q| (q, shown_order(q, p.seed, r.ordinal)))
        .collect();
    let mut req = build_request(&p.model, &p.intro, &pairs, &r.persona);
    let (mut results, mut call) = match ask(ctx, &req, r).await? {
        Reply::Blocked(reason) => {
            ctx.event(
                EventLevel::Warn,
                format!(
                    "Respondent #{}: Gemini declined to answer ({reason}); stored as refused",
                    r.ordinal
                ),
            );
            let answers = pairs
                .iter()
                .map(|(q, shown)| SavedAnswer {
                    question_id: q.id,
                    shown: shown.clone(),
                    outcome: Outcome::Refused {
                        reason: reason.clone(),
                    },
                })
                .collect();
            return save(ctx, r, None, answers, Vec::new(), Vec::new()).await;
        }
        Reply::Got { json, call } => (check_reply(&pairs, &json), call),
    };
    let problems: Vec<String> = results.iter().filter_map(|x| x.clone().err()).collect();
    if !problems.is_empty() {
        req.prompt.push_str(&format!(
            "\nYour previous reply had problems: {}. Answer every question again, following the format exactly.\n",
            problems.join("; ")
        ));
        if let Reply::Got { json, call: c2 } = ask(ctx, &req, r).await? {
            let again = check_reply(&pairs, &json);
            for (old, new) in results.iter_mut().zip(again) {
                if old.is_err() && new.is_ok() {
                    *old = new;
                }
            }
            call = c2.or(call);
        }
    }

    let mut answers = Vec::new();
    let mut deltas = Vec::new();
    let mut console = Vec::new();
    let at = now_ms();
    for ((q, shown), res) in pairs.iter().zip(results) {
        let outcome = match res {
            Ok(c) => {
                deltas.push(AnswerDelta {
                    question_id: q.id,
                    respondent_id: r.id,
                    code: c.code.clone(),
                    codes: c.codes.clone(),
                    value: c.value,
                });
                console.push(ConsoleLine {
                    at: at.clone(),
                    respondent: r.ordinal,
                    question: q.code.clone(),
                    answer: answer::display(q, &c),
                    reason: c.reason.clone(),
                });
                Outcome::Valid(c)
            }
            Err(problem) => {
                console.push(ConsoleLine {
                    at: at.clone(),
                    respondent: r.ordinal,
                    question: q.code.clone(),
                    answer: "(invalid)".into(),
                    reason: problem.clone(),
                });
                Outcome::Invalid { problem }
            }
        };
        answers.push(SavedAnswer {
            question_id: q.id,
            shown: if q.body.question_type.is_choice() {
                shown.clone()
            } else {
                Vec::new()
            },
            outcome,
        });
    }
    save(ctx, r, call, answers, deltas, console).await
}

async fn save(
    ctx: &Ctx,
    r: &PlannedRespondent,
    call: Option<(u32, Usage, u64)>,
    answers: Vec<SavedAnswer>,
    deltas: Vec<AnswerDelta>,
    console: Vec<ConsoleLine>,
) -> Result<(), Halt> {
    let n = answers.len() as u32;
    let (run_id, respondent_id) = (ctx.plan.run_id, r.id);
    ctx.writer
        .write(Box::new(move |c| {
            runs::save_respondent(c, run_id, respondent_id, call, &answers)
        }))
        .await
        .map_err(|e| Halt(format!("could not save answers: {}", e.message)))?;
    let _ = ctx.updates.send(Update::Done {
        answers: n,
        latency_ms: call.map(|(_, _, l)| l as u32),
        usage: call.map(|(_, u, _)| u),
        deltas,
        console,
    });
    Ok(())
}

/// One request with retries. Rate limits and temporary failures are retried (and feed the
/// concurrency controller); a bad key, a spent quota or a Gemini outage halts the run.
async fn ask(ctx: &Ctx, req: &StructuredRequest, r: &PlannedRespondent) -> Result<Reply, Halt> {
    let estimate = ((req.system.len() + req.prompt.len()) / 4) as u32;
    for attempt in 1..=MAX_ATTEMPTS {
        let permit = ctx
            .limiter
            .acquire(estimate)
            .await
            .map_err(|e| Halt(e.message))?;
        let result = ctx.llm.complete_structured(req).await;
        if let Some(limit) = ctx
            .adaptive
            .record(matches!(result, Err(LlmError::RateLimited { .. })))
        {
            ctx.event(EventLevel::Info, format!("Concurrency now {limit}"));
        }
        match result {
            Ok(resp) => {
                ctx.limiter
                    .record_actual(&permit, resp.usage.input_tokens)
                    .await;
                return Ok(Reply::Got {
                    json: resp.json,
                    call: Some((attempt, resp.usage, resp.latency_ms)),
                });
            }
            Err(e) => {
                log_failed(ctx, r.id, attempt, &e);
                drop(permit);
                match e {
                    LlmError::RateLimited { retry_after } if attempt < MAX_ATTEMPTS => {
                        let wait = backoff(attempt, retry_after);
                        ctx.event(
                            EventLevel::Warn,
                            format!(
                                "Rate limited (respondent #{}); retrying in {:.1}s",
                                r.ordinal,
                                wait.as_secs_f32()
                            ),
                        );
                        tokio::time::sleep(wait).await;
                    }
                    LlmError::Transient(msg) if attempt < MAX_ATTEMPTS => {
                        let wait = backoff(attempt, None);
                        ctx.event(
                            EventLevel::Warn,
                            format!(
                                "Gemini error (respondent #{}): {msg}; retrying in {}s",
                                r.ordinal,
                                wait.as_secs()
                            ),
                        );
                        tokio::time::sleep(wait).await;
                    }
                    // Unreadable reply: treat it as all answers invalid, so the repair retry runs.
                    LlmError::SchemaViolation(_) => {
                        return Ok(Reply::Got {
                            json: Value::Null,
                            call: None,
                        })
                    }
                    LlmError::Blocked(reason) => return Ok(Reply::Blocked(reason)),
                    LlmError::Auth(m) => {
                        return Err(Halt(format!("Gemini rejected the API key ({m})")))
                    }
                    LlmError::InvalidRequest(m) => {
                        return Err(Halt(format!("Gemini refused the request ({m})")))
                    }
                    LlmError::RateLimited { .. } | LlmError::Transient(_) => {
                        return Err(Halt("Gemini kept failing; try resuming later".into()))
                    }
                }
            }
        }
    }
    Err(Halt("Gemini kept failing; try resuming later".into()))
}

fn log_failed(ctx: &Ctx, respondent_id: i64, attempt: u32, e: &LlmError) {
    let run_id = ctx.plan.run_id;
    let status = matches!(e, LlmError::RateLimited { .. }).then_some(429);
    let text = e.to_string();
    let _ = ctx.writer.send(Box::new(move |c| {
        c.execute(
            "INSERT INTO llm_calls(run_id, respondent_id, purpose, attempt, http_status, error) VALUES (?1, ?2, 'answer', ?3, ?4, ?5)",
            params![run_id, respondent_id, attempt, status, text],
        )?;
        Ok(())
    }));
}

/// Milliseconds since the Unix epoch, as text; the UI formats it in local time.
fn now_ms() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

struct Totals {
    answered: u32,
    respondents_done: u32,
    total_answers: u32,
    input_tokens: u64,
    output_tokens: u64,
}

/// Collects updates and sends one `Batch` per `FLUSH_EVERY` at most; events go straight out.
/// `price` (BACKLOG B1, from Settings) turns the running token totals into a live USD estimate;
/// it stays `None` (shown as "$—") until the user fills in the Gemini price table.
async fn aggregate(
    mut rx: mpsc::UnboundedReceiver<Update>,
    mut t: Totals,
    adaptive: Arc<AdaptiveConcurrency>,
    progress: ProgressFn,
    price: Option<ModelPrice>,
) {
    let mut latencies: Vec<u32> = Vec::new();
    let mut recent: VecDeque<(Instant, u32)> = VecDeque::new();
    let mut deltas = Vec::new();
    let mut console: VecDeque<ConsoleLine> = VecDeque::new();
    let mut dirty = false;
    let mut last_flush: Option<Instant> = None;
    let mut tick = tokio::time::interval(FLUSH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let open = tokio::select! {
            u = rx.recv() => match u {
                Some(Update::Done { answers, latency_ms, usage, deltas: d, console: c }) => {
                    t.answered += answers;
                    t.respondents_done += 1;
                    latencies.extend(latency_ms);
                    recent.push_back((Instant::now(), answers));
                    if let Some(u) = usage {
                        t.input_tokens += u64::from(u.input_tokens);
                        t.output_tokens += u64::from(u.output_tokens);
                    }
                    deltas.extend(d);
                    console.extend(c);
                    while console.len() > CONSOLE_PER_MESSAGE {
                        console.pop_front();
                    }
                    dirty = true;
                    true
                }
                Some(Update::Event(level, text)) => {
                    progress(RunProgress::Event { level, text });
                    true
                }
                None => false,
            },
            _ = tick.tick() => true,
        };
        let due = !open || last_flush.map_or(true, |t| t.elapsed() >= FLUSH_EVERY);
        if dirty && due {
            last_flush = Some(Instant::now());
            let cutoff = Instant::now().checked_sub(Duration::from_secs(60));
            while recent
                .front()
                .is_some_and(|(at, _)| cutoff.is_some_and(|c| *at < c))
            {
                recent.pop_front();
            }
            let (avg, p95) = latency_stats(&latencies);
            let cost_usd = price.map(|p| {
                (t.input_tokens as f64 / 1e6) * p.input_usd_per_million
                    + (t.output_tokens as f64 / 1e6) * p.output_usd_per_million
            });
            progress(RunProgress::Batch {
                answered: t.answered,
                total_answers: t.total_answers,
                respondents_done: t.respondents_done,
                cost_usd,
                avg_latency_ms: avg,
                p95_latency_ms: p95,
                answers_per_min: recent.iter().map(|(_, n)| n).sum(),
                concurrency: adaptive.current(),
                deltas: std::mem::take(&mut deltas),
                console: console.drain(..).collect(),
            });
            dirty = false;
        }
        if !open {
            return;
        }
    }
}

fn latency_stats(v: &[u32]) -> (u32, u32) {
    if v.is_empty() {
        return (0, 0);
    }
    let mut s = v.to_vec();
    s.sort_unstable();
    let avg = (s.iter().map(|x| u64::from(*x)).sum::<u64>() / s.len() as u64) as u32;
    let p95 = s[((s.len() * 95).div_ceil(100)).saturating_sub(1)];
    (avg, p95)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::runs::RunSettings;
    use crate::db::surveys::{self, NewQuestion};
    use crate::db::{cohorts, open};
    use crate::engine::answer::reply_for_prompt;
    use crate::engine::limiter::Limits;
    use crate::engine::persona::{self, PROMPT_VERSION};
    use crate::error::ErrorCode;
    use crate::llm::scripted::ScriptedLlm;
    use crate::model::{
        ChoiceOption, CohortConfig, NumericRange, QuestionBody, QuestionType, QuotaGroup, QuotaRow,
        RunConfig, Scale,
    };
    use crate::sampling::Sampler;
    use rusqlite::Connection;

    struct Env {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        writer: Writer,
    }

    impl Env {
        fn conn(&self) -> Connection {
            open(&self.path).unwrap()
        }
        fn count(&self, sql: &str) -> i64 {
            self.conn().query_row(sql, [], |r| r.get(0)).unwrap()
        }
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
            text: format!("Question {i}?"),
            question_type: t,
            options: ["Apple", "Samsung", "Google", "Other"]
                .iter()
                .map(|l| ChoiceOption {
                    code: String::new(),
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
                max: 2000.0,
                unit: "CAD".into(),
            }),
        }
    }

    /// A project with a locked cohort of `n` people and a survey of `q` accepted questions.
    fn env(n: u32, q: usize) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.db");
        let conn = open(&path).unwrap();
        conn.execute("INSERT INTO projects(title, research_type, countries_json) VALUES ('p', 'market_response', '[\"CA\"]')", [])
            .unwrap();
        let config = CohortConfig {
            size: n,
            seed: 5,
            quotas: vec![QuotaGroup {
                key: "age".into(),
                label: "Age".into(),
                rows: ["18–29", "30–44", "45–59", "60+"]
                    .iter()
                    .map(|l| QuotaRow {
                        label: l.to_string(),
                        percent: 25,
                    })
                    .collect(),
            }],
            screening: String::new(),
        };
        let cohort = cohorts::create(&conn, 1, &config, None, "flash", PROMPT_VERSION).unwrap();
        let skels = Sampler::new(&config, &["CA".into()])
            .unwrap()
            .draw()
            .unwrap();
        let reply = persona::sample_reply(&skels, Some("mobile_phone"), &[]);
        let ordinals: Vec<u32> = skels.iter().map(|s| s.ordinal).collect();
        let people = persona::parse_reply(&reply, &ordinals, Some("mobile_phone")).unwrap();
        for (s, p) in skels.iter().zip(&people) {
            cohorts::insert_respondent(&conn, cohort.id, s, p, "passed").unwrap();
        }
        cohorts::set_status(&conn, cohort.id, crate::model::CohortStatus::Ready, None).unwrap();
        cohorts::lock(&conn, cohort.id).unwrap();
        let survey = surveys::for_project(&conn, 1).unwrap();
        for i in 0..q {
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
        let (writer, _h) = Writer::spawn(open(&path).unwrap(), 50, Duration::from_millis(10));
        Env {
            _dir: dir,
            path,
            writer,
        }
    }

    fn settings(conc: u32) -> RunSettings<'static> {
        RunSettings {
            model: "flash",
            max_concurrency: conc,
            requests_left_today: 100_000,
        }
    }

    fn limiter(conc: u32) -> Arc<RateLimiter> {
        Arc::new(RateLimiter::new(
            Limits {
                requests_per_minute: 100_000,
                tokens_per_minute: 1_000_000_000,
                requests_per_day: 100_000,
                max_concurrency: conc,
            },
            0,
        ))
    }

    fn good() -> ScriptedLlm {
        ScriptedLlm::new(|req, _| Ok(reply_for_prompt(&req.prompt)))
    }

    type Seen = Arc<Mutex<Vec<RunProgress>>>;

    async fn go(
        e: &Env,
        run_id: i64,
        llm: &ScriptedLlm,
        conc: u32,
        control: watch::Receiver<Mode>,
    ) -> (RunOutcome, Seen) {
        let plan = runs::plan(&e.conn(), run_id).unwrap();
        let seen: Seen = Arc::default();
        let s2 = seen.clone();
        let out = run(
            plan,
            Arc::new(llm.clone()),
            limiter(conc),
            e.writer.clone(),
            control,
            Arc::new(move |p| s2.lock().unwrap().push(p)),
        )
        .await
        .unwrap();
        (out, seen)
    }

    fn start(e: &Env, conc: u32) -> i64 {
        runs::start(&e.conn(), 1, &RunConfig { seed: None }, &settings(conc))
            .unwrap()
            .id
    }

    #[test]
    fn a_run_on_an_unapproved_survey_is_refused_by_the_database() {
        let e = env(4, 3);
        let conn = e.conn();
        conn.execute(
            "UPDATE questions SET review_status = 'pending' WHERE code = 'Q2'",
            [],
        )
        .unwrap();
        let err = runs::start(&conn, 1, &RunConfig { seed: None }, &settings(4))
            .err()
            .unwrap();
        assert_eq!(err.code, ErrorCode::SurveyNotApproved);
        // The approval rolled back with the refused insert.
        assert_eq!(
            e.count("SELECT COUNT(*) FROM surveys WHERE status = 'approved'"),
            0
        );
        assert_eq!(e.count("SELECT COUNT(*) FROM simulation_runs"), 0);
        // Even a direct insert is refused.
        assert!(conn
            .execute("INSERT INTO simulation_runs(project_id, survey_id, cohort_id, survey_hash, provider, model, temperature, answer_mode, prompt_version, seed, max_concurrency) VALUES (1, 1, 1, 'h', 'gemini', 'm', 1, 'whole_survey', 'v', 1, 1)", [])
            .is_err());
    }

    #[test]
    fn start_checks_the_daily_quota_first() {
        let e = env(10, 2);
        let s = RunSettings {
            model: "flash",
            max_concurrency: 4,
            requests_left_today: 5,
        };
        let err = runs::start(&e.conn(), 1, &RunConfig { seed: None }, &s)
            .err()
            .unwrap();
        assert!(
            err.message.contains("only 5 are left today"),
            "{}",
            err.message
        );
        assert_eq!(e.count("SELECT COUNT(*) FROM simulation_runs"), 0);
    }

    /// SPEC §11: 100 respondents × 20 questions at 10 concurrent calls. With 300 ms per call
    /// the ideal is 3 s; engine overhead must stay small (Gemini itself takes ~5–10 s a call,
    /// which puts the real run well under the 3-minute budget).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn hundred_by_twenty_runs_ten_wide_and_saves_every_answer() {
        let e = env(100, 20);
        let id = start(&e, 10);
        let llm = good().with_delay(Duration::from_millis(300));
        let (_tx, rx) = watch::channel(Mode::Run);
        let started = std::time::Instant::now();
        let (out, seen) = go(&e, id, &llm, 10, rx).await;
        let took = started.elapsed();
        assert_eq!(out.status, RunStatus::Completed);
        assert!(took < Duration::from_millis(5_000), "took {took:?}");
        assert_eq!(llm.calls(), 100);
        assert_eq!(llm.max_in_flight(), 10);
        assert_eq!(
            e.count("SELECT COUNT(*) FROM responses WHERE status = 'valid'"),
            2000
        );
        assert_eq!(
            e.count("SELECT COUNT(*) FROM llm_calls WHERE purpose = 'answer'"),
            100
        );
        assert_eq!(e.count("SELECT COUNT(*) FROM simulation_runs WHERE status = 'completed' AND finished_at IS NOT NULL"), 1);
        let seen = seen.lock().unwrap();
        let batches: Vec<_> = seen
            .iter()
            .filter(|p| matches!(p, RunProgress::Batch { .. }))
            .collect();
        // At most one batch per 250 ms (plus the final flush).
        assert!(
            batches.len() as u128 <= took.as_millis() / 250 + 2,
            "{} batches in {took:?}",
            batches.len()
        );
        match batches.last().unwrap() {
            RunProgress::Batch {
                answered,
                total_answers,
                respondents_done,
                console,
                ..
            } => {
                assert_eq!(
                    (*answered, *total_answers, *respondents_done),
                    (2000, 2000, 100)
                );
                assert!(console.len() <= 20);
            }
            _ => unreachable!(),
        }
        let deltas: usize = batches
            .iter()
            .map(|b| match b {
                RunProgress::Batch { deltas, .. } => deltas.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(deltas, 2000);
        assert_eq!(
            seen.last().unwrap(),
            &RunProgress::Status {
                status: RunStatus::Completed
            }
        );
    }

    /// SPEC §11: a forced 30% rate of 429s still completes the run, with concurrency reduced.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn thirty_percent_429s_still_complete_with_lower_concurrency() {
        let e = env(60, 5);
        let id = start(&e, 8);
        let llm = ScriptedLlm::new(|req, i| {
            if [0, 3, 6].contains(&(i % 10)) {
                Err(LlmError::RateLimited {
                    retry_after: Some(Duration::from_millis(20)),
                })
            } else {
                Ok(reply_for_prompt(&req.prompt))
            }
        })
        .with_delay(Duration::from_millis(20));
        let (_tx, rx) = watch::channel(Mode::Run);
        let (out, seen) = go(&e, id, &llm, 8, rx).await;
        assert_eq!(out.status, RunStatus::Completed);
        assert!(out.min_concurrency < 8, "concurrency never went down");
        assert_eq!(
            e.count("SELECT COUNT(*) FROM responses WHERE status = 'valid'"),
            300
        );
        assert!(e.count("SELECT COUNT(*) FROM llm_calls WHERE http_status = 429") > 10);
        assert!(seen.lock().unwrap().iter().any(
            |p| matches!(p, RunProgress::Event { text, .. } if text.starts_with("Rate limited"))
        ));
    }

    /// SPEC §11: killing the app mid-run and relaunching resumes with no duplicate or lost answers.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn kill_and_relaunch_resumes_without_duplicates_or_gaps() {
        let e = env(40, 5);
        let id = start(&e, 4);
        let llm = good().with_delay(Duration::from_millis(80));
        let plan = runs::plan(&e.conn(), id).unwrap();
        let (_tx, rx) = watch::channel(Mode::Run);
        let job = tokio::spawn(run(
            plan,
            Arc::new(llm.clone()),
            limiter(4),
            e.writer.clone(),
            rx,
            Arc::new(|_| {}),
        ));
        tokio::time::sleep(Duration::from_millis(300)).await;
        job.abort(); // the app dies: in-flight calls are lost
        let _ = job.await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let partial = e.count("SELECT COUNT(DISTINCT respondent_id) FROM responses");
        assert!(partial > 0 && partial < 40, "partial = {partial}");
        // Every saved respondent is complete.
        assert_eq!(e.count("SELECT COUNT(*) FROM responses"), partial * 5);

        // Relaunch.
        assert_eq!(runs::recover_on_launch(&e.conn()).unwrap(), 1);
        assert_eq!(runs::get(&e.conn(), id).unwrap().status, RunStatus::Paused);
        let (_tx, rx) = watch::channel(Mode::Run);
        let (out, _) = go(&e, id, &llm, 4, rx).await;
        assert_eq!(out.status, RunStatus::Completed);
        assert_eq!(e.count("SELECT COUNT(*) FROM responses"), 200);
        assert_eq!(
            e.count(
                "SELECT COUNT(*) FROM (SELECT DISTINCT respondent_id, question_id FROM responses)"
            ),
            200
        );
        let run = runs::get(&e.conn(), id).unwrap();
        assert_eq!(
            (run.respondents_done, run.answered, run.error),
            (40, 200, None)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pause_lets_calls_finish_then_resume_completes_and_stop_is_final() {
        let e = env(30, 3);
        let id = start(&e, 3);
        let llm = good().with_delay(Duration::from_millis(60));
        let (tx, rx) = watch::channel(Mode::Run);
        let plan = runs::plan(&e.conn(), id).unwrap();
        let job = tokio::spawn(run(
            plan,
            Arc::new(llm.clone()),
            limiter(3),
            e.writer.clone(),
            rx,
            Arc::new(|_| {}),
        ));
        tokio::time::sleep(Duration::from_millis(150)).await;
        tx.send(Mode::Pause).unwrap();
        let out = job.await.unwrap().unwrap();
        assert_eq!(out.status, RunStatus::Paused);
        let done = e.count("SELECT COUNT(DISTINCT respondent_id) FROM responses");
        // Every call that started was saved.
        assert_eq!(done as usize, llm.calls());
        assert!(done < 30);

        let (tx, rx) = watch::channel(Mode::Run);
        let plan = runs::plan(&e.conn(), id).unwrap();
        let job = tokio::spawn(run(
            plan,
            Arc::new(llm.clone()),
            limiter(3),
            e.writer.clone(),
            rx,
            Arc::new(|_| {}),
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(Mode::Stop).unwrap();
        let out = job.await.unwrap().unwrap();
        assert_eq!(out.status, RunStatus::Stopped);
        let stopped_at = e.count("SELECT COUNT(*) FROM responses");
        assert!(stopped_at < 90);
        assert_eq!(runs::get(&e.conn(), id).unwrap().status, RunStatus::Stopped);
        // A stopped run can't be restarted by accident: a new run is needed.
        assert!(runs::start(&e.conn(), 1, &RunConfig { seed: None }, &settings(3)).is_ok());
    }

    #[tokio::test]
    async fn bad_answers_are_retried_once_then_stored_as_invalid() {
        let e = env(6, 3);
        let id = start(&e, 2);
        // Always leaves out Q2.
        let llm = ScriptedLlm::new(|req, _| {
            let mut v = reply_for_prompt(&req.prompt);
            v.as_object_mut().unwrap().remove("Q2");
            Ok(v)
        });
        let (_tx, rx) = watch::channel(Mode::Run);
        let (out, _) = go(&e, id, &llm, 2, rx).await;
        assert_eq!(out.status, RunStatus::Completed);
        assert_eq!(llm.calls(), 12);
        assert!(llm.requests()[1].prompt.contains("Q2: no answer"));
        assert_eq!(
            e.count("SELECT COUNT(*) FROM responses WHERE status = 'invalid'"),
            6
        );
        assert_eq!(
            e.count("SELECT COUNT(*) FROM responses WHERE status = 'valid'"),
            12
        );
    }

    #[tokio::test]
    async fn safety_blocks_are_stored_as_refused() {
        let e = env(3, 2);
        let id = start(&e, 1);
        let llm = ScriptedLlm::new(|req, i| {
            if i == 0 {
                Err(LlmError::Blocked("SAFETY".into()))
            } else {
                Ok(reply_for_prompt(&req.prompt))
            }
        });
        let (_tx, rx) = watch::channel(Mode::Run);
        let (out, _) = go(&e, id, &llm, 1, rx).await;
        assert_eq!(out.status, RunStatus::Completed);
        assert_eq!(
            e.count("SELECT COUNT(*) FROM responses WHERE status = 'refused'"),
            2
        );
    }

    #[tokio::test]
    async fn a_rejected_key_pauses_the_run_with_the_reason() {
        let e = env(5, 2);
        let id = start(&e, 2);
        let llm = ScriptedLlm::new(|_, _| Err(LlmError::Auth("API_KEY_INVALID".into())));
        let (_tx, rx) = watch::channel(Mode::Run);
        let (out, seen) = go(&e, id, &llm, 2, rx).await;
        assert_eq!(out.status, RunStatus::Paused);
        let run = runs::get(&e.conn(), id).unwrap();
        assert!(run.error.unwrap().contains("API key"));
        assert_eq!(run.answered, 0);
        assert!(seen.lock().unwrap().iter().any(|p| matches!(
            p,
            RunProgress::Event {
                level: EventLevel::Error,
                ..
            }
        )));
    }

    /// A model that always picks whatever is shown first is caught by the order-effect check,
    /// because options were shuffled per respondent and the order was stored.
    #[tokio::test]
    async fn always_picking_the_first_option_is_flagged_as_an_order_effect() {
        let e = env(60, 1);
        let id = start(&e, 4);
        let (_tx, rx) = watch::channel(Mode::Run);
        go(&e, id, &good(), 4, rx).await; // reply_for_prompt always answers option 1
        let rep = crate::report::report(&e.conn(), id).unwrap();
        let v = &rep.questions[0].validity;
        assert_eq!(v.first_position_rate, Some(100.0));
        assert_eq!(v.last_position_rate, Some(0.0));
        assert!(
            v.flags.iter().any(|f| f.starts_with("Order effect")),
            "{:?}",
            v.flags
        );
        // Shuffling spread the chosen options, so variance alone looks healthy.
        assert!(v.entropy.unwrap() > 0.8);
    }

    /// Review fixes: a paused run can't resume on an edited survey; a question with answers
    /// can't be edited in place; a report lists only what its run asked; launch recovery
    /// unsticks drafts and syntheses left generating.
    #[tokio::test]
    async fn runs_stay_tied_to_the_survey_they_asked() {
        let e = env(6, 2);
        let id = start(&e, 2);
        let (_tx, rx) = watch::channel(Mode::Run);
        go(&e, id, &good(), 2, rx).await;
        let conn = e.conn();
        let q1 = conn
            .query_row("SELECT id FROM questions WHERE code = 'Q1'", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();

        // Editing an answered question in place is refused; deleting retires it instead.
        let mut body = surveys::question(&conn, q1).unwrap().body;
        body.text = "Changed?".into();
        let err = surveys::update_question(&conn, q1, body).err().unwrap();
        assert!(err.message.contains("already has answers"));

        // A question added afterwards doesn't appear in this run's report.
        let survey_id = surveys::for_project(&conn, 1).unwrap().id;
        let added = surveys::add_question(&conn, survey_id).unwrap();
        let rep = crate::report::report(&conn, id).unwrap();
        assert_eq!(rep.questions.len(), 2);
        assert!(rep.questions.iter().all(|q| q.question_id != added.id));

        // Resuming (re-planning) this run on the changed survey is refused.
        conn.execute(
            "UPDATE simulation_runs SET status = 'paused' WHERE id = ?1",
            [id],
        )
        .unwrap();
        assert!(runs::plan(&conn, id)
            .err()
            .unwrap()
            .message
            .contains("survey changed"));

        // Launch recovery unsticks background jobs left generating.
        conn.execute_batch(
            "UPDATE surveys SET draft_status = 'generating'; UPDATE simulation_runs SET synthesis_status = 'generating';",
        )
        .unwrap();
        runs::recover_on_launch(&conn).unwrap();
        assert_eq!(
            surveys::get(&conn, survey_id).unwrap().draft_status,
            crate::model::DraftStatus::Failed
        );
        let rep = crate::report::report(&conn, id).unwrap();
        assert_eq!(rep.synthesis_status, crate::model::SynthesisStatus::Failed);
        assert!(rep.synthesis_error.unwrap().contains("app closed"));
    }

    /// SPEC §11: the same run config and seed gives identical option orders.
    #[tokio::test]
    async fn same_seed_gives_the_same_option_orders() {
        let e = env(8, 4);
        let (_tx, rx) = watch::channel(Mode::Run);
        let a = start(&e, 2);
        go(&e, a, &good(), 2, rx.clone()).await;
        let b = start(&e, 2);
        go(&e, b, &good(), 2, rx).await;
        let orders = |run: i64| -> Vec<Option<String>> {
            e.conn()
                .prepare("SELECT shown_options_json FROM responses WHERE run_id = ?1 ORDER BY respondent_id, question_id")
                .unwrap()
                .query_map([run], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(orders(a), orders(b));
        assert!(orders(a).iter().flatten().count() >= 8);
    }

    /// SPEC §11: no API key appears in the database, logs or exports. A run, theme coding,
    /// synthesis and both exports go through the real Gemini client (against a local mock
    /// server) with a recognisable key; then every file is searched for it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_api_key_in_the_database_logs_or_exports() {
        use crate::engine::synthesis::{self, SynthesisJob};
        use crate::llm::gemini::GeminiClient;
        use serde_json::json;
        use wiremock::matchers::{header, method};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        const KEY: &str = "AIzaLEAKCHECK_0123456789_abcdefghijklmn";
        struct Gemini;
        impl Respond for Gemini {
            fn respond(&self, req: &Request) -> ResponseTemplate {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let system = body["systemInstruction"]["parts"][0]["text"]
                    .as_str()
                    .unwrap_or_default();
                let prompt = body["contents"][0]["parts"][0]["text"]
                    .as_str()
                    .unwrap_or_default();
                let reply = if system.contains("survey as the person") {
                    reply_for_prompt(prompt)
                } else if system.contains("code open-ended") {
                    json!({"themes": [{"label": "Price", "description": "d", "answers": [1, 2]}]})
                } else {
                    json!({"summary": "Answers were mixed.", "friction_points": [], "segments": []})
                };
                ResponseTemplate::new(200).set_body_json(json!({
                    "candidates": [{ "content": { "parts": [{ "text": reply.to_string() }] }, "finishReason": "STOP" }],
                    "usageMetadata": { "promptTokenCount": 100, "candidatesTokenCount": 50 }
                }))
            }
        }
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("x-goog-api-key", KEY))
            .respond_with(Gemini)
            .mount(&server)
            .await;

        let e = env(6, 5); // includes an open-ended question
        let id = start(&e, 2);
        let llm: Arc<dyn LlmProvider> = Arc::new(GeminiClient::with_base_url(KEY, server.uri()));
        let (_tx, rx) = watch::channel(Mode::Run);
        let plan = runs::plan(&e.conn(), id).unwrap();
        let out = run(
            plan,
            llm.clone(),
            limiter(2),
            e.writer.clone(),
            rx,
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
        assert_eq!(out.status, RunStatus::Completed);
        synthesis::run(
            SynthesisJob {
                run_id: id,
                model: "flash".into(),
                db_path: e.path.clone(),
                recode_themes: false,
            },
            llm.clone(),
            limiter(2),
            e.writer.clone(),
        )
        .await
        .unwrap();
        // A rejected call's error text is stored too; it must not carry the key either.
        let bad = GeminiClient::with_base_url(KEY, "http://127.0.0.1:9")
            .complete_structured(&build_request("m", "", &[], ""))
            .await;
        let err = bad.unwrap_err().to_string();
        assert!(!err.contains(KEY));
        assert!(!format!("{:?}", GeminiClient::new(KEY)).contains(KEY));

        let conn = e.conn();
        let csv = crate::report::export::csv(&conn, id).unwrap();
        let json_export = crate::report::export::json(&conn, id).unwrap().to_string();
        assert!(csv.lines().count() == 7 && json_export.contains("Price"));
        drop(conn);
        let mut files = vec![csv.into_bytes(), json_export.into_bytes()];
        for suffix in ["", "-wal", "-shm"] {
            if let Ok(bytes) = std::fs::read(format!("{}{suffix}", e.path.display())) {
                files.push(bytes);
            }
        }
        for f in &files {
            assert!(
                !f.windows(KEY.len()).any(|w| w == KEY.as_bytes()),
                "API key found in an output"
            );
        }
        assert!(files.len() >= 3);
    }
}
