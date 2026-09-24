//! Answering (docs/DATA_FLOW.md §3, Step 4): one Gemini call per respondent with the whole
//! approved survey. Option order is shuffled per respondent from the run seed, options are
//! shown as numbers, and every answer is checked against its question.

use rand::seq::SliceRandom;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde_json::{json, Value};

use crate::llm::StructuredRequest;
use crate::model::{Question, QuestionType, RespondentDetail};

pub const PROMPT_VERSION: &str = "answer.v2";
const SYSTEM: &str = include_str!("../../prompts/answer.v2.md");
pub const TEMPERATURE: f32 = 1.0;

/// Option codes in the order this respondent sees them. Seeded by run seed, respondent and
/// question, so the same run config gives the same orders.
pub fn shown_order(q: &Question, seed: u64, respondent_ordinal: u32) -> Vec<String> {
    let mut codes: Vec<String> = q.body.options.iter().map(|o| o.code.clone()).collect();
    if q.body.randomize {
        let mix = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(u64::from(respondent_ordinal) << 32)
            .wrapping_add(q.id as u64);
        codes.shuffle(&mut ChaCha8Rng::seed_from_u64(mix));
    }
    codes
}

/// The profile block, from what Step 2 stored. Screening results are left out.
pub fn persona_text(d: &RespondentDetail) -> String {
    let country = crate::countries::find(&d.country)
        .map(|c| c.name.clone())
        .unwrap_or_else(|| d.country.clone());
    let mut t = format!(
        "Name: {}\nAge: {}\nGender: {}\nLives in: {}{}\nHousehold income: {} (yearly, local currency)\nOccupation group: {}\n\n{}\n\nValues: {}\nHabits: {}\nMedia habits: {}\nBrand loyalties: {}\nAttitude to the category: {}\nPrice sensitivity (1–5): {}\nDecision tendencies: {}\n",
        d.name,
        d.age,
        d.gender,
        if d.region.is_empty() { String::new() } else { format!("{}, ", d.region) },
        country,
        d.income,
        d.occupation,
        d.summary,
        d.values.join("; "),
        d.habits,
        d.media_habits,
        d.brand_loyalties,
        d.category_attitudes,
        d.price_sensitivity,
        d.biases.join("; "),
    );
    for (k, v) in &d.category_facts {
        t.push_str(&format!("{k}: {v}\n"));
    }
    t
}

fn describe(q: &Question, shown: &[String]) -> String {
    let b = &q.body;
    let mut s = format!("[{}] ", q.code);
    match b.question_type {
        QuestionType::SingleChoice => s.push_str("(pick one number)\n"),
        QuestionType::MultiChoice => s.push_str(&format!(
            "(pick up to {} numbers)\n",
            b.max_choices.unwrap_or(b.options.len() as u32)
        )),
        QuestionType::Likert => {
            let sc = b.scale.as_ref().expect("likert has a scale");
            s.push_str(&format!(
                "(scale {} = {} to {} = {})\n",
                sc.min,
                if sc.min_label.is_empty() {
                    "lowest"
                } else {
                    &sc.min_label
                },
                sc.max,
                if sc.max_label.is_empty() {
                    "highest"
                } else {
                    &sc.max_label
                },
            ));
        }
        QuestionType::Numeric => {
            let r = b.numeric.as_ref().expect("numeric has a range");
            s.push_str(&format!(
                "(a number from {} to {} {})\n",
                r.min, r.max, r.unit
            ));
        }
        QuestionType::OpenEnded => s.push_str("(answer in your own words)\n"),
    }
    s.push_str(&b.text);
    s.push('\n');
    for (i, code) in shown.iter().enumerate() {
        let label = &b
            .options
            .iter()
            .find(|o| &o.code == code)
            .expect("option")
            .label;
        s.push_str(&format!("  {}. {}\n", i + 1, label));
    }
    s
}

