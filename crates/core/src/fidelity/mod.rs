//! Fidelity benchmark (docs/SPEC.md §8, TEST_PLAN S13): does a persona's attribute actually
//! drive its answers?
//!
//! A pack holds questions, each tied to one persona attribute with a rule, written by a
//! person, that maps the attribute to an expected answer distribution. Gemini may propose
//! candidate questions (`candidates`), but never rules or expected answers. The expected
//! distribution for a cohort is computed exactly from its personas; no LLM is involved.
//! Synthetic answers are then compared with it:
//!
//! - fidelity score: 100 × (1 − mean total variation distance);
//! - attribute sensitivity: people matching a rule's first case pick its most expected
//!   answer more often than everyone else (one-sided two-proportion test, p < 0.05);
//! - subgroups: the score per age band, gender, income and region, worst three reported.
//!
//! This measures fidelity, not realism: nothing here says answers match real people.

pub mod candidates;
pub mod runner;

use std::collections::{BTreeMap, HashMap};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::surveys;
use crate::engine::persona::EnrichedPersona;
use crate::error::{AppError, AppResult};
use crate::model::{Question, QuestionBody, QuestionOrigin, QuestionType, ReviewStatus};
use crate::report::validity;

pub const PACK_FORMAT: &str = "fidelity-pack/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pack {
    pub format: String,
    /// e.g. "fidelity.v1"; scores are only compared within one version.
    pub version: String,
    /// "draft" while rules are being written; "frozen" once reviewed. Release runs need frozen.
    pub status: String,
    /// "planted" (rules from persona attributes); "observed" is reserved for real survey data.
    pub ground_truth: String,
    pub category: Option<String>,
    pub reviewed_by: Option<String>,
    pub frozen_at: Option<String>,
    pub notes: Option<String>,
    pub questions: Vec<PackQuestion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackQuestion {
    pub code: String,
    pub question: QuestionBody,
    /// Where the question text came from: "gemini-candidate" or "human".
    pub source: String,
    /// The persona attribute the rule reads, e.g. "price_sensitivity".
    pub attribute: String,
    pub rule: Rule,
    /// Who wrote the rule. Rules are never generated.
    pub rule_author: String,
}

/// First matching case wins; `otherwise` covers everyone else. Distributions map answer keys
/// (option codes, scale points, or numeric bin indexes) to probabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub cases: Vec<Case>,
    pub otherwise: BTreeMap<String, f64>,
    /// Numeric questions: bins as [low, high); answers are keyed by bin index "0", "1", ….
    #[serde(default)]
    pub bins: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub when: Condition,
    pub expect: BTreeMap<String, f64>,
}

/// One test on one attribute. Exactly one of `eq`, `one_of`, `min`/`max`, `contains`,
/// `contains_any`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub attribute: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eq: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// For list attributes (biases, values): case-insensitive substring of any item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    /// Like `contains`, but matching any of several phrasings: biases and values are
    /// free text the persona model writes to its own wording (`persona.v1.md` gives examples,
    /// not a fixed vocabulary), so a rule tied to one exact phrase misses close synonyms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains_any: Option<Vec<String>>,
}

/// Persona attributes a rule may read.
pub const ATTRIBUTES: [&str; 11] = [
    "age",
    "age_band",
    "gender",
    "region",
    "income",
    "occupation",
    "country",
    "price_sensitivity",
    "biases",
    "values",
    "category.<field>",
];

/// A respondent as the benchmark sees them.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub id: i64,
    pub ordinal: u32,
    pub attrs: HashMap<String, Value>,
    /// The profile text the answering prompt uses.
    pub persona_text: String,
}

impl Person {
    pub fn attr(&self, key: &str) -> Option<&Value> {
        self.attrs.get(key)
    }

    pub fn attr_text(&self, key: &str) -> String {
        match self.attrs.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        }
    }
}

