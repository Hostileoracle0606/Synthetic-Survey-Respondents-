//! Who is in the cohort. Head counts must add up to the sample size exactly, so every
//! group is apportioned with the largest-remainder method; `skeleton` draws the people.

pub mod population;
pub mod skeleton;

pub use skeleton::{census_default_quotas, Sampler, Skeleton};

use crate::error::{AppError, AppResult};
use crate::model::CohortConfig;

pub const MAX_COHORT: u32 = 1_000;

/// Splits `n` people across `percents` (which must total 100) so the counts total exactly `n`.
/// Ties in the remainder go to the earlier row, so results are deterministic.
pub fn apportion(percents: &[u32], n: u32) -> AppResult<Vec<u32>> {
    let total: u32 = percents.iter().sum();
    if total != 100 {
        return Err(AppError::invalid(format!(
            "quota group totals {total}%, must be 100%"
        )));
    }
    let exact: Vec<u64> = percents
        .iter()
        .map(|&p| u64::from(p) * u64::from(n))
        .collect();
    let mut counts: Vec<u32> = exact.iter().map(|&x| (x / 100) as u32).collect();
    let mut left = n - counts.iter().sum::<u32>();
    let mut order: Vec<usize> = (0..percents.len()).collect();
    order.sort_by(|&a, &b| (exact[b] % 100).cmp(&(exact[a] % 100)).then(a.cmp(&b)));
    for i in order {
        if left == 0 {
            break;
        }
        counts[i] += 1;
        left -= 1;
    }
    Ok(counts)
}

