//! Draws demographic skeletons: who is in the cohort. No LLM involved.
//!
//! 1. Build weighted cells: the census table for each selected country, or (for countries
//!    without one) every combination of the quota values with a neutral occupation mix.
//! 2. Rake the cell weights to the quota percentages (iterative proportional fitting).
//! 3. Apportion each quota group to exact integer counts (largest remainder).
//! 4. Draw people one at a time, only from cells whose quota values still have room, with
//!    probability ∝ raked weight × remaining room. Every group's counts come out exact.

use std::collections::HashSet;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use super::population::{self, Cell, AGE_BANDS, GENDERS, NON_BINARY_LABEL};
use super::{apportion, validate};
use crate::countries;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::model::CohortConfig;

#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    pub ordinal: u32,
    pub country: String,
    pub age: u32,
    pub age_band: String,
    pub gender: String,
    pub region: String,
    pub income: String,
    pub occupation: String,
    /// e.g. "age=30–44|region=Ontario|income=Over $100k" — the quota values this person fills.
    pub quota_cell: String,
}

pub struct Sampler {
    cells: Vec<Cell>,
    raked: Vec<f64>,
    quota_keys: Vec<String>,
    config: CohortConfig,
    ages: Vec<(String, Vec<(u32, f64)>)>,
    /// Ordinals reassigned to `NON_BINARY_LABEL` (BACKLOG B7), so `redraw` keeps the same
    /// respondent non-binary if it re-picks them.
    non_binary: HashSet<u32>,
}

impl Sampler {
    /// `country_codes` are the project's Target Countries (ISO codes).
    pub fn new(config: &CohortConfig, country_codes: &[String]) -> AppResult<Self> {
        validate(config)?;
        if country_codes.is_empty() {
            return Err(AppError::invalid("choose at least one country"));
        }
        let mut config = config.clone();
        // Country quota rows use display names in the UI; the cells use ISO codes.
        for g in config.quotas.iter_mut().filter(|g| g.key == "country") {
            for r in g.rows.iter_mut() {
                let code = countries::list()
                    .iter()
                    .find(|c| c.name == r.label || c.code == r.label)
                    .map(|c| c.code.clone())
                    .ok_or_else(|| {
                        AppError::invalid(format!("unknown country \"{}\" in quotas", r.label))
                    })?;
                if !country_codes.contains(&code) {
                    return Err(AppError::invalid(format!(
                        "{} is in the quotas but not a Target Country",
                        r.label
                    )));
                }
                r.label = code;
            }
        }
        if country_codes.len() > 1 && !config.quotas.iter().any(|g| g.key == "country") {
            return Err(AppError::invalid(
                "several countries need a Country quota group",
            ));
        }
        let quota_keys: Vec<String> = config.quotas.iter().map(|g| g.key.clone()).collect();
        for k in &quota_keys {
            if !matches!(
                k.as_str(),
                "country" | "age" | "gender" | "region" | "income" | "occupation"
            ) {
                return Err(AppError::invalid(format!(
                    "unsupported quota dimension \"{k}\""
                )));
            }
        }
        if config.non_binary_share > 0 && quota_keys.iter().any(|k| k == "gender") {
            return Err(AppError::invalid(
                "a gender quota group and a non-binary share can't both be set",
            ));
        }
        let non_binary = non_binary_ordinals(config.size, config.non_binary_share, config.seed)?;

        let mut cells = Vec::new();
        let mut ages = Vec::new();
        for code in country_codes {
            match population::table(code) {
                Some(t) => {
                    cells.extend(t.cells.iter().cloned());
                    ages.push((code.clone(), t.ages.clone()));
                }
                None => cells.extend(synthetic_cells(code, &config)),
            }
        }
        // Every quota value must exist somewhere, or it could never be filled.
        for g in &config.quotas {
            for r in g.rows.iter().filter(|r| r.percent > 0) {
                if !cells
                    .iter()
                    .any(|c| c.value(&g.key) == Some(r.label.as_str()))
                {
                    return Err(AppError::invalid(format!(
                        "{}: \"{}\" is not a known value",
                        g.label, r.label
                    )));
                }
            }
        }
        let raked = rake(&cells, &config)?;
        Ok(Self {
            cells,
            raked,
            quota_keys,
            config,
            ages,
            non_binary,
        })
    }

