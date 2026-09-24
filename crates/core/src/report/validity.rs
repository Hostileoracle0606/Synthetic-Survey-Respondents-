//! Label-free response checks (docs/SPEC.md §8; TEST_PLAN S13 variance, midpoint rate and
//! order effect). They flag answers that look like LLM artefacts rather than opinions.

use crate::model::{Question, QuestionType, ReportRow, Validity};

/// Normalised entropy below this means answers collapsed onto one option.
pub const MIN_ENTROPY: f64 = 0.4;
/// Likert midpoint share above this (percent) is flagged.
pub const MAX_MIDPOINT: f64 = 60.0;
/// Order effect: first vs last position difference (points) that is flagged when significant.
pub const ORDER_GAP: f64 = 5.0;

/// Shannon entropy of the counts divided by ln(k); None when fewer than 2 options or no answers.
pub fn normalised_entropy(counts: &[u32]) -> Option<f64> {
    let n: u32 = counts.iter().sum();
    if counts.len() < 2 || n == 0 {
        return None;
    }
    let h: f64 = counts
        .iter()
        .filter(|c| **c > 0)
        .map(|c| {
            let p = f64::from(*c) / f64::from(n);
            -p * p.ln()
        })
        .sum();
    Some(h / (counts.len() as f64).ln())
}

/// Standard normal CDF (Abramowitz–Stegun 7.1.26 erf approximation, error < 1.5e-7).
pub fn normal_cdf(z: f64) -> f64 {
    let x = z.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-x * x).exp();
    if z >= 0.0 {
        0.5 * (1.0 + erf)
    } else {
        0.5 * (1.0 - erf)
    }
}

/// Two-sided p-value of a two-proportion z-test (x1 of n1 vs x2 of n2).
pub fn two_proportion_p(x1: u32, n1: u32, x2: u32, n2: u32) -> f64 {
    if n1 == 0 || n2 == 0 {
        return 1.0;
    }
    let (p1, p2) = (f64::from(x1) / f64::from(n1), f64::from(x2) / f64::from(n2));
    if p1 == p2 {
        return 1.0;
    }
    let p = f64::from(x1 + x2) / f64::from(n1 + n2);
    let se = (p * (1.0 - p) * (1.0 / f64::from(n1) + 1.0 / f64::from(n2))).sqrt();
    if se == 0.0 {
        return 0.0;
    }
    (2.0 * (1.0 - normal_cdf(((p1 - p2) / se).abs()))).clamp(0.0, 1.0)
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// `positions`: for each valid shuffled single-choice answer, (position chosen, options shown), 0-based.
pub fn check(q: &Question, n: u32, rows: &[ReportRow], positions: &[(usize, usize)]) -> Validity {
    let mut v = Validity::default();
    let t = q.body.question_type;
    if n == 0 {
        return v;
    }
    if matches!(t, QuestionType::SingleChoice | QuestionType::Likert) {
        let counts: Vec<u32> = rows.iter().map(|r| r.count).collect();
        v.entropy = normalised_entropy(&counts).map(|e| (e * 100.0).round() / 100.0);
        if let Some(e) = v.entropy {
            if e < MIN_ENTROPY {
                v.flags.push(format!(
                    "Low variance: answers cluster on few options (entropy {e:.2}). Real respondents usually spread more."
                ));
            }
        }
    }
    if t == QuestionType::Likert && rows.len() % 2 == 1 {
        let mid = rows[rows.len() / 2].percent;
        v.midpoint_rate = Some(mid);
        if mid > MAX_MIDPOINT {
            v.flags.push(format!(
                "Most respondents chose the midpoint ({mid}%), a common sign of model hedging."
            ));
        }
    }
    if t == QuestionType::SingleChoice && q.body.randomize && !positions.is_empty() {
        let total = positions.len() as u32;
        let first = positions.iter().filter(|(p, _)| *p == 0).count() as u32;
        let last = positions.iter().filter(|(p, k)| *p + 1 == *k).count() as u32;
        let (fr, lr) = (
            f64::from(first) * 100.0 / f64::from(total),
            f64::from(last) * 100.0 / f64::from(total),
        );
        v.first_position_rate = Some(round1(fr));
        v.last_position_rate = Some(round1(lr));
        let p = two_proportion_p(first, total, last, total);
        if (fr - lr).abs() >= ORDER_GAP && p < 0.05 {
            v.flags.push(format!(
                "Order effect: {}% chose the option shown first vs {}% the option shown last (p = {p:.3}).",
                round1(fr),
                round1(lr)
            ));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_is_one_for_even_and_zero_for_collapsed_answers() {
        assert!((normalised_entropy(&[10, 10, 10, 10]).unwrap() - 1.0).abs() < 1e-12);
        assert_eq!(normalised_entropy(&[40, 0, 0]), Some(0.0));
        assert_eq!(normalised_entropy(&[5]), None);
        assert_eq!(normalised_entropy(&[0, 0]), None);
    }

    #[test]
    fn statistics_match_known_values() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((normal_cdf(1.96) - 0.975).abs() < 1e-4);
        assert!((normal_cdf(-1.645) - 0.05).abs() < 1e-3);
        // 60/100 vs 40/100: z ≈ 2.83, p ≈ 0.0047.
        let p = two_proportion_p(60, 100, 40, 100);
        assert!((p - 0.0047).abs() < 0.0005, "p = {p}");
        assert_eq!(two_proportion_p(5, 10, 5, 10), 1.0);
    }
}
