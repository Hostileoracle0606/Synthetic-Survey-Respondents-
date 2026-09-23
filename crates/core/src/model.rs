//! Types shared with the UI. `cargo test` regenerates their TypeScript versions in
//! `src/types/gen/`; CI fails if the committed files are out of date.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Step 1 "Research Type".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ResearchType {
    Brand,
    MarketResponse,
    Concept,
}

impl ResearchType {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Brand => "brand",
            Self::MarketResponse => "market_response",
            Self::Concept => "concept",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "brand" => Some(Self::Brand),
            "market_response" => Some(Self::MarketResponse),
            "concept" => Some(Self::Concept),
            _ => None,
        }
    }
}

/// Step 1 fields other than the audience. Autosaved as the user types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SurveyInfo {
    pub title: String,
    pub research_type: Option<ResearchType>,
    pub product_category: Option<String>,
    /// ISO 3166-1 alpha-2 codes, e.g. `["US"]`.
    pub countries: Vec<String>,
    pub research_goal: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Project {
    #[ts(type = "number")]
    pub id: i64,
    pub title: String,
    pub research_type: Option<ResearchType>,
    pub product_category: Option<String>,
    pub countries: Vec<String>,
    pub research_goal: String,
    /// Furthest wizard step reached, 1–5.
    pub wizard_step: u8,
    pub created_at: String,
    pub updated_at: String,
}

/// One entry in the Target Country dropdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CountryOption {
    pub code: String,
    pub name: String,
    /// False means the sampler uses quota percentages only (no real joint distribution).
    pub has_census_table: bool,
    pub regions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QuotaRow {
    pub label: String,
    pub percent: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QuotaGroup {
    /// e.g. "age", "region", "income", "country".
    pub key: String,
    pub label: String,
    pub rows: Vec<QuotaRow>,
}

/// Step 1 audience section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CohortConfig {
    pub size: u32,
    #[ts(type = "number")]
    pub seed: u64,
    pub quotas: Vec<QuotaGroup>,
    pub screening: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CohortProgress {
    pub done: u32,
    pub total: u32,
    pub replaced: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AnswerDelta {
    #[ts(type = "number")]
    pub question_id: i64,
    #[ts(type = "number")]
    pub respondent_id: i64,
    pub code: Option<String>,
    pub codes: Option<Vec<String>>,
    pub value: Option<f64>,
}

/// One line of the Step 4 live console.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConsoleLine {
    pub at: String,
    pub respondent: u32,
    pub question: String,
    pub answer: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RunStatus {
    Queued,
    Running,
    Paused,
    Stopped,
    Cancelled,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EventLevel {
    Info,
    Warn,
    Error,
}

/// Streamed to Step 4 at most every 250 ms (see docs/DATA_FLOW.md §3, Step 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(export)]
pub enum RunProgress {
    Batch {
        answered: u32,
        total_answers: u32,
        respondents_done: u32,
        /// None until the user has filled in the Gemini price table.
        cost_usd: Option<f64>,
        avg_latency_ms: u32,
        p95_latency_ms: u32,
        answers_per_min: u32,
        concurrency: u32,
        deltas: Vec<AnswerDelta>,
        console: Vec<ConsoleLine>,
    },
    Event {
        level: EventLevel,
        text: String,
    },
    Status {
        status: RunStatus,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CohortStatus {
    Draft,
    Generating,
    Ready,
    Locked,
    Failed,
}

impl CohortStatus {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Generating => "generating",
            Self::Ready => "ready",
            Self::Locked => "locked",
            Self::Failed => "failed",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "generating" => Self::Generating,
            "ready" => Self::Ready,
            "locked" => Self::Locked,
            "failed" => Self::Failed,
            _ => Self::Draft,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Cohort {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub project_id: i64,
    pub name: String,
    pub status: CohortStatus,
    pub config: CohortConfig,
    /// Set when the job failed, e.g. an invalid key.
    pub error: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Share {
    pub label: String,
    pub percent: f64,
}

/// The five Step 2 cards.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CohortSummary {
    pub status: CohortStatus,
    pub respondents: u32,
    pub average_age: Option<f64>,
    pub gender: Vec<Share>,
    /// Most common value of the product category's main field, e.g. the upgrade trigger.
    pub top_trigger: Option<Share>,
    pub top_trigger_label: String,
    pub replaced_at_screening: u32,
}

/// One card in the Step 2 grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RespondentCard {
    #[ts(type = "number")]
    pub id: i64,
    pub ordinal: u32,
    pub name: String,
    pub age: u32,
    pub gender: String,
    pub occupation: String,
    pub biases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RespondentPage {
    pub items: Vec<RespondentCard>,
    pub total: u32,
}

/// Everything the Step 2 drawer shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RespondentDetail {
    #[ts(type = "number")]
    pub id: i64,
    pub ordinal: u32,
    pub name: String,
    pub age: u32,
    pub gender: String,
    pub country: String,
    pub region: String,
    pub income: String,
    pub occupation: String,
    pub summary: String,
    pub values: Vec<String>,
    pub habits: String,
    pub media_habits: String,
    pub brand_loyalties: String,
    pub category_attitudes: String,
    pub price_sensitivity: u8,
    pub biases: Vec<String>,
    /// Category facts as label/value pairs, ready to display.
    pub category_facts: Vec<(String, String)>,
    pub screen_status: String,
    pub screen_reason: String,
}