    /// Draws `config.size` skeletons. The same config, countries and seed give the same result.
    pub fn draw(&self) -> AppResult<Vec<Skeleton>> {
        let n = self.config.size;
        let mut remaining: Vec<Vec<(String, u32)>> = Vec::new();
        for g in &self.config.quotas {
            let counts = apportion(&g.rows.iter().map(|r| r.percent).collect::<Vec<_>>(), n)?;
            remaining.push(
                g.rows
                    .iter()
                    .zip(counts)
                    .map(|(r, c)| (r.label.clone(), c))
                    .collect(),
            );
        }
        let mut rng = ChaCha8Rng::seed_from_u64(self.config.seed);
        let mut out = Vec::with_capacity(n as usize);
        for ordinal in 1..=n {
            let mut scores = Vec::with_capacity(self.cells.len());
            for (i, c) in self.cells.iter().enumerate() {
                let mut s = self.raked[i];
                for (gi, key) in self.quota_keys.iter().enumerate() {
                    let v = c.value(key).unwrap_or_default();
                    s *= f64::from(
                        remaining[gi]
                            .iter()
                            .find(|(l, _)| l == v)
                            .map_or(0, |(_, r)| *r),
                    );
                }
                scores.push(s);
            }
            let i = pick(&scores, &mut rng).ok_or_else(|| {
                AppError::new(
                    ErrorCode::Internal,
                    "no cell fits the remaining quotas; try different quotas",
                )
            })?;
            for (gi, key) in self.quota_keys.iter().enumerate() {
                let v = self.cells[i].value(key).unwrap_or_default();
                if let Some(slot) = remaining[gi].iter_mut().find(|(l, _)| l == v) {
                    slot.1 -= 1;
                }
            }
            out.push(self.skeleton(ordinal, i, &mut rng));
        }
        Ok(out)
    }

    /// Replaces a skeleton that failed screening with another from the same quota cell, so
    /// every group's counts stay exact. `attempt` makes each redraw different but reproducible.
    pub fn redraw(&self, failed: &Skeleton, attempt: u32) -> AppResult<Skeleton> {
        let seed = self.config.seed
            ^ (u64::from(failed.ordinal) << 20)
            ^ (u64::from(attempt) << 48)
            ^ 0x5eed;
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let scores: Vec<f64> = self
            .cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if quota_cell(c, &self.quota_keys) == failed.quota_cell {
                    self.raked[i]
                } else {
                    0.0
                }
            })
            .collect();
        let i = pick(&scores, &mut rng)
            .ok_or_else(|| AppError::new(ErrorCode::Internal, "quota cell has no population"))?;
        Ok(self.skeleton(failed.ordinal, i, &mut rng))
    }

    fn skeleton(&self, ordinal: u32, i: usize, rng: &mut ChaCha8Rng) -> Skeleton {
        let c = &self.cells[i];
        let (_, lo, hi) = AGE_BANDS
            .iter()
            .find(|(b, _, _)| *b == c.age_band)
            .copied()
            .unwrap_or(("", 18, 84));
        let age = match self.ages.iter().find(|(code, _)| *code == c.country) {
            Some((_, dist)) => {
                let within: Vec<f64> = dist
                    .iter()
                    .map(|(a, w)| {
                        if (lo..=hi.max(if lo == 60 { 120 } else { hi })).contains(a) {
                            *w
                        } else {
                            0.0
                        }
                    })
                    .collect();
                pick(&within, rng).map_or(lo, |k| dist[k].0)
            }
            None => rng.random_range(lo..=hi),
        };
        let gender = if self.non_binary.contains(&ordinal) {
            NON_BINARY_LABEL.to_string()
        } else {
            c.gender.clone()
        };
        Skeleton {
            ordinal,
            country: c.country.clone(),
            age,
            age_band: c.age_band.clone(),
            gender,
            region: c.region.clone(),
            income: c.income.clone(),
            occupation: c.occupation.clone(),
            quota_cell: quota_cell(c, &self.quota_keys),
        }
    }
}

fn quota_cell(c: &Cell, keys: &[String]) -> String {
    keys.iter()
        .map(|k| format!("{k}={}", c.value(k).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("|")
}

/// Picks exactly `share`% of ordinals `1..=n` (largest-remainder exact) to reassign to
/// `NON_BINARY_LABEL`. Keyed by ordinal, not draw order, so `redraw` reproduces the same
/// choice for a respondent it replaces.
fn non_binary_ordinals(n: u32, share: u8, seed: u64) -> AppResult<HashSet<u32>> {
    if share == 0 {
        return Ok(HashSet::new());
    }
    let count = apportion(&[u32::from(share), 100 - u32::from(share)], n)?[0] as usize;
    let mut ordinals: Vec<u32> = (1..=n).collect();
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x6e6f6e5f62696e61); // distinct stream from draw/redraw
    for i in (1..ordinals.len()).rev() {
        let j = rng.random_range(0..=i);
        ordinals.swap(i, j);
    }
    Ok(ordinals.into_iter().take(count).collect())
}

/// Weighted choice; None when every weight is zero.
fn pick(weights: &[f64], rng: &mut ChaCha8Rng) -> Option<usize> {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return None;
    }
    let mut x = rng.random::<f64>() * total;
    for (i, w) in weights.iter().enumerate() {
        if *w > 0.0 {
            if x < *w {
                return Some(i);
            }
            x -= w;
        }
    }
    weights.iter().rposition(|w| *w > 0.0)
}