/// A schema built from this survey: one required entry per question code, holding exactly
/// the field its type needs, with its range (Gemini enforces these, so a choice question can't
/// be answered with free text).
pub fn reply_schema(questions: &[(&Question, Vec<String>)]) -> Value {
    let reason = json!({ "type": "string" });
    let mut props = serde_json::Map::new();
    for (q, shown) in questions {
        let k = shown.len().max(1);
        let (field, spec) = match q.body.question_type {
            QuestionType::SingleChoice => (
                "choice",
                json!({ "type": "integer", "minimum": 1, "maximum": k }),
            ),
            QuestionType::MultiChoice => (
                "choices",
                json!({
                    "type": "array",
                    "items": { "type": "integer", "minimum": 1, "maximum": k },
                    "minItems": 1,
                    "maxItems": q.body.max_choices.unwrap_or(k as u32)
                }),
            ),
            QuestionType::Likert => {
                let s = q.body.scale.as_ref().expect("likert has a scale");
                (
                    "value",
                    json!({ "type": "integer", "minimum": s.min, "maximum": s.max }),
                )
            }
            QuestionType::Numeric => {
                let r = q.body.numeric.as_ref().expect("numeric has a range");
                (
                    "value",
                    json!({ "type": "number", "minimum": r.min, "maximum": r.max }),
                )
            }
            QuestionType::OpenEnded => ("text", json!({ "type": "string" })),
        };
        props.insert(
            q.code.clone(),
            json!({
                "type": "object",
                "properties": { field: spec, "reason": reason },
                "required": [field, "reason"]
            }),
        );
    }
    let required: Vec<&str> = questions.iter().map(|(q, _)| q.code.as_str()).collect();
    json!({ "type": "object", "properties": props, "required": required })
}

/// Instructions first (identical for every call, so Gemini's implicit cache can reuse them),
/// then the survey with this respondent's option order, then the persona.
pub fn build_request(
    model: &str,
    intro: &str,
    questions: &[(&Question, Vec<String>)],
    persona: &str,
) -> StructuredRequest {
    let mut prompt = String::from("Survey\n");
    if !intro.trim().is_empty() {
        prompt.push_str(intro.trim());
        prompt.push('\n');
    }
    prompt.push('\n');
    for (q, shown) in questions {
        prompt.push_str(&describe(q, shown));
        prompt.push('\n');
    }
    prompt.push_str("About you\n");
    prompt.push_str(persona);
    StructuredRequest {
        model: model.to_string(),
        system: SYSTEM.to_string(),
        prompt,
        schema: reply_schema(questions),
        temperature: TEMPERATURE,
        max_output_tokens: 8_192,
        logprobs: false,
    }
}

/// Distribution mode (docs/SPEC.md §8, BACKLOG B23): one question, one call, so the output has
/// few enough tokens that the one encoding the chosen option can be told apart from the rest.
/// Only single-choice questions are supported; a multi-choice answer is several independent
/// yes/no picks, not one categorical distribution.
pub fn build_single_question_request(
    model: &str,
    question: &Question,
    shown: &[String],
    persona: &str,
) -> StructuredRequest {
    let mut prompt = String::from("Survey\n\n");
    prompt.push_str(&describe(question, shown));
    prompt.push_str("\nAbout you\n");
    prompt.push_str(persona);
    StructuredRequest {
        model: model.to_string(),
        system: SYSTEM.to_string(),
        prompt,
        schema: reply_schema(&[(question, shown.to_vec())]),
        temperature: TEMPERATURE,
        max_output_tokens: 256,
        logprobs: true,
    }
}