impl Condition {
    pub fn matches(&self, p: &Person) -> bool {
        let Some(v) = p.attr(&self.attribute) else {
            return false;
        };
        let text = |v: &Value| match v {
            Value::String(s) => s.to_lowercase(),
            other => other.to_string().to_lowercase(),
        };
        if let Some(eq) = &self.eq {
            return text(v) == text(eq);
        }
        if let Some(list) = &self.one_of {
            return list.iter().any(|x| text(x) == text(v));
        }
        if let Some(needle) = &self.contains {
            let needle = needle.to_lowercase();
            return match v {
                Value::Array(items) => items.iter().any(|i| text(i).contains(&needle)),
                other => text(other).contains(&needle),
            };
        }
        if let Some(needles) = &self.contains_any {
            let needles: Vec<String> = needles.iter().map(|n| n.to_lowercase()).collect();
            let hits = |t: &str| needles.iter().any(|n| t.contains(n.as_str()));
            return match v {
                Value::Array(items) => items.iter().any(|i| hits(&text(i))),
                other => hits(&text(other)),
            };
        }
        if self.min.is_some() || self.max.is_some() {
            let Some(x) = v.as_f64() else { return false };
            return self.min.is_none_or_le(x) && self.max.is_none_or_ge(x);
        }
        false
    }
}

trait Bound {
    fn is_none_or_le(&self, x: f64) -> bool;
    fn is_none_or_ge(&self, x: f64) -> bool;
}

impl Bound for Option<f64> {
    fn is_none_or_le(&self, x: f64) -> bool {
        self.map_or(true, |m| m <= x)
    }
    fn is_none_or_ge(&self, x: f64) -> bool {
        self.map_or(true, |m| x <= m)
    }
}

impl Rule {
    pub fn expected_for(&self, p: &Person) -> &BTreeMap<String, f64> {
        self.cases
            .iter()
            .find(|c| c.when.matches(p))
            .map_or(&self.otherwise, |c| &c.expect)
    }
}

/// The answer keys a question can take, in order.
pub fn answer_keys(q: &PackQuestion) -> Vec<String> {
    let b = &q.question;
    match b.question_type {
        QuestionType::SingleChoice => b.options.iter().map(|o| o.code.clone()).collect(),
        QuestionType::Likert => {
            let s = b.scale.as_ref().map_or((1, 5), |s| (s.min, s.max));
            (s.0..=s.1).map(|v| v.to_string()).collect()
        }
        QuestionType::Numeric => (0..q.rule.bins.len()).map(|i| i.to_string()).collect(),
        QuestionType::MultiChoice | QuestionType::OpenEnded => Vec::new(),
    }
}