/// Cells for a country without a census table: every combination of its quota values,
/// both genders and the neutral occupation mix. Dimensions without quotas get one value.
fn synthetic_cells(code: &str, config: &CohortConfig) -> Vec<Cell> {
    let values = |key: &str, default: &[&str]| -> Vec<String> {
        config
            .quotas
            .iter()
            .find(|g| g.key == key)
            .map(|g| g.rows.iter().map(|r| r.label.clone()).collect())
            .unwrap_or_else(|| default.iter().map(|s| s.to_string()).collect())
    };
    let ages = values("age", &AGE_BANDS.map(|(b, _, _)| b));
    let incomes = values("income", &population::INCOMES);
    let regions = values("region", &[""]);
    let occupations = population::fallback_occupations();
    let mut cells = Vec::new();
    for a in &ages {
        for g in GENDERS {
            for r in &regions {
                for i in &incomes {
                    for (o, w) in &occupations {
                        cells.push(Cell {
                            country: code.to_string(),
                            age_band: a.clone(),
                            gender: g.to_string(),
                            region: r.clone(),
                            income: i.clone(),
                            occupation: o.clone(),
                            weight: *w,
                        });
                    }
                }
            }
        }
    }
    cells
}

/// Iterative proportional fitting: scale cell weights until each quota group's shares match.
fn rake(cells: &[Cell], config: &CohortConfig) -> AppResult<Vec<f64>> {
    let mut w: Vec<f64> = cells.iter().map(|c| c.weight.max(0.0)).collect();
    for _ in 0..200 {
        let mut max_err: f64 = 0.0;
        for g in &config.quotas {
            let total: f64 = w.iter().sum();
            for r in &g.rows {
                let target = f64::from(r.percent) / 100.0;
                let idx: Vec<usize> = cells
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.value(&g.key) == Some(r.label.as_str()))
                    .map(|(i, _)| i)
                    .collect();
                let share: f64 = idx.iter().map(|&i| w[i]).sum::<f64>() / total;
                if share <= 0.0 {
                    if target > 0.0 {
                        return Err(AppError::invalid(format!(
                            "{}: \"{}\" has no population",
                            g.label, r.label
                        )));
                    }
                    continue;
                }
                max_err = max_err.max((share - target).abs());
                let f = target / share;
                for i in idx {
                    w[i] *= f;
                }
            }
        }
        if max_err < 1e-9 {
            break;
        }
    }
    Ok(w)
}

/// Census-based default quotas for Step 1: a representative sample unless the user edits them.
pub fn census_default_quotas(code: &str) -> Option<Vec<crate::model::QuotaGroup>> {
    use crate::model::{QuotaGroup, QuotaRow};
    let t = population::table(code)?;
    let group = |key: &str, label: &str, order: &[&str]| -> QuotaGroup {
        let m = population::marginal(t, key);
        let shares: Vec<f64> = order
            .iter()
            .map(|v| m.iter().find(|(k, _)| k == v).map_or(0.0, |(_, s)| *s))
            .collect();
        let percents = round_to_100(&shares);
        QuotaGroup {
            key: key.into(),
            label: label.into(),
            rows: order
                .iter()
                .zip(percents)
                .map(|(v, p)| QuotaRow {
                    label: v.to_string(),
                    percent: p,
                })
                .collect(),
        }
    };
    let regions: Vec<&str> = countries::find(code)?
        .regions
        .iter()
        .map(String::as_str)
        .collect();
    let mut groups = vec![group("age", "Age", &AGE_BANDS.map(|(b, _, _)| b))];
    if !regions.is_empty() {
        groups.push(group("region", "Region", &regions));
    }
    groups.push(group("income", "Household income", &population::INCOMES));
    Some(groups)
}

