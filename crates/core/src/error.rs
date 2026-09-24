use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Stable error codes the UI can branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    InvalidInput,
    NotFound,
    NotImplemented,
    CohortLocked,
    SurveyNotApproved,
    Database,
    Llm,
    Keychain,
    Internal,
}

/// The error every command returns. Serialises to `{ code, message }`.
#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize, TS)]
#[error("{code:?}: {message}")]
#[ts(export)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidInput, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// For commands whose contract exists but whose body lands in a later milestone.
    pub fn not_implemented(what: &str, milestone: &str) -> Self {
        Self::new(
            ErrorCode::NotImplemented,
            format!("{what} is planned for {milestone}"),
        )
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        // Surface the trigger message so the UI can tell "survey not approved" apart.
        let text = e.to_string();
        if text.contains("survey_not_approved") {
            return Self::new(
                ErrorCode::SurveyNotApproved,
                "The survey must be approved before a run can start",
            );
        }
        Self::new(ErrorCode::Database, text)
    }
}

impl From<rusqlite_migration::Error> for AppError {
    fn from(e: rusqlite_migration::Error) -> Self {
        Self::new(ErrorCode::Database, format!("migration failed: {e}"))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::new(ErrorCode::Internal, format!("JSON error: {e}"))
    }
}
