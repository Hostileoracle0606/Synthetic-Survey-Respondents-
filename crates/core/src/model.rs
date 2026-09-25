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
    /// Percentage (0-100) of respondents drawn as non-binary, applied on top of the census
    /// or synthetic gender draw (BACKLOG B7). 0 keeps the binary census/quota draw as is.
    /// Mutually exclusive with a "gender" quota group.
    #[serde(default)]
    pub non_binary_share: u8,
    /// Target Countries at the moment this cohort was generated, so Step 2 can tell the user
    /// their Step 1 audience has drifted (BACKLOG B4). Not user-editable directly.
    #[serde(default)]
    pub countries: Vec<String>,
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

// ---- Step 3: questionnaire ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum QuestionType {
    SingleChoice,
    MultiChoice,
    Likert,
    Numeric,
    OpenEnded,
}

impl QuestionType {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::SingleChoice => "single_choice",
            Self::MultiChoice => "multi_choice",
            Self::Likert => "likert",
            Self::Numeric => "numeric",
            Self::OpenEnded => "open_ended",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "single_choice" => Self::SingleChoice,
            "multi_choice" => Self::MultiChoice,
            "likert" => Self::Likert,
            "numeric" => Self::Numeric,
            "open_ended" => Self::OpenEnded,
            _ => return None,
        })
    }

    pub fn is_choice(self) -> bool {
        matches!(self, Self::SingleChoice | Self::MultiChoice)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChoiceOption {
    /// Stable within the question ("A", "B", …); answers store this code.
    pub code: String,
    pub label: String,
}

/// Likert scale, e.g. 1–7 with end labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Scale {
    pub min: i32,
    pub max: i32,
    pub min_label: String,
    pub max_label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NumericRange {
    pub min: f64,
    pub max: f64,
    pub unit: String,
}

/// The part of a question a reviewer edits. Which fields apply depends on the type:
/// options (choice types), scale (likert), numeric (numeric).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QuestionBody {
    pub text: String,
    pub question_type: QuestionType,
    pub options: Vec<ChoiceOption>,
    /// Shuffle option order per respondent (choice types).
    pub randomize: bool,
    /// Most options a respondent may pick (multi choice).
    pub max_choices: Option<u32>,
    pub scale: Option<Scale>,
    pub numeric: Option<NumericRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum QuestionOrigin {
    Ai,
    AiEdited,
    Human,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReviewStatus {
    /// In the AI suggestions sidebar; not part of the survey until added.
    Suggested,
    Pending,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Question {
    #[ts(type = "number")]
    pub id: i64,
    pub code: String,
    pub order_index: u32,
    pub body: QuestionBody,
    pub is_active: bool,
    pub origin: QuestionOrigin,
    pub review_status: ReviewStatus,
    /// What the question is for (from the draft); shown to the reviewer, never to respondents.
    pub objective: Option<String>,
    pub rationale: Option<String>,
    /// The critic's check of the current wording; None until one has been asked for (a
    /// person's new question is checked when first saved). Advice only: never blocks approval.
    pub critique: Option<Critique>,
}

/// Wording problems the critic looks for (docs/DATA_FLOW.md §3, Step 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CriticIssue {
    Leading,
    DoubleBarrelled,
    Unclear,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CriticFlag {
    pub issue: CriticIssue,
    /// What is wrong and how to fix it, in a sentence or two.
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CriticStatus {
    Checking,
    Done,
    Failed,
}

/// Stored with the question (`questions.critic_json`) and cleared when its wording changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Critique {
    pub status: CriticStatus,
    pub flags: Vec<CriticFlag>,
    /// Why the check failed, when it did.
    pub error: Option<String>,
    /// Prompt version that produced the flags, e.g. "critic.v1".
    pub prompt_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SurveyStatus {
    Draft,
    InReview,
    Approved,
}

/// State of the Gemini draft for a survey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DraftStatus {
    /// Written by hand; no draft requested.
    None,
    Generating,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Survey {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub project_id: i64,
    pub title: String,
    pub intro: String,
    pub status: SurveyStatus,
    pub draft_status: DraftStatus,
    pub draft_error: Option<String>,
    /// Active questions in survey order.
    pub questions: Vec<Question>,
    /// AI suggestions not yet added.
    pub suggestions: Vec<Question>,
}

// ---- Step 4: simulation ----

/// Options for Run Survey Simulation. Everything else is fixed per run and snapshotted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunConfig {
    /// Seeds option shuffling; defaults to the cohort's seed.
    #[ts(type = "number | null")]
    pub seed: Option<u64>,
    /// Distribution mode (SPEC §8, BACKLOG B23): record each option's probability for
    /// single-choice questions, from an extra logprobs-enabled call per question and
    /// respondent. Off by default; the UI hides the toggle when the model doesn't support it
    /// (`probe_logprobs`).
    #[serde(default)]
    pub logprobs: bool,
}

/// Shown before Run Survey Simulation (SPEC §5 "Cost estimate").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CostEstimate {
    /// The answering model the run would use.
    pub model: String,
    /// One whole-survey call per respondent.
    pub calls: u32,
    #[ts(type = "number")]
    pub input_tokens: u64,
    #[ts(type = "number")]
    pub output_tokens: u64,
    /// None until a Flash price is saved in Settings.
    pub cost_usd: Option<f64>,
    /// True when output tokens come from earlier runs on this model rather than a rule of thumb.
    pub output_from_history: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SimulationRun {
    #[ts(type = "number")]
    pub id: i64,
    #[ts(type = "number")]
    pub project_id: i64,
    #[ts(type = "number")]
    pub survey_id: i64,
    #[ts(type = "number")]
    pub cohort_id: i64,
    pub status: RunStatus,
    pub model: String,
    /// Answering prompt version, shown with the results (SPEC §8 disclosure).
    pub prompt_version: String,
    pub respondents: u32,
    pub questions: u32,
    /// Stored answers (any status).
    pub answered: u32,
    pub respondents_done: u32,
    /// Why the run paused or failed, e.g. an invalid key.
    pub error: Option<String>,
    pub created_at: String,
    /// Estimated when the run started; None without a saved Flash price.
    pub est_cost_usd: Option<f64>,
    /// Actual cost of the run's calls so far, from their token counts.
    pub cost_usd: Option<f64>,
}

impl RunStatus {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Cancelled => "cancelled",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "stopped" => Self::Stopped,
            "cancelled" => Self::Cancelled,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Queued,
        }
    }
}

// ---- Step 5: report ----

/// Chart chosen from the question type (docs/DATA_FLOW.md §3, Step 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ChartKind {
    /// Single choice with up to 6 options.
    Pie,
    Bar,
    /// Multiple choice: % of respondents choosing each option (can total over 100%).
    MultiBar,
    /// Likert: distribution across the scale.
    Diverging,
    Histogram,
    Themes,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReportRow {
    /// Option code, scale point or histogram bin start.
    pub key: String,
    pub label: String,
    pub count: u32,
    pub percent: f64,
    /// Distribution mode (SPEC §8, BACKLOG B23): this option's average recorded probability,
    /// as a percentage (0–100, same scale as `percent`) across respondents the run probed,
    /// charted next to the sampled count. `None` on every row when the run didn't use
    /// distribution mode, or for chart types it doesn't cover (only single-choice answers are
    /// probed).
    pub avg_prob: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ThemeSummary {
    #[ts(type = "number")]
    pub id: i64,
    pub label: String,
    pub description: String,
    pub count: u32,
    pub percent: f64,
    /// Up to three example answers.
    pub quotes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QuestionReport {
    #[ts(type = "number")]
    pub question_id: i64,
    pub code: String,
    pub text: String,
    pub question_type: QuestionType,
    pub chart: ChartKind,
    /// Valid answers, the base for every percentage.
    pub n: u32,
    pub invalid: u32,
    pub refused: u32,
    pub rows: Vec<ReportRow>,
    pub mean: Option<f64>,
    pub median: Option<f64>,
    pub q1: Option<f64>,
    pub q3: Option<f64>,
    pub unit: Option<String>,
    pub themes: Vec<ThemeSummary>,
    /// Open answers shown while themes are not coded yet (up to 5).
    pub sample_answers: Vec<String>,
    pub validity: Validity,
}

/// Label-free checks on a question's answers (docs/SPEC.md §8, TEST_PLAN S13). They need no
/// expected answers, so they run on every question.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Validity {
    /// Normalised entropy 0–1 (single choice and scales); low means answers collapsed onto one option.
    pub entropy: Option<f64>,
    /// Share (%) choosing the midpoint of an odd scale.
    pub midpoint_rate: Option<f64>,
    /// Shuffled single choice: % choosing whatever was shown first, and last.
    pub first_position_rate: Option<f64>,
    pub last_position_rate: Option<f64>,
    /// Plain-language warnings, e.g. "Most respondents chose the midpoint (72%)".
    pub flags: Vec<String>,
}

/// A respondent attribute to cross-tabulate by.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Dimension {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SynthesisStatus {
    None,
    Generating,
    Ready,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FrictionPoint {
    pub label: String,
    /// Checked against the theme coding before display.
    pub mentions: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SegmentTakeaway {
    /// Dimension key, e.g. "age".
    pub dimension: String,
    /// A real cross-tab group, e.g. "18–29".
    pub group: String,
    pub takeaway: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Synthesis {
    pub summary: String,
    pub friction_points: Vec<FrictionPoint>,
    pub segments: Vec<SegmentTakeaway>,
    pub based_on_n: u32,
    pub model: String,
    /// Claims removed because they failed the checks.
    pub dropped: u32,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Report {
    pub run: SimulationRun,
    /// Respondents with saved answers (fewer than the cohort for a stopped run).
    pub based_on_n: u32,
    pub questions: Vec<QuestionReport>,
    pub dimensions: Vec<Dimension>,
    pub synthesis: Option<Synthesis>,
    pub synthesis_status: SynthesisStatus,
    pub synthesis_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CrossTabGroup {
    pub label: String,
    pub n: u32,
    /// Under 30 respondents: read with care.
    pub low_base: bool,
    /// % within the group, one per column.
    pub cells: Vec<f64>,
    /// Likert and numeric questions.
    pub mean: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CrossTab {
    #[ts(type = "number")]
    pub question_id: i64,
    pub dimension: Dimension,
    /// Answer columns: options, scale points or themes.
    pub columns: Vec<ReportRow>,
    pub groups: Vec<CrossTabGroup>,
}

/// Save-dialog export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExportFormat {
    Csv,
    Json,
}

/// Gemini's per-project rate-limit tier (BACKLOG B1); sets the [`crate::engine::limiter::Limits`]
/// the rate limiter uses. Picked by the user in Settings since Gemini has no API to read it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UsageTier {
    #[default]
    Free,
    Tier1,
    Tier2,
    Tier3,
}

impl UsageTier {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Tier1 => "tier1",
            Self::Tier2 => "tier2",
            Self::Tier3 => "tier3",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "free" => Some(Self::Free),
            "tier1" => Some(Self::Tier1),
            "tier2" => Some(Self::Tier2),
            "tier3" => Some(Self::Tier3),
            _ => None,
        }
    }
}

/// USD per 1,000,000 tokens, as published on the Gemini pricing page. The user copies these in;
/// the app has no way to read them back from a key.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModelPrice {
    pub input_usd_per_million: f64,
    pub output_usd_per_million: f64,
    /// Input tokens served from Gemini's implicit cache, usually a tenth of the input price.
    /// None charges them at the full input price, so the cost is never understated.
    #[serde(default)]
    pub cached_input_usd_per_million: Option<f64>,
}

/// Settings screen (BACKLOG B1): everything but the API key itself, which stays in the OS
/// keychain. `null` model IDs mean "pick automatically" (newest stable Flash / Pro).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Settings {
    pub flash_model: Option<String>,
    pub pro_model: Option<String>,
    pub usage_tier: UsageTier,
    pub flash_price: Option<ModelPrice>,
    pub pro_price: Option<ModelPrice>,
}
