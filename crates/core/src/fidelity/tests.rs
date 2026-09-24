use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::engine::limiter::{Limits, RateLimiter};
use crate::llm::scripted::ScriptedLlm;

const PACK: &str = include_str!("../../../../benchmarks/fidelity.v1.json");

fn pack() -> Pack {
    load_pack(PACK).unwrap()
}

/// 300 people whose attributes vary independently.
fn people() -> Vec<Person> {
    (1..=300i64)
        .map(|id| {
            let mut attrs: HashMap<String, Value> = HashMap::new();
            let age = [22u32, 35, 50, 75][(id % 4) as usize];
            attrs.insert("age".into(), json!(age));
            attrs.insert("age_band".into(), json!(crate::report::age_band(age)));
            attrs.insert(
                "gender".into(),
                json!(if id % 2 == 0 { "Female" } else { "Male" }),
            );
            attrs.insert(
                "region".into(),
                json!(["Ontario", "Quebec", "Prairies"][(id % 3) as usize]),
            );
            attrs.insert(
                "income".into(),
                json!(["Under $50k", "$50k–$100k", "Over $100k"][((id / 3) % 3) as usize]),
            );
            attrs.insert("price_sensitivity".into(), json!((id * 7 % 5) + 1));
            let bias = ["Early adopter", "Brand-loyal", "Status quo"][((id / 5) % 3) as usize];
            attrs.insert("biases".into(), json!([bias]));
            attrs.insert("values".into(), json!(["Family"]));
            Person {
                id,
                ordinal: id as u32,
                attrs,
                persona_text: format!("Person #{id}\n"),
            }
        })
        .collect()
}

fn limiter() -> Arc<RateLimiter> {
    Arc::new(RateLimiter::new(
        Limits {
            requests_per_minute: 100_000,
            tokens_per_minute: 1_000_000_000,
            requests_per_day: 100_000,
            max_concurrency: 16,
        },
        0,
    ))
}

