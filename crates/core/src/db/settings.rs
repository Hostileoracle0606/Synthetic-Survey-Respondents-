//! Settings screen persistence (BACKLOG B1): stored as one JSON blob under a fixed key in the
//! generic `settings(key, value)` table. The API key never goes here; it stays in the OS keychain.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppResult;
use crate::model::Settings;

const KEY: &str = "app";

/// The saved settings, or the defaults (auto-picked models, Free tier, no prices) if none were
/// ever saved.
pub fn get(conn: &Connection) -> AppResult<Settings> {
    let value: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [KEY], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(match value {
        Some(v) => serde_json::from_str(&v).unwrap_or_default(),
        None => Settings::default(),
    })
}

pub fn save(conn: &Connection, settings: &Settings) -> AppResult<Settings> {
    let value = serde_json::to_string(settings)?;
    conn.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![KEY, value],
    )?;
    Ok(settings.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelPrice, UsageTier};

    #[test]
    fn defaults_when_nothing_saved() {
        let conn = crate::db::open_in_memory();
        assert_eq!(get(&conn).unwrap(), Settings::default());
    }

    #[test]
    fn save_then_get_round_trips_and_upserts() {
        let conn = crate::db::open_in_memory();
        let s = Settings {
            flash_model: Some("gemini-2.5-flash".into()),
            pro_model: None,
            usage_tier: UsageTier::Tier1,
            flash_price: Some(ModelPrice {
                input_usd_per_million: 0.075,
                output_usd_per_million: 0.30,
                cached_input_usd_per_million: Some(0.0075),
            }),
            pro_price: None,
        };
        save(&conn, &s).unwrap();
        assert_eq!(get(&conn).unwrap(), s);

        let s2 = Settings {
            usage_tier: UsageTier::Tier2,
            ..s
        };
        save(&conn, &s2).unwrap();
        assert_eq!(get(&conn).unwrap(), s2);
    }
}
