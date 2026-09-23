//! The Gemini API key lives in Windows Credential Manager, never in SQLite, logs or exports.

use survey_core::{AppError, AppResult, ErrorCode};

const SERVICE: &str = "SyntheticSurvey";
const ACCOUNT: &str = "gemini-api-key";

fn entry() -> AppResult<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT)
        .map_err(|e| AppError::new(ErrorCode::Keychain, e.to_string()))
}

pub fn set(key: &str) -> AppResult<()> {
    entry()?
        .set_password(key)
        .map_err(|e| AppError::new(ErrorCode::Keychain, e.to_string()))
}

pub fn get() -> AppResult<Option<String>> {
    match entry()?.get_password() {
        Ok(k) => Ok(Some(k)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AppError::new(ErrorCode::Keychain, e.to_string())),
    }
}

pub fn delete() -> AppResult<()> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::new(ErrorCode::Keychain, e.to_string())),
    }
}
