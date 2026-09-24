//! Cost figures (SPEC §5 "Cost estimate", docs/DATA_FLOW.md §3 Step 4): the estimate shown
//! before Run Survey Simulation and the live and stored cost of a run. Prices come from the
//! Settings screen (BACKLOG B1); runs answer with Flash, so they use the saved Flash price.
//! Without one, every figure is None and the UI shows "$—".

use rusqlite::Connection;

use crate::db::settings;
use crate::error::AppResult;
use crate::llm::Usage;
use crate::model::ModelPrice;

/// The price a simulation run is charged at: the saved Flash price, if any.
pub fn run_price(conn: &Connection) -> AppResult<Option<ModelPrice>> {
    Ok(settings::get(conn)?.flash_price)
}

/// USD for `input` tokens of which `cached` came from the implicit cache, plus `output`.
fn usd(p: &ModelPrice, input: f64, cached: f64, output: f64) -> f64 {
    let cached = cached.min(input);
    let cached_price = p
        .cached_input_usd_per_million
        .unwrap_or(p.input_usd_per_million);
    ((input - cached) * p.input_usd_per_million
        + cached * cached_price
        + output * p.output_usd_per_million)
        / 1_000_000.0
}

/// Uncached input × input price + cached input × cached price + output × output price.
pub fn cost(p: &ModelPrice, u: &Usage) -> f64 {
    usd(
        p,
        f64::from(u.input_tokens),
        f64::from(u.cached_tokens),
        f64::from(u.output_tokens),
    )
}

/// Cost of a run's calls so far, from their token counts in `llm_calls`.
pub fn run_cost(conn: &Connection, run_id: i64, p: &ModelPrice) -> AppResult<f64> {
    let (input, cached, output): (i64, i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(cached_tokens), 0), COALESCE(SUM(output_tokens), 0)
         FROM llm_calls WHERE run_id = ?1",
        [run_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(usd(p, input as f64, cached as f64, output as f64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Settings;

    fn price(cached: Option<f64>) -> ModelPrice {
        ModelPrice {
            input_usd_per_million: 1.0,
            output_usd_per_million: 10.0,
            cached_input_usd_per_million: cached,
        }
    }

    const USAGE: Usage = Usage {
        input_tokens: 1_000_000,
        cached_tokens: 400_000,
        output_tokens: 100_000,
    };

    #[test]
    fn cached_tokens_are_charged_at_the_cached_price() {
        // 0.6 + 0.04 + 1.0
        assert!((cost(&price(Some(0.1)), &USAGE) - 1.64).abs() < 1e-9);
    }

    #[test]
    fn without_a_cached_price_cached_tokens_cost_full_input_price() {
        // 1.0 + 1.0
        assert!((cost(&price(None), &USAGE) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn runs_use_the_saved_flash_price() {
        let conn = crate::db::open_in_memory();
        assert_eq!(run_price(&conn).unwrap(), None);
        settings::save(
            &conn,
            &Settings {
                flash_price: Some(price(Some(0.1))),
                pro_price: Some(price(None)),
                ..Settings::default()
            },
        )
        .unwrap();
        assert_eq!(run_price(&conn).unwrap(), Some(price(Some(0.1))));
    }
}
