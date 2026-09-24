//! Step 5 analysis (docs/DATA_FLOW.md §3, Step 5): per-question aggregates, cross-tabs by
//! respondent attributes, and exports. Everything is computed from the stored answers, so a
//! stopped run reports on what it has, with n shown everywhere.

pub mod export;

use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::db::{runs, surveys};
use crate::error::{AppError, AppResult};
use crate::model::{
    ChartKind, CrossTab, CrossTabGroup, Dimension, Question, QuestionReport, QuestionType, Report,
    ReportRow, Synthesis, SynthesisStatus, ThemeSummary,
};
use crate::sampling::population::{AGE_BANDS, INCOMES};

/// Groups smaller than this are marked "low base".
pub const LOW_BASE: u32 = 30;
const HISTOGRAM_BINS: usize = 8;

/// One stored answer with the respondent's attributes.
#[derive(Debug, Clone)]
pub(crate) struct Answer {
    pub response_id: i64,
    pub question_id: i64,
    pub respondent_id: i64,
    pub status: String,
    pub answer: Value,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Person {
    pub ordinal: u32,
    pub name: String,
    pub age: u32,
    pub gender: String,
    pub country: String,
    pub region: String,
    pub income: String,
    pub occupation: String,
}

impl Person {
    pub fn attr(&self, key: &str) -> String {
        match key {
            "age" => age_band(self.age).to_string(),
            "gender" => self.gender.clone(),
            "country" => self.country.clone(),
            "region" => self.region.clone(),
            "income" => self.income.clone(),
            "occupation" => self.occupation.clone(),
            _ => String::new(),
        }
    }
}

pub fn age_band(age: u32) -> &'static str {
    AGE_BANDS
        .iter()
        .find(|(_, lo, hi)| age >= *lo && (age <= *hi || *lo == 60))
        .map_or("18–29", |(b, _, _)| b)
}

pub(crate) struct RunData {
    pub questions: Vec<Question>,
    pub answers: Vec<Answer>,
    pub people: HashMap<i64, Person>,
    /// response id → theme ids
    pub themes_of: HashMap<i64, Vec<i64>>,
    /// (question id) → themes in display order
    pub themes: HashMap<i64, Vec<(i64, String, String)>>,
}

