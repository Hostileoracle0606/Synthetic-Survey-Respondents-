//! The Gemini price table (the price part of BACKLOG B1) and the cost figures built on it:
//! the estimate shown before Run Survey Simulation and the live cost in Step 4
//! (docs/DATA_FLOW.md §3, Step 4). Prices are stored per model in `settings` under
//! `price_table`; a model with no stored price uses the starting price for its tier.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::llm::Usage;

const SETTING: &str = "price_table";

/// US dollars per million tokens for one Gemini model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelPrice {
    pub model: String,
    pub input_per_million: f64,
    /// Input tokens served from Gemini's implicit cache.
    pub cached_input_per_million: f64,
    /// Output tokens, thinking included (Gemini bills thinking as output).
    pub output_per_million: f64,
    /// False while the price is the built-in starting value, not one the user saved.
    pub saved: bool,
}

/// Starting prices by tier, from Google's published Gemini 3 pricing (standard tier, prompts
/// up to 200k tokens). They are editable because Google changes them; check
/// https://ai.google.dev/gemini-api/docs/pricing. None for a model outside the known tiers.
pub fn default_price(model: &str) -> Option<ModelPrice> {
    let m = model.to_ascii_lowercase();
    let (input, cached, output) = if m.contains("flash-lite") {
        (0.10, 0.01, 0.40)
    } else if m.contains("flash") {
        (0.50, 0.05, 3.00)
    } else if m.contains("pro") {
        (2.00, 0.20, 12.00)
    } else {
        return None;
    };
    Some(ModelPrice {
        model: model.to_string(),
        input_per_million: input,
        cached_input_per_million: cached,
        output_per_million: output,
        saved: false,
    })
}

/// Prices the user has saved, by model.
pub fn saved(conn: &Connection) -> AppResult<Vec<ModelPrice>> {
    let json: Option<String> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SETTING],
            |r| r.get(0),
        )
        .optional()?;
    Ok(json
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default())
}

/// The price used for `model`: the saved one, else the tier's starting price.
pub fn price_for(conn: &Connection, model: &str) -> AppResult<Option<ModelPrice>> {
    Ok(saved(conn)?
        .into_iter()
        .find(|p| p.model == model)
        .or_else(|| default_price(model)))
}

/// The table for the given models (e.g. the ones the key can use) plus any saved ones.
pub fn table(conn: &Connection, models: &[String]) -> AppResult<Vec<ModelPrice>> {
    let mut out = saved(conn)?;
    for m in models {
        if !out.iter().any(|p| &p.model == m) {
            out.extend(default_price(m));
        }
    }
    out.sort_by(|a, b| a.model.cmp(&b.model));
    Ok(out)
}

/// Saves one model's price, replacing any earlier one.
pub fn set_price(conn: &Connection, price: ModelPrice) -> AppResult<ModelPrice> {
    let model = price.model.trim().to_string();
    if model.is_empty() {
        return Err(AppError::invalid("name the model the price is for"));
    }
    let values = [
        price.input_per_million,
        price.cached_input_per_million,
        price.output_per_million,
    ];
    if values
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0 || *v > 1_000.0)
    {
        return Err(AppError::invalid(
            "prices are US dollars per million tokens, from 0 to 1,000",
        ));
    }
    let price = ModelPrice {
        model: model.clone(),
        saved: true,
        ..price
    };
    let mut all = saved(conn)?;
    all.retain(|p| p.model != model);
    all.push(price.clone());
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![SETTING, serde_json::to_string(&all).unwrap_or_default()],
    )?;
    Ok(price)
}

/// Uncached input × input price + cached input × cached price + output × output price.
pub fn cost(p: &ModelPrice, u: &Usage) -> f64 {
    let cached = u.cached_tokens.min(u.input_tokens);
    (f64::from(u.input_tokens - cached) * p.input_per_million
        + f64::from(cached) * p.cached_input_per_million
        + f64::from(u.output_tokens) * p.output_per_million)
        / 1_000_000.0
}

/// Cost of a run's answer calls so far, from `llm_calls`.
pub fn run_cost(conn: &Connection, run_id: i64, p: &ModelPrice) -> AppResult<f64> {
    let u: (i64, i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_tokens), 0), COALESCE(SUM(output_tokens), 0)
         FROM llm_calls WHERE run_id = ?1",
        [run_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let cached = u.1.min(u.0);
    Ok(((u.0 - cached) as f64 * p.input_per_million
        + cached as f64 * p.cached_input_per_million
        + u.2 as f64 * p.output_per_million)
        / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gemini_tier_has_a_starting_price() {
        let flash = default_price("gemini-3.8-flash").unwrap();
        assert!(!flash.saved);
        assert!(flash.cached_input_per_million < flash.input_per_million);
        assert!(
            default_price("gemini-3.1-flash-lite")
                .unwrap()
                .input_per_million
                < flash.input_per_million
        );
        assert!(
            default_price("gemini-3.1-pro-preview")
                .unwrap()
                .output_per_million
                > flash.output_per_million
        );
        assert!(default_price("gemma-3").is_none());
    }

    #[test]
    fn saved_prices_win_over_starting_prices() {
        let conn = crate::db::open_in_memory();
        assert_eq!(
            price_for(&conn, "gemini-x-flash")
                .unwrap()
                .unwrap()
                .input_per_million,
            0.50
        );
        let mut p = default_price("gemini-x-flash").unwrap();
        p.input_per_million = 0.25;
        assert!(set_price(&conn, p.clone()).unwrap().saved);
        p.output_per_million = 2.0;
        set_price(&conn, p).unwrap();
        let got = price_for(&conn, "gemini-x-flash").unwrap().unwrap();
        assert_eq!((got.input_per_million, got.output_per_million), (0.25, 2.0));
        assert_eq!(saved(&conn).unwrap().len(), 1);
        let t = table(&conn, &["gemini-x-pro".into(), "gemini-x-flash".into()]).unwrap();
        assert_eq!(t.len(), 2);
        assert!(t.iter().any(|p| p.model == "gemini-x-pro" && !p.saved));

        let mut bad = default_price("gemini-x-flash").unwrap();
        bad.output_per_million = -1.0;
        assert!(set_price(&conn, bad).is_err());
    }

    #[test]
    fn cached_tokens_are_charged_at_the_cached_price() {
        let p = ModelPrice {
            model: "m".into(),
            input_per_million: 1.0,
            cached_input_per_million: 0.1,
            output_per_million: 10.0,
            saved: true,
        };
        let u = Usage {
            input_tokens: 1_000_000,
            cached_tokens: 400_000,
            output_tokens: 100_000,
        };
        // 0.6 + 0.04 + 1.0
        assert!((cost(&p, &u) - 1.64).abs() < 1e-9);
    }
}