/// The Step 1 gate for the audience section, enforced again here so the UI can't bypass it.
pub fn validate(config: &CohortConfig) -> AppResult<()> {
    if config.size == 0 || config.size > MAX_COHORT {
        return Err(AppError::invalid(format!(
            "number of respondents must be 1–{MAX_COHORT}"
        )));
    }
    if config.non_binary_share > 100 {
        return Err(AppError::invalid("non-binary share must be 0-100"));
    }
    if config.quotas.is_empty() {
        return Err(AppError::invalid("at least one quota group is required"));
    }
    for g in &config.quotas {
        if g.rows.is_empty() {
            return Err(AppError::invalid(format!(
                "quota group \"{}\" has no rows",
                g.label
            )));
        }
        let percents: Vec<u32> = g.rows.iter().map(|r| r.percent).collect();
        apportion(&percents, config.size)
            .map_err(|e| AppError::invalid(format!("{}: {}", g.label, e.message)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{QuotaGroup, QuotaRow};
    use rand::{Rng, SeedableRng};

    #[test]
    fn exact_counts_for_fixed_cases() {
        assert_eq!(
            apportion(&[25, 30, 25, 20], 200).unwrap(),
            vec![50, 60, 50, 40]
        );
        assert_eq!(apportion(&[33, 33, 34], 7).unwrap(), vec![2, 2, 3]);
        assert_eq!(apportion(&[50, 50], 1).unwrap(), vec![1, 0]);
        assert_eq!(
            apportion(&[17, 38, 21, 24], 1000).unwrap(),
            vec![170, 380, 210, 240]
        );
    }

    #[test]
    fn rejects_totals_other_than_100() {
        assert!(apportion(&[40, 40], 100).is_err());
        assert!(apportion(&[60, 50], 100).is_err());
    }

    #[test]
    fn random_configs_always_total_n() {
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(7);
        for _ in 0..2_000 {
            let k = rng.random_range(1..=6);
            let mut left = 100u32;
            let mut percents = Vec::new();
            for i in 0..k {
                let p = if i == k - 1 {
                    left
                } else {
                    rng.random_range(0..=left)
                };
                percents.push(p);
                left -= p;
            }
            let n = rng.random_range(1..=MAX_COHORT);
            let counts = apportion(&percents, n).unwrap();
            assert_eq!(counts.iter().sum::<u32>(), n, "{percents:?} n={n}");
            for (c, p) in counts.iter().zip(&percents) {
                let ideal = f64::from(*p) * f64::from(n) / 100.0;
                assert!((f64::from(*c) - ideal).abs() < 1.0, "{percents:?} n={n}");
            }
        }
    }

    #[test]
    fn validate_checks_size_and_every_group() {
        let group = |p: &[u32]| QuotaGroup {
            key: "age".into(),
            label: "Age".into(),
            rows: p
                .iter()
                .enumerate()
                .map(|(i, &percent)| QuotaRow {
                    label: format!("g{i}"),
                    percent,
                })
                .collect(),
        };
        let mut c = CohortConfig {
            size: 200,
            seed: 1,
            quotas: vec![group(&[50, 50])],
            screening: String::new(),
            non_binary_share: 0,
            countries: vec![],
        };
        assert!(validate(&c).is_ok());
        c.size = 0;
        assert!(validate(&c).is_err());
        c.size = 200;
        c.quotas.push(group(&[40, 40]));
        assert_eq!(
            validate(&c).unwrap_err().code,
            crate::ErrorCode::InvalidInput
        );
        c.quotas.pop();
        c.non_binary_share = 101;
        assert_eq!(
            validate(&c).unwrap_err().code,
            crate::ErrorCode::InvalidInput
        );
    }
}

/// Step 1 default quota groups for a country selection (docs/DATA_FLOW.md, Step 1):
/// census shares for one census country; generic shares otherwise, plus an even Country
/// group when several countries are selected.
pub fn default_quotas(country_codes: &[String]) -> Vec<crate::model::QuotaGroup> {
    use crate::model::{QuotaGroup, QuotaRow};
    if let [only] = country_codes {
        if let Some(groups) = census_default_quotas(only) {
            return groups;
        }
    }
    let rows = |pairs: &[(&str, u32)]| -> Vec<QuotaRow> {
        pairs
            .iter()
            .map(|(l, p)| QuotaRow {
                label: l.to_string(),
                percent: *p,
            })
            .collect()
    };
    let mut groups = Vec::new();
    if country_codes.len() > 1 {
        let names: Vec<String> = country_codes
            .iter()
            .map(|c| crate::countries::find(c).map_or_else(|| c.clone(), |o| o.name.clone()))
            .collect();
        let n = names.len() as u32;
        let base = 100 / n;
        groups.push(QuotaGroup {
            key: "country".into(),
            label: "Country".into(),
            rows: names
                .into_iter()
                .enumerate()
                .map(|(i, label)| QuotaRow {
                    label,
                    percent: base + if i == 0 { 100 - base * n } else { 0 },
                })
                .collect(),
        });
    }
    groups.push(QuotaGroup {
        key: "age".into(),
        label: "Age".into(),
        rows: rows(&[("18–29", 25), ("30–44", 30), ("45–59", 25), ("60+", 20)]),
    });
    if let [only] = country_codes {
        if let Some(c) = crate::countries::find(only) {
            if !c.regions.is_empty() {
                let n = c.regions.len() as u32;
                let base = 100 / n;
                groups.push(QuotaGroup {
                    key: "region".into(),
                    label: "Region".into(),
                    rows: c
                        .regions
                        .iter()
                        .enumerate()
                        .map(|(i, r)| QuotaRow {
                            label: r.clone(),
                            percent: base + if i == 0 { 100 - base * n } else { 0 },
                        })
                        .collect(),
                });
            }
        }
    }
    groups.push(QuotaGroup {
        key: "income".into(),
        label: "Household income".into(),
        rows: rows(&[("Under $50k", 30), ("$50k–$100k", 40), ("Over $100k", 30)]),
    });
    groups
}

#[cfg(test)]
mod default_tests {
    use super::*;

    #[test]
    fn defaults_follow_the_country_selection() {
        let keys = |c: &[&str]| -> Vec<String> {
            default_quotas(&c.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                .into_iter()
                .map(|g| g.key)
                .collect()
        };
        assert_eq!(keys(&["CA"]), ["age", "region", "income"]);
        assert_eq!(keys(&["US"]), ["age", "income"]);
        assert_eq!(keys(&["US", "CA", "GB"]), ["country", "age", "income"]);
        for g in default_quotas(&["US".into(), "CA".into(), "GB".into()]) {
            assert_eq!(g.rows.iter().map(|r| r.percent).sum::<u32>(), 100);
        }
    }
}