/// Deterministic pseudo-random draw in [0, 1) per person and question.
fn draw(id: i64, code: &str) -> f64 {
    let h = code
        .bytes()
        .fold(7u64, |a, b| a.wrapping_mul(31).wrapping_add(u64::from(b)));
    let x = (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    ((x >> 11) % 10_000) as f64 / 10_000.0
}

/// The numbered position of option `code` for question `q` in this prompt.
fn position_in_prompt(prompt: &str, q: &PackQuestion, code: &str) -> usize {
    let label = &q
        .question
        .options
        .iter()
        .find(|o| o.code == code)
        .unwrap()
        .label;
    let start = prompt.find(&format!("[{}]", q.code)).unwrap();
    let block = &prompt[start..];
    let line = block
        .lines()
        .find(|l| {
            l.trim_start()
                .split_once(". ")
                .is_some_and(|(_, l2)| l2 == label)
        })
        .unwrap();
    line.trim_start()
        .split_once('.')
        .unwrap()
        .0
        .parse()
        .unwrap()
}

/// Answers as each persona's rule says, sampling so that answers follow the distribution.
fn faithful(pack: Pack, people: Vec<Person>) -> ScriptedLlm {
    let by_id: HashMap<i64, Person> = people.into_iter().map(|p| (p.id, p)).collect();
    ScriptedLlm::new(move |req, _| {
        let id: i64 = req
            .prompt
            .split("Person #")
            .nth(1)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let p = &by_id[&id];
        let answers: Vec<Value> = pack
            .questions
            .iter()
            .map(|q| {
                let dist = q.rule.expected_for(p);
                let u = draw(id, &q.code);
                let mut acc = 0.0;
                let key = dist.iter().find(|(_, pr)| { acc += **pr; u < acc }).map_or_else(|| dist.keys().last().unwrap().clone(), |(k, _)| k.clone());
                match q.question.question_type {
                    QuestionType::SingleChoice => json!({"code": q.code, "choice": position_in_prompt(&req.prompt, q, &key), "reason": "r"}),
                    QuestionType::Likert => json!({"code": q.code, "value": key.parse::<f64>().unwrap(), "reason": "r"}),
                    _ => {
                        let (lo, hi) = q.rule.bins[key.parse::<usize>().unwrap()];
                        json!({"code": q.code, "value": (lo + hi) / 2.0, "reason": "r"})
                    }
                }
            })
            .collect();
        Ok(json!({ "answers": answers }))
    })
}

/// Ignores the persona: first option shown, scale midpoint, a fixed number.
fn ignorant(pack: Pack) -> ScriptedLlm {
    ScriptedLlm::new(move |_, _| {
        let answers: Vec<Value> = pack
            .questions
            .iter()
            .map(|q| match q.question.question_type {
                QuestionType::SingleChoice => json!({"code": q.code, "choice": 1, "reason": "r"}),
                QuestionType::Likert => json!({"code": q.code, "value": 3, "reason": "r"}),
                _ => json!({"code": q.code, "value": 600, "reason": "r"}),
            })
            .collect();
        Ok(json!({ "answers": answers }))
    })
}

async fn bench(llm: ScriptedLlm) -> FidelityReport {
    let (pk, ppl) = (pack(), people());
    let run = runner::run(&pk, &ppl, Arc::new(llm), limiter(), "test-flash", 11, 8)
        .await
        .unwrap();
    assert_eq!((run.invalid, run.failed), (0, 0));
    assert_eq!(run.answers.len(), 300 * pk.questions.len());
    score(
        &pk,
        &ppl,
        &run.answers,
        ("test-flash", crate::engine::answer::PROMPT_VERSION, 11),
    )
}

#[test]
fn the_starter_pack_is_valid_but_not_releasable() {
    let p = pack();
    assert_eq!(p.questions.len(), 6);
    validate(&p, false).unwrap();
    let err = validate(&p, true).unwrap_err();
    assert!(err[0].contains("frozen"));
}

#[test]
fn validation_catches_broken_rules() {
    let mut p = pack();
    p.questions[0].rule.otherwise.insert("A".into(), 0.9); // no longer sums to 1
    p.questions[1].rule.cases[0].expect.insert("9".into(), 0.0); // not a scale point
    p.questions[2].question.text = "Given your income, what would you spend?".into(); // names the attribute
    p.questions[3].rule_author.clear();
    p.questions[4].attribute = "shoe_size".into();
    let problems = validate(&p, false).unwrap_err();
    for needle in [
        "sums to",
        "\"9\" is not an answer",
        "names the attribute",
        "rule_author",
        "unknown attribute",
    ] {
        assert!(
            problems.iter().any(|x| x.contains(needle)),
            "missing {needle}: {problems:?}"
        );
    }
    // "age" must match as a word, not inside "usage".
    let mut ok = pack();
    ok.questions[4].question.text = "How is your data usage on average?".into();
    validate(&ok, false).unwrap();
}

#[test]
fn conditions_and_distances() {
    let ppl = people();
    let p = &ppl[0]; // id 1
    let c = |v: Value| -> Condition { serde_json::from_value(v).unwrap() };
    assert!(c(json!({"attribute": "gender", "eq": "male"})).matches(p));
    assert!(c(json!({"attribute": "age", "min": 30, "max": 40})).matches(p));
    assert!(!c(json!({"attribute": "age", "min": 40})).matches(p));
    assert!(c(json!({"attribute": "region", "one_of": ["Quebec", "Ontario"]})).matches(p));
    assert!(c(json!({"attribute": "biases", "contains": "early"})).matches(p));
    assert!(!c(json!({"attribute": "missing", "eq": 1})).matches(p));
    // contains_any: any of several phrasings, since biases are free text (persona.v1.md gives
    // examples, not a fixed vocabulary) and a rule tied to one exact phrase misses synonyms.
    assert!(c(json!({"attribute": "biases", "contains_any": ["novelty", "early adopter"]})).matches(p));
    assert!(!c(json!({"attribute": "biases", "contains_any": ["brand-loyal", "status quo"]})).matches(p));
    let a: BTreeMap<String, f64> = [("A".into(), 0.5), ("B".into(), 0.5)].into();
    let b: BTreeMap<String, f64> = [("A".into(), 1.0)].into();
    assert_eq!(tvd(&a, &a), 0.0);
    assert!((tvd(&a, &b) - 0.5).abs() < 1e-12);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_faithful_model_scores_high_and_an_ignorant_one_does_not() {
    let (pk, ppl) = (pack(), people());
    let good = bench(faithful(pk.clone(), ppl)).await;
    let bad = bench(ignorant(pk)).await;
    eprintln!(
        "faithful: score {} sensitive {}% | ignorant: score {} sensitive {}% worst {:?}",
        good.score,
        good.sensitive_share,
        bad.score,
        bad.sensitive_share,
        bad.worst_subgroups
            .iter()
            .map(|s| (&s.group, s.score))
            .collect::<Vec<_>>()
    );
    assert!(good.score >= 90.0, "faithful score {}", good.score);
    assert!(
        good.sensitive_share >= 80.0,
        "faithful sensitivity {}",
        good.sensitive_share
    );
    assert!(
        bad.score <= good.score - 20.0,
        "ignorant score {} vs {}",
        bad.score,
        good.score
    );
    assert!(
        bad.sensitive_share <= 34.0,
        "ignorant sensitivity {}",
        bad.sensitive_share
    );
    // The label-free checks catch the ignorant model's habits.
    let flags: Vec<&String> = bad
        .questions
        .iter()
        .flat_map(|q| &q.validity_flags)
        .collect();
    assert!(
        flags.iter().any(|f| f.starts_with("Order effect")),
        "{flags:?}"
    );
    assert!(flags.iter().any(|f| f.contains("midpoint")), "{flags:?}");
    assert_eq!(bad.worst_subgroups.len(), 3);
    assert!(good
        .disclosure
        .contains("not whether answers match real people"));
    assert_eq!(good.pack_status, "draft");
}

#[test]
fn candidates_become_a_draft_pack_that_needs_rules() {
    let reply = json!({"questions": [
        {"code": "x", "attribute": "price_sensitivity", "text": "Which phone would you pick?", "type": "single_choice", "options": ["Cheap", "Mid", "Top"]},
        {"code": "y", "attribute": "age", "text": "How easy is setup?", "type": "likert", "scale_min_label": "Hard", "scale_max_label": "Easy"},
        {"code": "z", "attribute": "income", "text": "Bad", "type": "ranking"}
    ]});
    let p = candidates::draft_pack(&reply, "mobile_phone", "fidelity.v2");
    assert_eq!(p.questions.len(), 2);
    assert_eq!(p.status, "draft");
    assert!(p.questions.iter().all(|q| q.rule.cases.is_empty()
        && q.rule_author.is_empty()
        && q.source == "gemini-candidate"));
    // Unusable until a person writes the rules.
    let problems = validate(&p, false).unwrap_err();
    assert!(problems.iter().any(|x| x.contains("rule_author")));
    // The candidate prompt never asks for answers or rules.
    let req = candidates::request("m", "mobile_phone", 30);
    let schema = req.schema.to_string();
    assert!(!schema.contains("expect") && !schema.contains("rule"));
}

/// Runs the pack once under one seed and returns the raw answers, for building the
/// `(seed, answers)` pairs `stability` takes.
async fn run_seed(pk: &Pack, ppl: &[Person], llm: ScriptedLlm, seed: u64) -> Vec<BenchAnswer> {
    let run = runner::run(pk, ppl, Arc::new(llm), limiter(), "test-flash", seed, 8)
        .await
        .unwrap();
    assert_eq!((run.invalid, run.failed), (0, 0));
    run.answers
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stability_holds_for_a_model_that_reads_the_persona() {
    let (pk, ppl) = (pack(), people());
    let mut runs = Vec::new();
    for seed in [11u64, 17, 23] {
        runs.push((
            seed,
            run_seed(&pk, &ppl, faithful(pk.clone(), ppl.clone()), seed).await,
        ));
    }
    let report = stability(&pk, &ppl, "test-flash", &runs);
    assert_eq!(report.seeds, vec![11, 17, 23]);
    assert!(report.stable, "{report:?}");
    assert!(report.max_tvd < 0.05, "max_tvd {}", report.max_tvd);
}

/// `stability` compares aggregate distributions, not individual people, so a genuinely
/// order-driven model (always the first option shown) still nets out close to stable once
/// pooled over enough respondents and different shuffles — real instability looks like the
/// answers themselves moving between runs, which this constructs directly.
#[test]
fn stability_flags_a_question_whose_answers_shift_between_seeds() {
    let pk = pack();
    let ppl = people();
    let code = "F01_NEXT_PHONE";
    let run_a: Vec<BenchAnswer> = ppl
        .iter()
        .map(|p| BenchAnswer {
            person: p.id,
            question: code.into(),
            key: "A".into(),
            position: None,
        })
        .collect();
    let run_b: Vec<BenchAnswer> = ppl
        .iter()
        .map(|p| BenchAnswer {
            person: p.id,
            question: code.into(),
            key: "D".into(),
            position: None,
        })
        .collect();
    let report = stability(&pk, &ppl, "test-flash", &[(11, run_a), (17, run_b)]);
    let row = report.questions.iter().find(|s| s.code == code).unwrap();
    assert!(row.flagged, "{row:?}");
    assert_eq!(row.tvd, 1.0);
    assert!(!report.stable);
}

#[test]
fn stability_holds_when_answers_dont_move_between_seeds() {
    let pk = pack();
    let ppl = people();
    let answers: Vec<BenchAnswer> = pk
        .questions
        .iter()
        .flat_map(|q| {
            let key = answer_keys(q).first().cloned().unwrap_or_default();
            ppl.iter().map(move |p| BenchAnswer {
                person: p.id,
                question: q.code.clone(),
                key: key.clone(),
                position: None,
            })
        })
        .collect();
    let report = stability(
        &pk,
        &ppl,
        "test-flash",
        &[(11, answers.clone()), (17, answers.clone()), (23, answers)],
    );
    assert!(report.stable, "{report:?}");
    assert_eq!(report.max_tvd, 0.0);
    assert_eq!(report.seeds, vec![11, 17, 23]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn model_comparison_reports_two_models_side_by_side() {
    let (pk, ppl) = (pack(), people());
    let a = run_seed(&pk, &ppl, faithful(pk.clone(), ppl.clone()), 11).await;
    let b = run_seed(&pk, &ppl, ignorant(pk.clone()), 11).await;
    let good = score(&pk, &ppl, &a, ("flash-a", crate::engine::answer::PROMPT_VERSION, 11));
    let bad = score(&pk, &ppl, &b, ("flash-b", crate::engine::answer::PROMPT_VERSION, 11));
    let cmp = compare_models(&good, &bad);
    assert_eq!(cmp.a_model, "flash-a");
    assert_eq!(cmp.b_model, "flash-b");
    assert_eq!(cmp.rows.len(), pk.questions.len());
    assert!(cmp.a_score > cmp.b_score);
    let f01 = cmp
        .rows
        .iter()
        .find(|r| r.code == "F01_NEXT_PHONE")
        .unwrap();
    assert!(f01.a_tvd < f01.b_tvd);
}
