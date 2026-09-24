//! Runs a benchmark pack: every respondent answers all pack questions in one call, with the
//! same answering prompt, shuffling and checks as a simulation run. Answers stay in memory;
//! benchmark runs are not project data.

use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::{answer_key, as_question, BenchAnswer, Pack, Person};
use crate::engine::answer::{build_request, check_reply, shown_order};
use crate::engine::call::with_retries;
use crate::engine::limiter::RateLimiter;
use crate::error::{AppError, AppResult};
use crate::llm::LlmProvider;
use crate::model::QuestionType;

#[derive(Debug, Clone, PartialEq)]
pub struct BenchRun {
    pub answers: Vec<BenchAnswer>,
    /// Answers that failed the checks (not scored).
    pub invalid: u32,
    /// Respondents whose call failed or was blocked.
    pub failed: u32,
}

pub async fn run(
    pack: &Pack,
    people: &[Person],
    llm: Arc<dyn LlmProvider>,
    limiter: Arc<RateLimiter>,
    model: &str,
    seed: u64,
    concurrency: usize,
) -> AppResult<BenchRun> {
    let pack = Arc::new(pack.clone());
    let slots = Arc::new(Semaphore::new(concurrency.max(1)));
    let mut tasks = JoinSet::new();
    for p in people.iter().cloned() {
        let (pack, llm, limiter, slots, model) = (
            pack.clone(),
            llm.clone(),
            limiter.clone(),
            slots.clone(),
            model.to_string(),
        );
        tasks.spawn(async move {
            let _slot = slots.acquire_owned().await.expect("semaphore open");
            let questions: Vec<_> = pack
                .questions
                .iter()
                .enumerate()
                .map(|(i, q)| as_question(q, i))
                .collect();
            let pairs: Vec<_> = questions
                .iter()
                .map(|q| (q, shown_order(q, seed, p.ordinal)))
                .collect();
            let req = build_request(&model, "", &pairs, &p.persona_text);
            let reply = with_retries(&llm, &limiter, &req, |_, _| {}).await;
            let mut out = Vec::new();
            let mut invalid = 0;
            match reply {
                Err(_) => return (out, 0, 1),
                Ok(r) => {
                    for (((q, shown), pq), res) in pairs
                        .iter()
                        .zip(&pack.questions)
                        .zip(check_reply(&pairs, &r.json))
                    {
                        let Ok(c) = res else {
                            invalid += 1;
                            continue;
                        };
                        let Some(key) = answer_key(pq, &c.answer_json) else {
                            invalid += 1;
                            continue;
                        };
                        let position = (q.body.question_type == QuestionType::SingleChoice)
                            .then(|| {
                                c.code
                                    .as_ref()
                                    .and_then(|code| shown.iter().position(|s| s == code))
                                    .map(|i| (i, shown.len()))
                            })
                            .flatten();
                        out.push(BenchAnswer {
                            person: p.id,
                            question: pq.code.clone(),
                            key,
                            position,
                        });
                    }
                }
            }
            (out, invalid, 0)
        });
    }
    let mut run = BenchRun {
        answers: Vec::new(),
        invalid: 0,
        failed: 0,
    };
    while let Some(res) = tasks.join_next().await {
        let (a, i, f) =
            res.map_err(|e| AppError::invalid(format!("benchmark task failed: {e}")))?;
        run.answers.extend(a);
        run.invalid += i;
        run.failed += f;
    }
    Ok(run)
}