/// Turns a raw `logprobsResult` (Google's documented shape: `topCandidates`, one entry per
/// output token, each holding that position's top alternates) into a probability per shown
/// option, for a single-choice question's `"choice"` field (an integer from 1 to
/// `shown.len()`). Finds the output token position whose top alternates are mostly small
/// integers in that range — the digit answering `"choice"` — since `build_single_question_request`
/// asks for exactly one question, so no other output token is likely to look like that.
/// `None` when no such position is found (never requested, or the model didn't return one).
///
/// Only options whose token appears among the requested top alternates get a share of the
/// mass; a model whose true second-choice options fall outside that list are not counted.
pub fn option_probabilities(
    logprobs: &Value,
    shown: &[String],
) -> Option<std::collections::BTreeMap<String, f64>> {
    let positions = logprobs["topCandidates"].as_array()?;
    let n = shown.len() as i64;
    let best = positions.iter().max_by_key(|pos| {
        pos["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| {
                c["token"]
                    .as_str()
                    .and_then(|t| t.trim().parse::<i64>().ok())
                    .is_some_and(|v| (1..=n).contains(&v))
            })
            .count()
    })?;
    let candidates = best["candidates"].as_array()?;
    let mut by_index: std::collections::BTreeMap<usize, f64> = std::collections::BTreeMap::new();
    for c in candidates {
        let Some(i) = c["token"]
            .as_str()
            .and_then(|t| t.trim().parse::<i64>().ok())
            .filter(|v| (1..=n).contains(v))
            .map(|v| v as usize - 1)
        else {
            continue;
        };
        let Some(lp) = c["logProbability"].as_f64() else {
            continue;
        };
        *by_index.entry(i).or_insert(0.0) += lp.exp();
    }
    if by_index.is_empty() {
        return None;
    }
    let total: f64 = by_index.values().sum();
    Some(
        by_index
            .into_iter()
            .map(|(i, p)| (shown[i].clone(), p / total))
            .collect(),
    )
}

/// A checked answer, ready to store.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    /// `{"code":"B"}`, `{"codes":[…]}`, `{"value":5}` or `{"text":"…"}`.
    pub answer_json: Value,
    pub code: Option<String>,
    pub codes: Option<Vec<String>>,
    pub value: Option<f64>,
    pub text: Option<String>,
    pub reason: String,
}