/// Checks a pack. `release` also requires it to be frozen and reviewed.
pub fn validate(pack: &Pack, release: bool) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    if pack.format != PACK_FORMAT {
        problems.push(format!("format must be {PACK_FORMAT}"));
    }
    if pack.ground_truth != "planted" && pack.ground_truth != "observed" {
        problems.push("ground_truth must be \"planted\" or \"observed\"".into());
    }
    if release
        && (pack.status != "frozen" || pack.reviewed_by.is_none() || pack.frozen_at.is_none())
    {
        problems.push("release runs need a frozen pack with reviewed_by and frozen_at".into());
    }
    let mut codes = std::collections::HashSet::new();
    for q in &pack.questions {
        let at = |m: &str| format!("{}: {m}", q.code);
        if !codes.insert(q.code.clone()) {
            problems.push(at("duplicate code"));
        }
        if let Err(e) = surveys::normalise(q.question.clone()) {
            problems.push(at(&e.message));
        }
        if matches!(
            q.question.question_type,
            QuestionType::MultiChoice | QuestionType::OpenEnded
        ) {
            problems.push(at(
                "benchmark questions must be single choice, scale or number",
            ));
        }
        if q.question.question_type == QuestionType::Numeric && q.rule.bins.len() < 2 {
            problems.push(at("number questions need at least 2 bins"));
        }
        if q.rule_author.trim().is_empty() {
            problems.push(at("rule_author is empty; rules are written by a person"));
        }
        let attr_ok = |a: &str| ATTRIBUTES.contains(&a) || a.starts_with("category.");
        if !attr_ok(&q.attribute) {
            problems.push(at(&format!("unknown attribute {}", q.attribute)));
        }
        let keys = answer_keys(q);
        let dists = q
            .rule
            .cases
            .iter()
            .map(|c| (&c.expect, Some(&c.when)))
            .chain([(&q.rule.otherwise, None)]);
        for (d, when) in dists {
            if let Some(w) = when {
                if !attr_ok(&w.attribute) {
                    problems.push(at(&format!("unknown attribute {}", w.attribute)));
                }
            }
            let total: f64 = d.values().sum();
            if (total - 1.0).abs() > 1e-6 {
                problems.push(at(&format!("a distribution sums to {total}, not 1")));
            }
            if let Some(k) = d.keys().find(|k| !keys.contains(k)) {
                problems.push(at(&format!("\"{k}\" is not an answer to this question")));
            }
        }
        // A rule that names its attribute in the question makes the test trivial.
        let named = q
            .attribute
            .split('.')
            .next_back()
            .unwrap_or_default()
            .replace('_', " ");
        let words: String = q
            .question
            .text
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect();
        if !named.is_empty()
            && format!(
                " {} ",
                words.split_whitespace().collect::<Vec<_>>().join(" ")
            )
            .contains(&format!(" {named} "))
        {
            problems.push(at("the question names the attribute it tests"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

pub fn load_pack(text: &str) -> AppResult<Pack> {
    Ok(serde_json::from_str(text)?)
}

/// The pack question as the answering prompt sees it.
pub fn as_question(q: &PackQuestion, index: usize) -> Question {
    Question {
        id: index as i64 + 1,
        code: q.code.clone(),
        order_index: index as u32 + 1,
        body: q.question.clone(),
        is_active: true,
        origin: QuestionOrigin::Human,
        review_status: ReviewStatus::Accepted,
        objective: None,
        rationale: None,
        critique: None,
    }
}

/// The cohort's respondents (screening passes and flags) with the attributes rules may use.
pub fn load_people(conn: &Connection, cohort_id: i64) -> AppResult<Vec<Person>> {
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM respondents WHERE cohort_id = ?1 AND screen_status <> 'failed' ORDER BY ordinal")?
        .query_map([cohort_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    ids.into_iter()
        .map(|id| {
            let d = crate::db::cohorts::detail(conn, id)?;
            let doc: String = conn.query_row(
                "SELECT persona_json FROM respondents WHERE id = ?1",
                [id],
                |r| r.get(0),
            )?;
            let doc: Value = serde_json::from_str(&doc)?;
            let p: EnrichedPersona = serde_json::from_value(doc["persona"].clone())?;
            let mut attrs: HashMap<String, Value> = HashMap::new();
            attrs.insert("age".into(), Value::from(d.age));
            attrs.insert(
                "age_band".into(),
                Value::from(crate::report::age_band(d.age)),
            );
            attrs.insert("gender".into(), Value::from(d.gender.clone()));
            attrs.insert("region".into(), Value::from(d.region.clone()));
            attrs.insert("income".into(), Value::from(d.income.clone()));
            attrs.insert("occupation".into(), Value::from(d.occupation.clone()));
            attrs.insert("country".into(), Value::from(d.country.clone()));
            attrs.insert("price_sensitivity".into(), Value::from(p.price_sensitivity));
            attrs.insert("biases".into(), Value::from(p.biases.clone()));
            attrs.insert("values".into(), Value::from(p.values.clone()));
            for (k, v) in &p.category_profile {
                attrs.insert(format!("category.{k}"), v.clone());
            }
            Ok(Person {
                id,
                ordinal: d.ordinal,
                attrs,
                persona_text: crate::engine::answer::persona_text(&d),
            })
        })
        .collect()
}

/// One synthetic answer to a pack question, keyed like the rule (option code, scale point
/// or bin index), plus where the chosen option was shown (for the order-effect check).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchAnswer {
    pub person: i64,
    pub question: String,
    pub key: String,
    /// (position chosen, options shown), 0-based; single choice only.
    pub position: Option<(usize, usize)>,
}

pub fn answer_key(q: &PackQuestion, answer: &Value) -> Option<String> {
    match q.question.question_type {
        QuestionType::SingleChoice => answer["code"].as_str().map(str::to_string),
        QuestionType::Likert => answer["value"].as_f64().map(|v| format!("{}", v as i64)),
        QuestionType::Numeric => {
            let v = answer["value"].as_f64()?;
            let last = q.rule.bins.len().checked_sub(1)?;
            q.rule
                .bins
                .iter()
                .position(|(lo, hi)| v >= *lo && v < *hi)
                .or_else(|| (v == q.rule.bins[last].1).then_some(last))
                .map(|i| i.to_string())
        }
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionScore {
    pub code: String,
    pub attribute: String,
    pub n: u32,
    pub expected: BTreeMap<String, f64>,
    pub observed: BTreeMap<String, f64>,
    /// Total variation distance, 0 (identical) to 1.
    pub tvd: f64,
    /// Did people matching the first rule case pick its top answer more often (p < 0.05)?
    pub sensitive: Option<bool>,
    pub sensitivity_p: Option<f64>,
    pub validity_flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubgroupScore {
    pub dimension: String,
    pub group: String,
    pub n: u32,
    pub score: f64,
    /// 15 points or more below the overall score.
    pub flagged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FidelityReport {
    pub pack_version: String,
    pub pack_status: String,
    pub model: String,
    pub prompt_version: String,
    pub seed: u64,
    pub respondents: u32,
    /// 0–100; labelled "Fidelity", never "Accuracy".
    pub score: f64,
    /// Share (%) of questions whose attribute measurably drives answers (target ≥ 80%).
    pub sensitive_share: f64,
    pub questions: Vec<QuestionScore>,
    pub worst_subgroups: Vec<SubgroupScore>,
    pub disclosure: String,
}

fn distribution(keys: &[String], counts: &HashMap<String, f64>) -> BTreeMap<String, f64> {
    let total: f64 = keys
        .iter()
        .map(|k| counts.get(k).copied().unwrap_or(0.0))
        .sum();
    keys.iter()
        .map(|k| {
            (
                k.clone(),
                if total > 0.0 {
                    counts.get(k).copied().unwrap_or(0.0) / total
                } else {
                    0.0
                },
            )
        })
        .collect()
}

pub fn tvd(a: &BTreeMap<String, f64>, b: &BTreeMap<String, f64>) -> f64 {
    let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    0.5 * keys
        .into_iter()
        .map(|k| (a.get(k).copied().unwrap_or(0.0) - b.get(k).copied().unwrap_or(0.0)).abs())
        .sum::<f64>()
}

fn round(x: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (x * f).round() / f
}

/// Expected vs observed answers to one question over a set of people.
struct Comparison {
    expected: BTreeMap<String, f64>,
    observed: BTreeMap<String, f64>,
    tvd: f64,
    n: u32,
}

/// None when none of `people` answered the question.
fn compare(
    q: &PackQuestion,
    people: &[&Person],
    answers: &HashMap<(i64, &str), &BenchAnswer>,
) -> Option<Comparison> {
    let keys = answer_keys(q);
    let mut expected: HashMap<String, f64> = HashMap::new();
    let mut observed: HashMap<String, f64> = HashMap::new();
    let mut n = 0;
    for p in people {
        let Some(a) = answers.get(&(p.id, q.code.as_str())) else {
            continue;
        };
        n += 1;
        for (k, pr) in q.rule.expected_for(p) {
            *expected.entry(k.clone()).or_default() += pr;
        }
        *observed.entry(a.key.clone()).or_default() += 1.0;
    }
    if n == 0 {
        return None;
    }
    let (expected, observed) = (
        distribution(&keys, &expected),
        distribution(&keys, &observed),
    );
    let tvd = tvd(&expected, &observed);
    Some(Comparison {
        expected,
        observed,
        tvd,
        n,
    })
}

/// Scores answers against a pack. Pure: no LLM, no database.
pub fn score(
    pack: &Pack,
    people: &[Person],
    answers: &[BenchAnswer],
    meta: (&str, &str, u64),
) -> FidelityReport {
    let by_key: HashMap<(i64, &str), &BenchAnswer> = answers
        .iter()
        .map(|a| ((a.person, a.question.as_str()), a))
        .collect();
    let everyone: Vec<&Person> = people.iter().collect();
    let mut questions = Vec::new();
    for (i, q) in pack.questions.iter().enumerate() {
        let Some(Comparison {
            expected,
            observed,
            tvd: d,
            n,
        }) = compare(q, &everyone, &by_key)
        else {
            continue;
        };
        // Attribute sensitivity against the first case.
        let (mut sensitive, mut sensitivity_p) = (None, None);
        if let Some(first) = q.rule.cases.first() {
            if let Some((top, _)) = first.expect.iter().max_by(|a, b| a.1.total_cmp(b.1)) {
                let (mut x1, mut n1, mut x2, mut n2) = (0u32, 0u32, 0u32, 0u32);
                for p in people {
                    let Some(a) = by_key.get(&(p.id, q.code.as_str())) else {
                        continue;
                    };
                    let hit = u32::from(&a.key == top);
                    if first.when.matches(p) {
                        n1 += 1;
                        x1 += hit;
                    } else {
                        n2 += 1;
                        x2 += hit;
                    }
                }
                if n1 > 0 && n2 > 0 {
                    let p2 = validity::two_proportion_p(x1, n1, x2, n2);
                    let direction = f64::from(x1) / f64::from(n1) > f64::from(x2) / f64::from(n2);
                    let one_sided = if direction { p2 / 2.0 } else { 1.0 - p2 / 2.0 };
                    sensitive = Some(direction && one_sided < 0.05);
                    sensitivity_p = Some(round(one_sided, 4));
                }
            }
        }
        // Label-free checks, from the same answers.
        let question = as_question(q, i);
        let rows: Vec<crate::model::ReportRow> = observed
            .iter()
            .map(|(k, share)| crate::model::ReportRow {
                key: k.clone(),
                label: k.clone(),
                count: (share * f64::from(n)).round() as u32,
                percent: round(share * 100.0, 1),
                avg_prob: None,
            })
            .collect();
        let positions: Vec<(usize, usize)> = answers
            .iter()
            .filter(|a| a.question == q.code)
            .filter_map(|a| a.position)
            .collect();
        let v = validity::check(&question, n, &rows, &positions);
        questions.push(QuestionScore {
            code: q.code.clone(),
            attribute: q.attribute.clone(),
            n,
            expected: expected
                .into_iter()
                .map(|(k, v)| (k, round(v, 4)))
                .collect(),
            observed: observed
                .into_iter()
                .map(|(k, v)| (k, round(v, 4)))
                .collect(),
            tvd: round(d, 4),
            sensitive,
            sensitivity_p,
            validity_flags: v.flags,
        });
    }
    let overall = if questions.is_empty() {
        0.0
    } else {
        round(
            100.0 * (1.0 - questions.iter().map(|q| q.tvd).sum::<f64>() / questions.len() as f64),
            1,
        )
    };
    let tested: Vec<&QuestionScore> = questions.iter().filter(|q| q.sensitive.is_some()).collect();
    let sensitive_share = if tested.is_empty() {
        0.0
    } else {
        round(
            100.0 * tested.iter().filter(|q| q.sensitive == Some(true)).count() as f64
                / tested.len() as f64,
            1,
        )
    };

    let mut subgroups = Vec::new();
    for dim in ["age_band", "gender", "income", "region"] {
        let mut groups: BTreeMap<String, Vec<&Person>> = BTreeMap::new();
        for p in people {
            let g = p.attr_text(dim);
            if !g.is_empty() {
                groups.entry(g).or_default().push(p);
            }
        }
        if groups.len() < 2 {
            continue;
        }
        for (g, members) in groups {
            let dists: Vec<f64> = pack
                .questions
                .iter()
                .filter_map(|q| compare(q, &members, &by_key).map(|c| c.tvd))
                .collect();
            if dists.is_empty() {
                continue;
            }
            let s = round(
                100.0 * (1.0 - dists.iter().sum::<f64>() / dists.len() as f64),
                1,
            );
            subgroups.push(SubgroupScore {
                dimension: dim.into(),
                group: g,
                n: members.len() as u32,
                score: s,
                flagged: s <= overall - 15.0,
            });
        }
    }
    subgroups.sort_by(|a, b| a.score.total_cmp(&b.score));
    subgroups.truncate(3);

    FidelityReport {
        pack_version: pack.version.clone(),
        pack_status: pack.status.clone(),
        model: meta.0.into(),
        prompt_version: meta.1.into(),
        seed: meta.2,
        respondents: people.len() as u32,
        score: overall,
        sensitive_share,
        questions,
        worst_subgroups: subgroups,
        disclosure: "Fidelity measures whether persona attributes drive answers, not whether answers match real people. Synthetic respondents — directional only; not calibrated against real survey data.".into(),
    }
}

/// One question's stability across seeds (TEST_PLAN S13 "run-to-run stability"): the same
/// cohort and pack, answered again with a different seed, which reshuffles shown option order.
/// A model that is actually reading the persona should land on close to the same distribution
/// each time; the largest pairwise TVD across the seeds is flagged at 0.1 or more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StabilityScore {
    pub code: String,
    pub tvd: f64,
    pub flagged: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StabilityReport {
    pub pack_version: String,
    pub model: String,
    pub seeds: Vec<u64>,
    pub respondents: u32,
    pub questions: Vec<StabilityScore>,
    pub max_tvd: f64,
    pub stable: bool,
}

fn observed_distribution(
    q: &PackQuestion,
    people: &[&Person],
    answers: &HashMap<(i64, &str), &BenchAnswer>,
) -> Option<BTreeMap<String, f64>> {
    let keys = answer_keys(q);
    let mut counts: HashMap<String, f64> = HashMap::new();
    let mut n = 0u32;
    for p in people {
        let Some(a) = answers.get(&(p.id, q.code.as_str())) else {
            continue;
        };
        n += 1;
        *counts.entry(a.key.clone()).or_default() += 1.0;
    }
    (n > 0).then(|| distribution(&keys, &counts))
}

/// Scores stability across 2+ runs of the same pack, cohort and model, one per seed.
pub fn stability(
    pack: &Pack,
    people: &[Person],
    model: &str,
    runs: &[(u64, Vec<BenchAnswer>)],
) -> StabilityReport {
    let everyone: Vec<&Person> = people.iter().collect();
    let mut questions = Vec::new();
    for q in &pack.questions {
        let dists: Vec<BTreeMap<String, f64>> = runs
            .iter()
            .filter_map(|(_, answers)| {
                let by_key: HashMap<(i64, &str), &BenchAnswer> = answers
                    .iter()
                    .map(|a| ((a.person, a.question.as_str()), a))
                    .collect();
                observed_distribution(q, &everyone, &by_key)
            })
            .collect();
        if dists.len() < 2 {
            continue;
        }
        let mut max_tvd: f64 = 0.0;
        for i in 0..dists.len() {
            for j in (i + 1)..dists.len() {
                max_tvd = max_tvd.max(tvd(&dists[i], &dists[j]));
            }
        }
        questions.push(StabilityScore {
            code: q.code.clone(),
            tvd: round(max_tvd, 4),
            flagged: max_tvd >= 0.1,
        });
    }
    StabilityReport {
        pack_version: pack.version.clone(),
        model: model.into(),
        seeds: runs.iter().map(|(s, _)| *s).collect(),
        respondents: people.len() as u32,
        max_tvd: questions.iter().map(|q| q.tvd).fold(0.0, f64::max),
        stable: questions.iter().all(|q| !q.flagged),
        questions,
    }
}

/// One question compared between two models (TEST_PLAN S13 "model comparison").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelComparisonRow {
    pub code: String,
    pub attribute: String,
    pub a_tvd: f64,
    pub b_tvd: f64,
    pub a_sensitive: Option<bool>,
    pub b_sensitive: Option<bool>,
}

/// The default Flash model's fidelity report (`a`) against one other Gemini model (`b`),
/// side by side, to inform the default model choice. Reported, not pass/fail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelComparison {
    pub pack_version: String,
    pub a_model: String,
    pub b_model: String,
    pub a_score: f64,
    pub b_score: f64,
    pub a_sensitive_share: f64,
    pub b_sensitive_share: f64,
    pub rows: Vec<ModelComparisonRow>,
}

pub fn compare_models(a: &FidelityReport, b: &FidelityReport) -> ModelComparison {
    let b_by_code: HashMap<&str, &QuestionScore> =
        b.questions.iter().map(|q| (q.code.as_str(), q)).collect();
    let rows = a
        .questions
        .iter()
        .filter_map(|qa| {
            let qb = b_by_code.get(qa.code.as_str())?;
            Some(ModelComparisonRow {
                code: qa.code.clone(),
                attribute: qa.attribute.clone(),
                a_tvd: qa.tvd,
                b_tvd: qb.tvd,
                a_sensitive: qa.sensitive,
                b_sensitive: qb.sensitive,
            })
        })
        .collect();
    ModelComparison {
        pack_version: a.pack_version.clone(),
        a_model: a.model.clone(),
        b_model: b.model.clone(),
        a_score: a.score,
        b_score: b.score,
        a_sensitive_share: a.sensitive_share,
        b_sensitive_share: b.sensitive_share,
        rows,
    }
}

pub fn read_pack_file(path: &std::path::Path) -> AppResult<Pack> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| AppError::invalid(format!("cannot read {}: {e}", path.display())))?;
    load_pack(&text)
}

#[cfg(test)]
pub(crate) mod tests;