pub(crate) fn load(conn: &Connection, run_id: i64) -> AppResult<RunData> {
    let run = runs::get(conn, run_id)?;
    let survey = surveys::get(conn, run.survey_id)?;
    // Questions answered in this run, in survey order (a later edit may have retired one).
    let answered: Vec<i64> = conn
        .prepare("SELECT DISTINCT question_id FROM responses WHERE run_id = ?1")?
        .query_map([run_id], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut questions: Vec<Question> = survey.questions.clone();
    for id in answered {
        if !questions.iter().any(|q| q.id == id) {
            questions.push(surveys::question(conn, id)?);
        }
    }
    let answers = conn
        .prepare(
            "SELECT id, question_id, respondent_id, status, answer_json FROM responses WHERE run_id = ?1 ORDER BY respondent_id, question_id",
        )?
        .query_map([run_id], |r| {
            let json: Option<String> = r.get(4)?;
            Ok(Answer {
                response_id: r.get(0)?,
                question_id: r.get(1)?,
                respondent_id: r.get(2)?,
                status: r.get(3)?,
                answer: json
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or(Value::Null),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let people = conn
        .prepare(
            "SELECT id, ordinal, COALESCE(display_name,''), COALESCE(age,0), COALESCE(gender,''), country,
                COALESCE(location,''), COALESCE(income_bracket,''), COALESCE(occupation,'')
             FROM respondents WHERE cohort_id = ?1",
        )?
        .query_map([run.cohort_id], |r| {
            let code: String = r.get(5)?;
            Ok((
                r.get::<_, i64>(0)?,
                Person {
                    ordinal: r.get(1)?,
                    name: r.get(2)?,
                    age: r.get(3)?,
                    gender: r.get(4)?,
                    country: crate::countries::find(&code).map_or(code, |c| c.name.clone()),
                    region: r.get(6)?,
                    income: r.get(7)?,
                    occupation: r.get(8)?,
                },
            ))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?;
    let mut themes: HashMap<i64, Vec<(i64, String, String)>> = HashMap::new();
    for row in conn
        .prepare("SELECT id, question_id, label, COALESCE(description,'') FROM themes WHERE run_id = ?1 ORDER BY id")?
        .query_map([run_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get(2)?, r.get(3)?)))?
    {
        let (id, q, label, desc) = row?;
        themes.entry(q).or_default().push((id, label, desc));
    }
    let mut themes_of: HashMap<i64, Vec<i64>> = HashMap::new();
    for row in conn
        .prepare(
            "SELECT rt.response_id, rt.theme_id FROM response_themes rt JOIN themes t ON t.id = rt.theme_id WHERE t.run_id = ?1",
        )?
        .query_map([run_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
    {
        let (resp, theme) = row?;
        themes_of.entry(resp).or_default().push(theme);
    }
    Ok(RunData {
        questions,
        answers,
        people,
        themes_of,
        themes,
    })
}

fn pct(count: u32, n: u32) -> f64 {
    if n == 0 {
        0.0
    } else {
        (f64::from(count) * 1000.0 / f64::from(n)).round() / 10.0
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Quantile with linear interpolation (Excel's QUARTILE.INC / R type 7).
pub fn quantile(sorted: &[f64], q: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let h = (sorted.len() - 1) as f64 * q;
    let lo = h.floor() as usize;
    let hi = h.ceil() as usize;
    Some(sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo]))
}

pub fn chart_for(q: &Question) -> ChartKind {
    match q.body.question_type {
        QuestionType::SingleChoice if q.body.options.len() <= 6 => ChartKind::Pie,
        QuestionType::SingleChoice => ChartKind::Bar,
        QuestionType::MultiChoice => ChartKind::MultiBar,
        QuestionType::Likert => ChartKind::Diverging,
        QuestionType::Numeric => ChartKind::Histogram,
        QuestionType::OpenEnded => ChartKind::Themes,
    }
}

/// The keys an answer counts toward: option codes, the scale point, or theme ids.
fn keys_of(q: &Question, a: &Answer, data: &RunData) -> Vec<String> {
    match q.body.question_type {
        QuestionType::SingleChoice => a.answer["code"]
            .as_str()
            .map(str::to_string)
            .into_iter()
            .collect(),
        QuestionType::MultiChoice => a.answer["codes"]
            .as_array()
            .map(|v| {
                v.iter()
                    .filter_map(|c| c.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        QuestionType::Likert => a.answer["value"]
            .as_f64()
            .map(|v| format!("{}", v as i64))
            .into_iter()
            .collect(),
        QuestionType::Numeric => Vec::new(),
        QuestionType::OpenEnded => data
            .themes_of
            .get(&a.response_id)
            .map(|t| t.iter().map(|id| id.to_string()).collect())
            .unwrap_or_default(),
    }
}

/// Columns for a question: (key, label), in display order.
fn columns(q: &Question, data: &RunData) -> Vec<(String, String)> {
    match q.body.question_type {
        QuestionType::SingleChoice | QuestionType::MultiChoice => q
            .body
            .options
            .iter()
            .map(|o| (o.code.clone(), o.label.clone()))
            .collect(),
        QuestionType::Likert => {
            let s = q.body.scale.as_ref().expect("likert has a scale");
            (s.min..=s.max)
                .map(|v| {
                    let label = if v == s.min && !s.min_label.is_empty() {
                        format!("{v} – {}", s.min_label)
                    } else if v == s.max && !s.max_label.is_empty() {
                        format!("{v} – {}", s.max_label)
                    } else {
                        v.to_string()
                    };
                    (v.to_string(), label)
                })
                .collect()
        }
        QuestionType::Numeric => Vec::new(),
        QuestionType::OpenEnded => data
            .themes
            .get(&q.id)
            .map(|t| {
                t.iter()
                    .map(|(id, l, _)| (id.to_string(), l.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn question_report(q: &Question, data: &RunData) -> QuestionReport {
    let mine: Vec<&Answer> = data
        .answers
        .iter()
        .filter(|a| a.question_id == q.id)
        .collect();
    let valid: Vec<&Answer> = mine
        .iter()
        .copied()
        .filter(|a| a.status == "valid")
        .collect();
    let n = valid.len() as u32;
    let count_status = |s: &str| mine.iter().filter(|a| a.status == s).count() as u32;
    let mut counts: HashMap<String, u32> = HashMap::new();
    for a in &valid {
        for k in keys_of(q, a, data) {
            *counts.entry(k).or_default() += 1;
        }
    }
    let mut rows: Vec<ReportRow> = columns(q, data)
        .into_iter()
        .map(|(key, label)| {
            let c = counts.get(&key).copied().unwrap_or(0);
            ReportRow {
                key,
                label,
                count: c,
                percent: pct(c, n),
            }
        })
        .collect();
    let values: Vec<f64> = {
        let mut v: Vec<f64> = valid
            .iter()
            .filter_map(|a| a.answer["value"].as_f64())
            .collect();
        v.sort_by(f64::total_cmp);
        v
    };
    let mean = (!values.is_empty()
        && matches!(
            q.body.question_type,
            QuestionType::Likert | QuestionType::Numeric
        ))
    .then(|| round2(values.iter().sum::<f64>() / values.len() as f64));
    let (mut median, mut q1, mut q3, mut unit) = (None, None, None, None);
    if let (QuestionType::Numeric, Some(r)) = (q.body.question_type, q.body.numeric.as_ref()) {
        median = quantile(&values, 0.5).map(round2);
        q1 = quantile(&values, 0.25).map(round2);
        q3 = quantile(&values, 0.75).map(round2);
        unit = Some(r.unit.clone()).filter(|u| !u.is_empty());
        let width = (r.max - r.min) / HISTOGRAM_BINS as f64;
        rows = (0..HISTOGRAM_BINS)
            .map(|i| {
                let lo = r.min + width * i as f64;
                let hi = if i + 1 == HISTOGRAM_BINS {
                    r.max
                } else {
                    lo + width
                };
                let c = values
                    .iter()
                    .filter(|v| **v >= lo && (**v < hi || (i + 1 == HISTOGRAM_BINS && **v <= hi)))
                    .count() as u32;
                ReportRow {
                    key: format!("{lo}"),
                    label: format!("{}–{}", fmt_num(lo), fmt_num(hi)),
                    count: c,
                    percent: pct(c, n),
                }
            })
            .collect();
    }
    let mut themes = Vec::new();
    let mut sample_answers = Vec::new();
    if q.body.question_type == QuestionType::OpenEnded {
        let text = |a: &Answer| a.answer["text"].as_str().unwrap_or_default().to_string();
        for (id, label, description) in data.themes.get(&q.id).into_iter().flatten() {
            let tagged: Vec<&&Answer> = valid
                .iter()
                .filter(|a| {
                    data.themes_of
                        .get(&a.response_id)
                        .is_some_and(|t| t.contains(id))
                })
                .collect();
            let c = tagged.len() as u32;
            themes.push(ThemeSummary {
                id: *id,
                label: label.clone(),
                description: description.clone(),
                count: c,
                percent: pct(c, n),
                quotes: tagged.iter().take(3).map(|a| text(a)).collect(),
            });
        }
        themes.sort_by_key(|t| std::cmp::Reverse(t.count));
        if themes.is_empty() {
            sample_answers = valid.iter().take(5).map(|a| text(a)).collect();
        }
    }
    QuestionReport {
        question_id: q.id,
        code: q.code.clone(),
        text: q.body.text.clone(),
        question_type: q.body.question_type,
        chart: chart_for(q),
        n,
        invalid: count_status("invalid"),
        refused: count_status("refused"),
        rows,
        mean,
        median,
        q1,
        q3,
        unit,
        themes,
        sample_answers,
    }
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        format!("{x:.1}")
    }
}

const DIMENSIONS: [(&str, &str); 6] = [
    ("age", "Age"),
    ("gender", "Gender"),
    ("region", "Region"),
    ("income", "Household income"),
    ("occupation", "Occupation"),
    ("country", "Country"),
];

/// Attributes worth cross-tabbing: those with at least two groups among respondents.
fn dimensions(data: &RunData) -> Vec<Dimension> {
    DIMENSIONS
        .iter()
        .filter(|(k, _)| {
            let mut vals: Vec<String> = data
                .people
                .values()
                .map(|p| p.attr(k))
                .filter(|v| !v.is_empty())
                .collect();
            vals.sort();
            vals.dedup();
            vals.len() > 1
        })
        .map(|(k, l)| Dimension {
            key: k.to_string(),
            label: l.to_string(),
        })
        .collect()
}

pub fn synthesis(conn: &Connection, run_id: i64) -> AppResult<Option<Synthesis>> {
    let row: Option<(String, String, u32, String)> = conn
        .query_row(
            "SELECT content_json, model, based_on_n, created_at FROM syntheses WHERE run_id = ?1 ORDER BY id DESC LIMIT 1",
            [run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((json, model, based_on_n, created_at)) = row else {
        return Ok(None);
    };
    let v: Value = serde_json::from_str(&json)?;
    Ok(Some(Synthesis {
        summary: v["summary"].as_str().unwrap_or_default().to_string(),
        friction_points: serde_json::from_value(v["friction_points"].clone()).unwrap_or_default(),
        segments: serde_json::from_value(v["segments"].clone()).unwrap_or_default(),
        based_on_n,
        model,
        dropped: v["dropped"].as_u64().unwrap_or(0) as u32,
        created_at,
    }))
}

pub fn report(conn: &Connection, run_id: i64) -> AppResult<Report> {
    let run = runs::get(conn, run_id)?;
    let data = load(conn, run_id)?;
    let (status, error): (String, Option<String>) = conn.query_row(
        "SELECT synthesis_status, synthesis_error FROM simulation_runs WHERE id = ?1",
        [run_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(Report {
        based_on_n: run.respondents_done,
        questions: data
            .questions
            .iter()
            .map(|q| question_report(q, &data))
            .collect(),
        dimensions: dimensions(&data),
        synthesis: synthesis(conn, run_id)?,
        synthesis_status: match status.as_str() {
            "generating" => SynthesisStatus::Generating,
            "ready" => SynthesisStatus::Ready,
            "failed" => SynthesisStatus::Failed,
            _ => SynthesisStatus::None,
        },
        synthesis_error: error,
        run,
    })
}

/// Group labels in a sensible order: age and income by their bands, others by size.
fn group_order(key: &str, groups: &BTreeMap<String, Vec<&Answer>>) -> Vec<String> {
    let fixed: Option<Vec<&str>> = match key {
        "age" => Some(AGE_BANDS.iter().map(|(b, _, _)| *b).collect()),
        "income" => Some(INCOMES.to_vec()),
        _ => None,
    };
    let mut labels: Vec<String> = groups.keys().cloned().collect();
    match fixed {
        Some(order) => {
            labels.sort_by_key(|l| order.iter().position(|o| o == l).unwrap_or(usize::MAX))
        }
        None => labels.sort_by(|a, b| groups[b].len().cmp(&groups[a].len()).then(a.cmp(b))),
    }
    labels
}

pub(crate) fn crosstab_from(
    data: &RunData,
    question_id: i64,
    dim: &Dimension,
) -> AppResult<CrossTab> {
    let q = data
        .questions
        .iter()
        .find(|q| q.id == question_id)
        .ok_or_else(|| AppError::not_found(format!("question {question_id} is not in this run")))?;
    let cols = columns(q, data);
    let mut groups: BTreeMap<String, Vec<&Answer>> = BTreeMap::new();
    for a in data
        .answers
        .iter()
        .filter(|a| a.question_id == q.id && a.status == "valid")
    {
        let g = data
            .people
            .get(&a.respondent_id)
            .map(|p| p.attr(&dim.key))
            .unwrap_or_default();
        if !g.is_empty() {
            groups.entry(g).or_default().push(a);
        }
    }
    let order = group_order(&dim.key, &groups);
    let overall = question_report(q, data);
    let out = order
        .iter()
        .map(|label| {
            let members = &groups[label];
            let n = members.len() as u32;
            let mut counts: HashMap<String, u32> = HashMap::new();
            for a in members {
                for k in keys_of(q, a, data) {
                    *counts.entry(k).or_default() += 1;
                }
            }
            let values: Vec<f64> = members
                .iter()
                .filter_map(|a| a.answer["value"].as_f64())
                .collect();
            CrossTabGroup {
                label: label.clone(),
                n,
                low_base: n < LOW_BASE,
                cells: cols
                    .iter()
                    .map(|(k, _)| pct(counts.get(k).copied().unwrap_or(0), n))
                    .collect(),
                mean: (!values.is_empty())
                    .then(|| round2(values.iter().sum::<f64>() / values.len() as f64)),
            }
        })
        .collect();
    Ok(CrossTab {
        question_id,
        dimension: dim.clone(),
        columns: cols
            .into_iter()
            .map(|(key, label)| {
                let row = overall.rows.iter().find(|r| r.key == key);
                ReportRow {
                    count: row.map_or(0, |r| r.count),
                    percent: row.map_or(0.0, |r| r.percent),
                    key,
                    label,
                }
            })
            .collect(),
        groups: out,
    })
}

pub fn crosstab(
    conn: &Connection,
    run_id: i64,
    question_id: i64,
    dimension: &str,
) -> AppResult<CrossTab> {
    let data = load(conn, run_id)?;
    let dim = DIMENSIONS
        .iter()
        .find(|(k, _)| *k == dimension)
        .map(|(k, l)| Dimension {
            key: k.to_string(),
            label: l.to_string(),
        })
        .ok_or_else(|| AppError::invalid(format!("unknown dimension {dimension}")))?;
    crosstab_from(&data, question_id, &dim)
}

pub(crate) fn set_synthesis_status(
    conn: &Connection,
    run_id: i64,
    status: crate::model::SynthesisStatus,
    error: Option<&str>,
) -> AppResult<()> {
    let s = match status {
        SynthesisStatus::None => "none",
        SynthesisStatus::Generating => "generating",
        SynthesisStatus::Ready => "ready",
        SynthesisStatus::Failed => "failed",
    };
    conn.execute(
        "UPDATE simulation_runs SET synthesis_status = ?2, synthesis_error = ?3 WHERE id = ?1",
        params![run_id, s, error],
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod fixture;

#[cfg(test)]
mod tests {
    use super::fixture::Fixture;
    use super::*;

    #[test]
    fn quantiles_match_excel_quartile_inc() {
        let v = [100.0, 200.0, 300.0, 400.0, 1000.0];
        assert_eq!(quantile(&v, 0.5), Some(300.0));
        assert_eq!(quantile(&v, 0.25), Some(200.0));
        assert_eq!(quantile(&v, 0.75), Some(400.0));
        assert_eq!(quantile(&[1.0, 2.0], 0.5), Some(1.5));
        assert_eq!(quantile(&[], 0.5), None);
    }

    /// Golden fixture: 40 respondents with hand-set answers (see fixture.rs for the numbers).
    #[test]
    fn report_numbers_match_the_golden_fixture() {
        let f = Fixture::build();
        let r = report(&f.conn, f.run_id).unwrap();
        assert_eq!(r.based_on_n, 40);
        let q = |code: &str| r.questions.iter().find(|q| q.code == code).unwrap();

        let brand = q("BRAND");
        assert_eq!(brand.chart, ChartKind::Pie);
        assert_eq!((brand.n, brand.invalid, brand.refused), (38, 1, 1));
        let counts: Vec<(u32, f64)> = brand.rows.iter().map(|r| (r.count, r.percent)).collect();
        assert_eq!(counts, [(19, 50.0), (12, 31.6), (7, 18.4)]);

        let drivers = q("DRIVERS");
        assert_eq!(drivers.chart, ChartKind::MultiBar);
        let p: Vec<f64> = drivers.rows.iter().map(|r| r.percent).collect();
        assert_eq!(p, [75.0, 50.0, 25.0]); // totals over 100%

        let intent = q("INTENT");
        assert_eq!(intent.chart, ChartKind::Diverging);
        assert_eq!(intent.rows.len(), 5);
        assert_eq!(intent.rows[0].label, "1 – Not likely");
        assert_eq!(intent.mean, Some(3.0));
        assert_eq!(
            intent.rows.iter().map(|r| r.count).collect::<Vec<_>>(),
            [8, 8, 8, 8, 8]
        );

        let budget = q("BUDGET");
        assert_eq!(budget.chart, ChartKind::Histogram);
        assert_eq!(
            (budget.median, budget.q1, budget.q3),
            (Some(650.0), Some(400.0), Some(900.0))
        );
        assert_eq!(budget.rows.iter().map(|r| r.count).sum::<u32>(), 40);
        assert_eq!(budget.unit.as_deref(), Some("CAD"));

        let why = q("WHY");
        assert_eq!(why.chart, ChartKind::Themes);
        assert_eq!(why.themes[0].label, "Price");
        assert_eq!((why.themes[0].count, why.themes[0].percent), (24, 60.0));
        assert_eq!(why.themes[1].count, 10);
        assert!(why.themes[0].quotes.len() <= 3);

        let keys: Vec<&str> = r.dimensions.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, ["age", "gender", "income"]);
    }

    #[test]
    fn crosstabs_are_within_group_percentages_with_low_base_flags() {
        let f = Fixture::build();
        let ct = crosstab(&f.conn, f.run_id, f.q("BRAND"), "gender").unwrap();
        assert_eq!(
            ct.groups
                .iter()
                .map(|g| g.label.as_str())
                .collect::<Vec<_>>(),
            ["Female", "Male"]
        );
        let female = &ct.groups[0];
        assert_eq!(female.n, 19);
        assert!(female.low_base);
        // All women chose A in the fixture.
        assert_eq!(female.cells, [100.0, 0.0, 0.0]);
        let intent = crosstab(&f.conn, f.run_id, f.q("INTENT"), "age").unwrap();
        assert_eq!(intent.groups[0].label, "18–29");
        assert!(intent.groups.iter().all(|g| g.mean.is_some()));
        let n: u32 = intent.groups.iter().map(|g| g.n).sum();
        assert_eq!(n, 40);
        assert!(crosstab(&f.conn, f.run_id, f.q("INTENT"), "shoe_size").is_err());
    }

    #[test]
    fn a_stopped_run_reports_on_what_it_has() {
        let f = Fixture::build();
        f.conn
            .execute("DELETE FROM responses WHERE respondent_id IN (SELECT id FROM respondents WHERE ordinal > 30)", [])
            .unwrap();
        f.conn
            .execute("UPDATE simulation_runs SET status = 'stopped'", [])
            .unwrap();
        let r = report(&f.conn, f.run_id).unwrap();
        assert_eq!(r.based_on_n, 30);
        assert!(r
            .questions
            .iter()
            .all(|q| q.n + q.invalid + q.refused == 30));
    }
}