/// Whole-number percentages that total exactly 100 (largest remainder).
fn round_to_100(shares: &[f64]) -> Vec<u32> {
    let total: f64 = shares.iter().sum();
    let exact: Vec<f64> = shares.iter().map(|s| s / total * 100.0).collect();
    let mut out: Vec<u32> = exact.iter().map(|x| x.floor() as u32).collect();
    let mut left = 100 - out.iter().sum::<u32>();
    let mut order: Vec<usize> = (0..exact.len()).collect();
    order.sort_by(|&a, &b| {
        (exact[b] - exact[b].floor())
            .total_cmp(&(exact[a] - exact[a].floor()))
            .then(a.cmp(&b))
    });
    for i in order {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{QuotaGroup, QuotaRow};

    fn g(key: &str, rows: &[(&str, u32)]) -> QuotaGroup {
        QuotaGroup {
            key: key.into(),
            label: key.into(),
            rows: rows
                .iter()
                .map(|(l, p)| QuotaRow {
                    label: l.to_string(),
                    percent: *p,
                })
                .collect(),
        }
    }

    fn ca_config(size: u32, seed: u64) -> CohortConfig {
        CohortConfig {
            size,
            seed,
            quotas: vec![
                g(
                    "age",
                    &[("18–29", 25), ("30–44", 30), ("45–59", 25), ("60+", 20)],
                ),
                g(
                    "region",
                    &[
                        ("Atlantic", 7),
                        ("Quebec", 23),
                        ("Ontario", 38),
                        ("Prairies", 17),
                        ("British Columbia", 14),
                        ("Territories", 1),
                    ],
                ),
                g(
                    "income",
                    &[("Under $50k", 30), ("$50k–$100k", 40), ("Over $100k", 30)],
                ),
            ],
            screening: String::new(),
            non_binary_share: 0,
            countries: vec!["CA".into()],
        }
    }

    fn counts(skel: &[Skeleton], f: impl Fn(&Skeleton) -> &str, label: &str) -> u32 {
        skel.iter().filter(|s| f(s) == label).count() as u32
    }

    #[test]
    fn ca_counts_are_exact_for_every_group() {
        for size in [1, 7, 100, 200, 1000] {
            let cfg = ca_config(size, 42);
            let skel = Sampler::new(&cfg, &["CA".into()]).unwrap().draw().unwrap();
            assert_eq!(skel.len() as u32, size);
            for grp in &cfg.quotas {
                let want = apportion(
                    &grp.rows.iter().map(|r| r.percent).collect::<Vec<_>>(),
                    size,
                )
                .unwrap();
                for (r, w) in grp.rows.iter().zip(want) {
                    let got = match grp.key.as_str() {
                        "age" => counts(&skel, |s| &s.age_band, &r.label),
                        "region" => counts(&skel, |s| &s.region, &r.label),
                        _ => counts(&skel, |s| &s.income, &r.label),
                    };
                    assert_eq!(got, w, "size {size}, {} {}", grp.key, r.label);
                }
            }
        }
    }

    #[test]
    fn same_seed_same_cohort_different_seed_different_cohort() {
        let a = Sampler::new(&ca_config(200, 7), &["CA".into()])
            .unwrap()
            .draw()
            .unwrap();
        let b = Sampler::new(&ca_config(200, 7), &["CA".into()])
            .unwrap()
            .draw()
            .unwrap();
        let c = Sampler::new(&ca_config(200, 8), &["CA".into()])
            .unwrap()
            .draw()
            .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn ages_fall_inside_their_band() {
        let skel = Sampler::new(&ca_config(500, 3), &["CA".into()])
            .unwrap()
            .draw()
            .unwrap();
        for s in &skel {
            let (_, lo, hi) = AGE_BANDS.iter().find(|(b, _, _)| *b == s.age_band).unwrap();
            let hi = if *lo == 60 { 120 } else { *hi };
            assert!(
                (*lo..=hi).contains(&s.age),
                "{} not in {}",
                s.age,
                s.age_band
            );
        }
    }

    #[test]
    fn unquoted_dimensions_follow_the_census() {
        // No gender quota: the female share should be near the census 51%.
        let skel = Sampler::new(&ca_config(1000, 11), &["CA".into()])
            .unwrap()
            .draw()
            .unwrap();
        let female = counts(&skel, |s| &s.gender, "Female");
        assert!((440..=580).contains(&female), "female count {female}");
    }

    #[test]
    fn several_countries_including_one_without_census_data() {
        let cfg = CohortConfig {
            size: 150,
            seed: 5,
            quotas: vec![
                g("country", &[("Canada", 60), ("United States", 40)]),
                g(
                    "age",
                    &[("18–29", 25), ("30–44", 30), ("45–59", 25), ("60+", 20)],
                ),
                g(
                    "income",
                    &[("Under $50k", 30), ("$50k–$100k", 40), ("Over $100k", 30)],
                ),
            ],
            screening: String::new(),
            non_binary_share: 0,
            countries: vec!["CA".into(), "US".into()],
        };
        let skel = Sampler::new(&cfg, &["CA".into(), "US".into()])
            .unwrap()
            .draw()
            .unwrap();
        assert_eq!(counts(&skel, |s| &s.country, "CA"), 90);
        assert_eq!(counts(&skel, |s| &s.country, "US"), 60);
        assert_eq!(counts(&skel, |s| &s.age_band, "30–44"), 45);
        assert!(skel
            .iter()
            .filter(|s| s.country == "US")
            .all(|s| (18..=84).contains(&s.age)));
    }

    #[test]
    fn bad_inputs_are_rejected() {
        let mut cfg = ca_config(100, 1);
        cfg.quotas[0].rows[0].label = "Teenagers".into();
        assert_eq!(
            Sampler::new(&cfg, &["CA".into()]).err().unwrap().code,
            ErrorCode::InvalidInput
        );
        assert!(Sampler::new(&ca_config(100, 1), &[]).is_err());
        // Two countries without a country quota group.
        assert!(Sampler::new(&ca_config(100, 1), &["CA".into(), "US".into()]).is_err());
    }

    #[test]
    fn non_binary_share_is_exact_and_leaves_other_quotas_alone() {
        let mut cfg = ca_config(1000, 4);
        cfg.non_binary_share = 7;
        let skel = Sampler::new(&cfg, &["CA".into()]).unwrap().draw().unwrap();
        assert_eq!(counts(&skel, |s| &s.gender, NON_BINARY_LABEL), 70);
        for grp in &cfg.quotas {
            let want = apportion(
                &grp.rows.iter().map(|r| r.percent).collect::<Vec<_>>(),
                1000,
            )
            .unwrap();
            for (r, w) in grp.rows.iter().zip(want) {
                let got = match grp.key.as_str() {
                    "age" => counts(&skel, |s| &s.age_band, &r.label),
                    "region" => counts(&skel, |s| &s.region, &r.label),
                    _ => counts(&skel, |s| &s.income, &r.label),
                };
                assert_eq!(
                    got, w,
                    "{} {} unaffected by non_binary_share",
                    grp.key, r.label
                );
            }
        }
    }

    #[test]
    fn redraw_keeps_the_same_respondent_non_binary() {
        let mut cfg = ca_config(50, 9);
        cfg.non_binary_share = 20;
        let s = Sampler::new(&cfg, &["CA".into()]).unwrap();
        let skel = s.draw().unwrap();
        let non_binary_idx = skel
            .iter()
            .position(|p| p.gender == NON_BINARY_LABEL)
            .expect("at least one non-binary respondent in 50 at a 20% share");
        let redrawn = s.redraw(&skel[non_binary_idx], 1).unwrap();
        assert_eq!(redrawn.gender, NON_BINARY_LABEL);
    }

    #[test]
    fn non_binary_share_conflicts_with_a_gender_quota_group() {
        let mut cfg = ca_config(100, 1);
        cfg.non_binary_share = 5;
        cfg.quotas
            .push(g("gender", &[("Female", 50), ("Male", 50)]));
        assert_eq!(
            Sampler::new(&cfg, &["CA".into()]).err().unwrap().code,
            ErrorCode::InvalidInput
        );
    }

    #[test]
    fn redraw_keeps_the_quota_cell_and_is_reproducible() {
        let s = Sampler::new(&ca_config(50, 9), &["CA".into()]).unwrap();
        let skel = s.draw().unwrap();
        let r1 = s.redraw(&skel[10], 1).unwrap();
        let r1b = s.redraw(&skel[10], 1).unwrap();
        assert_eq!(r1, r1b);
        assert_eq!(r1.quota_cell, skel[10].quota_cell);
        assert_eq!(r1.ordinal, skel[10].ordinal);
    }

    #[test]
    fn census_defaults_total_100_and_follow_the_table() {
        let groups = census_default_quotas("CA").unwrap();
        assert_eq!(
            groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(),
            ["age", "region", "income"]
        );
        for grp in &groups {
            assert_eq!(
                grp.rows.iter().map(|r| r.percent).sum::<u32>(),
                100,
                "{}",
                grp.key
            );
        }
        let ontario = groups[1]
            .rows
            .iter()
            .find(|r| r.label == "Ontario")
            .unwrap()
            .percent;
        assert!((36..=42).contains(&ontario), "Ontario share {ontario}");
        assert!(census_default_quotas("US").is_none());
    }
}