/// Checks one reply entry against its question. `Err` says what is wrong.
pub fn check(q: &Question, shown: &[String], a: &Value) -> Result<Checked, String> {
    let reason = a["reason"].as_str().unwrap_or_default().trim().to_string();
    let pick = |n: &Value| -> Option<String> {
        let i = n.as_i64()?;
        (1..=shown.len() as i64)
            .contains(&i)
            .then(|| shown[i as usize - 1].clone())
    };
    // Fallback: the option's exact text instead of its number.
    let by_label = |v: &Value| -> Option<String> {
        let t = v.as_str()?.trim().to_lowercase();
        q.body
            .options
            .iter()
            .find(|o| shown.contains(&o.code) && o.label.trim().to_lowercase() == t)
            .map(|o| o.code.clone())
    };
    let mut c = Checked {
        answer_json: Value::Null,
        code: None,
        codes: None,
        value: None,
        text: None,
        reason,
    };
    match q.body.question_type {
        QuestionType::SingleChoice => {
            let code = pick(&a["choice"])
                .or_else(|| by_label(&a["choice"]).or_else(|| by_label(&a["text"])))
                .ok_or_else(|| {
                    format!(
                        "{}: \"choice\" must be one number from 1 to {}",
                        q.code,
                        shown.len()
                    )
                })?;
            c.answer_json = json!({ "code": code });
            c.code = Some(code);
        }
        QuestionType::MultiChoice => {
            let max = q.body.max_choices.unwrap_or(shown.len() as u32) as usize;
            let list = a["choices"].as_array().cloned().unwrap_or_default();
            let mut codes: Vec<String> = list.iter().filter_map(pick).collect();
            codes.dedup();
            let mut uniq = codes.clone();
            uniq.sort();
            uniq.dedup();
            if codes.is_empty()
                || codes.len() != list.len()
                || uniq.len() != codes.len()
                || codes.len() > max
            {
                return Err(format!(
                    "{}: \"choices\" must be 1 to {max} different numbers from 1 to {}",
                    q.code,
                    shown.len()
                ));
            }
            c.answer_json = json!({ "codes": codes });
            c.codes = Some(codes);
        }
        QuestionType::Likert => {
            let sc = q.body.scale.as_ref().expect("likert has a scale");
            let v = a["value"].as_f64().filter(|v| {
                v.fract() == 0.0 && (f64::from(sc.min)..=f64::from(sc.max)).contains(v)
            });
            let v = v.ok_or_else(|| {
                format!(
                    "{}: \"value\" must be a whole number from {} to {}",
                    q.code, sc.min, sc.max
                )
            })?;
            c.answer_json = json!({ "value": v });
            c.value = Some(v);
        }
        QuestionType::Numeric => {
            let r = q.body.numeric.as_ref().expect("numeric has a range");
            let v = a["value"]
                .as_f64()
                .filter(|v| v.is_finite() && (r.min..=r.max).contains(v))
                .ok_or_else(|| {
                    format!(
                        "{}: \"value\" must be a number from {} to {}",
                        q.code, r.min, r.max
                    )
                })?;
            c.answer_json = json!({ "value": v });
            c.value = Some(v);
        }
        QuestionType::OpenEnded => {
            let t: String = a["text"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .chars()
                .take(2_000)
                .collect();
            if t.is_empty() {
                return Err(format!("{}: \"text\" is empty", q.code));
            }
            c.answer_json = json!({ "text": t });
            c.text = Some(t);
        }
    }
    Ok(c)
}

/// Checks a whole reply: one result per question, in survey order.
pub fn check_reply(
    questions: &[(&Question, Vec<String>)],
    reply: &Value,
) -> Vec<Result<Checked, String>> {
    // Replies are keyed by question code; a list of {"code": …} entries is accepted too.
    let listed = reply["answers"].as_array().cloned().unwrap_or_default();
    questions
        .iter()
        .map(|(q, shown)| {
            let a = match reply.get(&q.code) {
                Some(a) if a.is_object() => a.clone(),
                _ => listed
                    .iter()
                    .find(|a| a["code"].as_str().map(str::trim) == Some(q.code.as_str()))
                    .cloned()
                    .ok_or_else(|| format!("{}: no answer", q.code))?,
            };
            check(q, shown, &a)
        })
        .collect()
}

/// How an answer reads in the live console.
pub fn display(q: &Question, c: &Checked) -> String {
    let label = |code: &str| {
        q.body
            .options
            .iter()
            .find(|o| o.code == code)
            .map_or(code.to_string(), |o| o.label.clone())
    };
    if let Some(code) = &c.code {
        label(code)
    } else if let Some(codes) = &c.codes {
        codes
            .iter()
            .map(|x| label(x))
            .collect::<Vec<_>>()
            .join(", ")
    } else if let Some(v) = c.value {
        format!("{v}")
    } else {
        c.text.clone().unwrap_or_default()
    }
}

/// Test helper: a valid reply for the questions as they appear in a prompt built by
/// `build_request` (codes in brackets). Picks option 1, the scale minimum, the range minimum.
pub fn reply_for_prompt(prompt: &str) -> Value {
    let mut answers = serde_json::Map::new();
    for line in prompt.lines() {
        if let Some(rest) = line.strip_prefix('[') {
            let Some((code, kind)) = rest.split_once("] ") else {
                continue;
            };
            let a = if kind.starts_with("(pick one") {
                json!({ "choice": 1, "reason": "It fits me." })
            } else if kind.starts_with("(pick up to") {
                json!({ "choices": [1], "reason": "It fits me." })
            } else if let Some(r) = kind.strip_prefix("(scale ") {
                let min: f64 = r.split_whitespace().next().unwrap().parse().unwrap();
                json!({ "value": min, "reason": "Not keen." })
            } else if let Some(r) = kind.strip_prefix("(a number from ") {
                let min: f64 = r.split_whitespace().next().unwrap().parse().unwrap();
                json!({ "value": min, "reason": "Budget." })
            } else {
                json!({ "text": "It depends on the price.", "reason": "Honest view." })
            };
            answers.insert(code.to_string(), a);
        }
    }
    Value::Object(answers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ChoiceOption, NumericRange, QuestionBody, QuestionOrigin, ReviewStatus, Scale,
    };

    pub(crate) fn q(id: i64, code: &str, t: QuestionType) -> Question {
        Question {
            id,
            code: code.into(),
            order_index: id as u32,
            body: QuestionBody {
                text: format!("Question {code}?"),
                question_type: t,
                options: if t.is_choice() {
                    ["Apple", "Samsung", "Google", "Other"]
                        .iter()
                        .enumerate()
                        .map(|(i, l)| ChoiceOption {
                            code: crate::db::surveys::option_code(i),
                            label: l.to_string(),
                        })
                        .collect()
                } else {
                    vec![]
                },
                randomize: t.is_choice(),
                max_choices: (t == QuestionType::MultiChoice).then_some(2),
                scale: (t == QuestionType::Likert).then(|| Scale {
                    min: 1,
                    max: 7,
                    min_label: "Low".into(),
                    max_label: "High".into(),
                }),
                numeric: (t == QuestionType::Numeric).then(|| NumericRange {
                    min: 0.0,
                    max: 2000.0,
                    unit: "CAD".into(),
                }),
            },
            is_active: true,
            origin: QuestionOrigin::Ai,
            review_status: ReviewStatus::Accepted,
            objective: Some("SECRET OBJECTIVE".into()),
            rationale: Some("SECRET RATIONALE".into()),
            critique: Some(crate::model::Critique {
                status: crate::model::CriticStatus::Done,
                flags: vec![crate::model::CriticFlag {
                    issue: crate::model::CriticIssue::Leading,
                    note: "SECRET CRITIC NOTE".into(),
                }],
                error: None,
                prompt_version: "critic.v1".into(),
            }),
        }
    }

    #[test]
    fn shuffles_are_seeded_per_respondent_and_question() {
        let a = q(1, "Q1", QuestionType::SingleChoice);
        assert_eq!(shown_order(&a, 7, 1), shown_order(&a, 7, 1));
        let orders: std::collections::HashSet<Vec<String>> =
            (1..40).map(|r| shown_order(&a, 7, r)).collect();
        assert!(orders.len() > 5, "orders barely vary");
        assert_ne!(
            (1..20).map(|r| shown_order(&a, 7, r)).collect::<Vec<_>>(),
            (1..20).map(|r| shown_order(&a, 8, r)).collect::<Vec<_>>()
        );
        let mut fixed = a.clone();
        fixed.body.randomize = false;
        assert_eq!(shown_order(&fixed, 7, 3), ["A", "B", "C", "D"]);
    }

    #[test]
    fn prompt_shows_numbers_and_hides_objective_and_rationale() {
        let a = q(1, "Q1", QuestionType::SingleChoice);
        let shown = vec!["C".to_string(), "A".into(), "D".into(), "B".into()];
        let r = build_request("m", "Intro text.", &[(&a, shown.clone())], "Name: Pat\n");
        assert!(r.prompt.contains("  1. Google\n  2. Apple"));
        assert!(!r.prompt.contains("SECRET"));
        assert!(r.prompt.find("Survey").unwrap() < r.prompt.find("About you").unwrap());
        // Option 1 as shown is Google (code C).
        let c = check(
            &a,
            &shown,
            &json!({"code": "Q1", "choice": 1, "reason": "r"}),
        )
        .unwrap();
        assert_eq!(c.code.as_deref(), Some("C"));
        assert_eq!(display(&a, &c), "Google");
    }

    #[test]
    fn answers_are_checked_against_their_question() {
        let s = q(1, "S", QuestionType::SingleChoice);
        let m = q(2, "M", QuestionType::MultiChoice);
        let l = q(3, "L", QuestionType::Likert);
        let n = q(4, "N", QuestionType::Numeric);
        let o = q(5, "O", QuestionType::OpenEnded);
        let sh = shown_order(&s, 1, 1);
        assert!(check(&s, &sh, &json!({"choice": 5})).is_err());
        assert!(check(&s, &sh, &json!({"choice": 0})).is_err());
        assert!(check(&m, &sh, &json!({"choices": [1, 2, 3]})).is_err()); // max 2
        assert!(check(&m, &sh, &json!({"choices": [1, 1]})).is_err());
        assert!(check(&m, &sh, &json!({"choices": [1, 9]})).is_err());
        assert_eq!(
            check(&m, &sh, &json!({"choices": [2, 1]}))
                .unwrap()
                .codes
                .unwrap()
                .len(),
            2
        );
        assert!(check(&l, &[], &json!({"value": 8})).is_err());
        assert!(check(&l, &[], &json!({"value": 3.5})).is_err());
        assert_eq!(
            check(&l, &[], &json!({"value": 7})).unwrap().value,
            Some(7.0)
        );
        assert!(check(&n, &[], &json!({"value": -1})).is_err());
        assert!(check(&o, &[], &json!({"text": "  "})).is_err());
        let qs = vec![(&s, sh.clone()), (&o, vec![])];
        let res = check_reply(
            &qs,
            &json!({"answers": [{"code": "S", "choice": 2, "reason": "r"}]}),
        );
        assert!(res[0].is_ok());
        assert_eq!(res[1].as_ref().unwrap_err(), "O: no answer");
    }

    #[test]
    fn the_schema_requires_the_right_field_for_each_question() {
        let s = q(1, "S", QuestionType::SingleChoice);
        let m = q(2, "M", QuestionType::MultiChoice);
        let l = q(3, "L", QuestionType::Likert);
        let o = q(4, "O", QuestionType::OpenEnded);
        let pairs = vec![
            (&s, shown_order(&s, 1, 1)),
            (&m, shown_order(&m, 1, 1)),
            (&l, vec![]),
            (&o, vec![]),
        ];
        let schema = reply_schema(&pairs);
        assert_eq!(schema["required"], json!(["S", "M", "L", "O"]));
        assert_eq!(
            schema["properties"]["S"]["required"],
            json!(["choice", "reason"])
        );
        assert_eq!(
            schema["properties"]["S"]["properties"]["choice"]["maximum"],
            4
        );
        assert_eq!(
            schema["properties"]["M"]["properties"]["choices"]["maxItems"],
            2
        );
        assert_eq!(
            schema["properties"]["L"]["properties"]["value"]["maximum"],
            7
        );
        assert_eq!(
            schema["properties"]["O"]["required"],
            json!(["text", "reason"])
        );
        assert_eq!(build_request("m", "", &pairs, "p").schema, schema);
    }

    /// Seen live: Gemini answered a choice question with the option's text.
    #[test]
    fn a_choice_given_as_the_option_text_is_accepted() {
        let s = q(1, "S", QuestionType::SingleChoice);
        let shown = shown_order(&s, 1, 1);
        let c = check(&s, &shown, &json!({"text": " samsung ", "reason": "r"})).unwrap();
        assert_eq!(c.code.as_deref(), Some("B"));
        assert!(check(&s, &shown, &json!({"text": "Nokia", "reason": "r"})).is_err());
        // Keyed replies and the older list form both work.
        let pairs = vec![(&s, shown.clone())];
        assert!(check_reply(&pairs, &json!({"S": {"choice": 1, "reason": "r"}}))[0].is_ok());
        assert!(check_reply(
            &pairs,
            &json!({"answers": [{"code": "S", "choice": 1, "reason": "r"}]})
        )[0]
        .is_ok());
    }

    #[test]
    fn test_reply_helper_answers_every_question_validly() {
        let qs: Vec<Question> = [
            QuestionType::SingleChoice,
            QuestionType::MultiChoice,
            QuestionType::Likert,
            QuestionType::Numeric,
            QuestionType::OpenEnded,
        ]
        .iter()
        .enumerate()
        .map(|(i, t)| q(i as i64 + 1, &format!("Q{}", i + 1), *t))
        .collect();
        let pairs: Vec<(&Question, Vec<String>)> =
            qs.iter().map(|x| (x, shown_order(x, 3, 9))).collect();
        let r = build_request("m", "", &pairs, "Name: Pat\n");
        assert!(check_reply(&pairs, &reply_for_prompt(&r.prompt))
            .iter()
            .all(Result::is_ok));
    }

    #[test]
    fn single_question_request_asks_for_logprobs_and_only_this_question() {
        let question = q(1, "Q1", QuestionType::SingleChoice);
        let shown = shown_order(&question, 3, 9);
        let req = build_single_question_request("m", &question, &shown, "Name: Pat\n");
        assert!(req.logprobs);
        assert!(req.prompt.contains("[Q1]"));
        assert!(!req.prompt.contains("SECRET OBJECTIVE"));
        assert_eq!(req.schema["required"], json!(["Q1"]));
    }

    /// Google's documented `logprobsResult` shape: one entry per output token, each with its
    /// own top alternates. A compact `{"choice":N,"reason":"…"}` reply puts the digit for
    /// `choice` in one token position; the "reason" text's tokens rarely all look like small
    /// integers, so the heuristic (most integer-shaped alternates) finds the right position.
    fn logprobs_result(chosen_index: usize, mass: &[(&str, f64)]) -> Value {
        let candidates: Vec<Value> = mass
            .iter()
            .map(|(tok, p)| json!({ "token": tok, "logProbability": p.ln() }))
            .collect();
        json!({
            "topCandidates": [
                // A position that looks nothing like the answer (part of "reason"'s text).
                { "candidates": [ { "token": "The", "logProbability": -0.05 } ] },
                { "candidates": candidates }
            ],
            "chosenCandidates": [
                { "token": "The", "logProbability": -0.05 },
                { "token": mass[chosen_index].0, "logProbability": mass[chosen_index].1.ln() }
            ]
        })
    }

    #[test]
    fn option_probabilities_reads_the_integer_shaped_token_position() {
        let shown = vec!["A".into(), "B".into(), "C".into(), "D".into()];
        let lp = logprobs_result(1, &[("1", 0.1), ("2", 0.7), ("3", 0.15), ("4", 0.05)]);
        let probs = option_probabilities(&lp, &shown).unwrap();
        assert!((probs["B"] - 0.7).abs() < 1e-9, "{probs:?}");
        assert!((probs.values().sum::<f64>() - 1.0).abs() < 1e-9);
        assert_eq!(probs.len(), 4);
    }

    #[test]
    fn option_probabilities_renormalises_over_the_alternates_it_actually_saw() {
        // Only 2 of 4 options showed up in the top alternates: the mass still sums to 1.
        let shown = vec!["A".into(), "B".into(), "C".into(), "D".into()];
        let lp = logprobs_result(0, &[("1", 0.8), ("2", 0.2)]);
        let probs = option_probabilities(&lp, &shown).unwrap();
        assert_eq!(probs.len(), 2);
        assert!((probs.values().sum::<f64>() - 1.0).abs() < 1e-9);
        assert!(probs["A"] > probs["B"]);
    }

    #[test]
    fn option_probabilities_is_none_without_logprobs() {
        assert!(option_probabilities(&json!({}), &["A".into(), "B".into()]).is_none());
        assert!(option_probabilities(
            &json!({"topCandidates": [{"candidates": [{"token": "the", "logProbability": -0.1}]}]}),
            &["A".into(), "B".into()]
        )
        .is_none());
    }
}
