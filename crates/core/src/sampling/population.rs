//! Population tables: weighted demographic cells per country, built offline from census
//! microdata by `tools/build-populations` (see `data/populations/*.meta.json` for sources).

use std::collections::HashMap;
use std::sync::LazyLock;

/// One weighted combination of demographics.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub country: String,
    pub age_band: String,
    pub gender: String,
    /// Empty when the country has no regional breakdown.
    pub region: String,
    pub income: String,
    pub occupation: String,
    pub weight: f64,
}

impl Cell {
    /// Value of a quota dimension for this cell. Quota group keys come from the UI.
    pub fn value(&self, key: &str) -> Option<&str> {
        match key {
            "country" => Some(&self.country),
            "age" => Some(&self.age_band),
            "gender" => Some(&self.gender),
            "region" => Some(&self.region),
            "income" => Some(&self.income),
            "occupation" => Some(&self.occupation),
            _ => None,
        }
    }
}

pub struct PopulationTable {
    pub country: &'static str,
    pub cells: Vec<Cell>,
    /// Weighted single-year ages (18+), used to draw an age inside a band.
    pub ages: Vec<(u32, f64)>,
}

pub const AGE_BANDS: [(&str, u32, u32); 4] = [
    ("18–29", 18, 29),
    ("30–44", 30, 44),
    ("45–59", 45, 59),
    ("60+", 60, 84),
];
pub const GENDERS: [&str; 2] = ["Female", "Male"];
/// Label used for the user-set non-binary share applied on top of the census draw (BACKLOG B7).
pub const NON_BINARY_LABEL: &str = "Non-binary";
pub const INCOMES: [&str; 3] = ["Under $50k", "$50k–$100k", "Over $100k"];

fn parse_cells(country: &str, csv: &str) -> Vec<Cell> {
    csv.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            assert_eq!(f.len(), 6, "bad population row: {l}");
            Cell {
                country: country.to_string(),
                age_band: f[0].into(),
                gender: f[1].into(),
                region: f[2].into(),
                income: f[3].into(),
                occupation: f[4].into(),
                weight: f[5].parse().expect("numeric weight"),
            }
        })
        .collect()
}

fn parse_ages(csv: &str) -> Vec<(u32, f64)> {
    csv.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (a, w) = l.split_once(',').expect("age,weight");
            (a.parse().expect("age"), w.parse().expect("weight"))
        })
        .collect()
}

static TABLES: LazyLock<HashMap<&'static str, PopulationTable>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    m.insert(
        "CA",
        PopulationTable {
            country: "CA",
            cells: parse_cells("CA", include_str!("../../data/populations/CA.csv")),
            ages: parse_ages(include_str!("../../data/populations/CA_ages.csv")),
        },
    );
    m
});

pub fn table(country: &str) -> Option<&'static PopulationTable> {
    TABLES.get(country)
}

/// Occupation mix used for countries without a census table: the Canadian adult mix, as a
/// neutral placeholder until those tables exist (BACKLOG B5).
pub fn fallback_occupations() -> Vec<(String, f64)> {
    let ca = table("CA").expect("CA table is bundled");
    let mut shares: Vec<(String, f64)> = Vec::new();
    for c in &ca.cells {
        match shares.iter_mut().find(|(o, _)| *o == c.occupation) {
            Some((_, w)) => *w += c.weight,
            None => shares.push((c.occupation.clone(), c.weight)),
        }
    }
    shares
}

/// Share of the table's population per value of a quota dimension, in table order.
pub fn marginal(t: &PopulationTable, key: &str) -> Vec<(String, f64)> {
    let total: f64 = t.cells.iter().map(|c| c.weight).sum();
    let mut out: Vec<(String, f64)> = Vec::new();
    for c in &t.cells {
        let v = c.value(key).unwrap_or_default().to_string();
        match out.iter_mut().find(|(k, _)| *k == v) {
            Some((_, w)) => *w += c.weight / total,
            None => out.push((v, c.weight / total)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ca_table_loads_and_covers_every_dimension() {
        let t = table("CA").unwrap();
        assert!(t.cells.len() > 500);
        let total: f64 = t.cells.iter().map(|c| c.weight).sum();
        assert!(
            total > 2.5e7,
            "weighted adult population looks wrong: {total}"
        );
        for (band, _, _) in AGE_BANDS {
            assert!(t.cells.iter().any(|c| c.age_band == band), "missing {band}");
        }
        for inc in INCOMES {
            assert!(t.cells.iter().any(|c| c.income == inc), "missing {inc}");
        }
        assert_eq!(marginal(t, "region").len(), 6);
        assert!(t.ages.iter().all(|(a, w)| *a >= 18 && *w > 0.0));
    }
}
