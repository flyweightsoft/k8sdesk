use serde::Serialize;
use thiserror::Error;

/// Public-facing application error. Never embeds secret material.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("crypto error")]
    Crypto,
    #[error("keyring error: {0}")]
    Keyring(String),
    #[error("cluster not found")]
    NotFound,
    #[error("invalid input: {0}")]
    BadInput(String),
    #[error("kubeconfig import rejected: {0}")]
    KubeconfigRejected(String),
    #[error("kubernetes error: {0}")]
    Kube(String),
    #[error("confirmation required")]
    ConfirmationRequired,
    #[error("invalid or expired confirmation token")]
    BadConfirmation,
    #[error("forbidden command: {0}")]
    Forbidden(String),
    #[error("parse error: {0}")]
    Parse(String),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_string())
    }
}

impl From<keyring::Error> for AppError {
    fn from(e: keyring::Error) -> Self {
        AppError::Keyring(e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Storage(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Storage(format!("json: {}", e))
    }
}

impl From<serde_yaml::Error> for AppError {
    fn from(e: serde_yaml::Error) -> Self {
        AppError::KubeconfigRejected(format!("yaml: {}", e))
    }
}

impl From<kube::Error> for AppError {
    fn from(e: kube::Error) -> Self {
        // kube::Error's Display is generally safe (no token bytes); still strip URLs of query params.
        let s = e.to_string();
        AppError::Kube(scrub(&s))
    }
}

impl From<kube::config::KubeconfigError> for AppError {
    fn from(e: kube::config::KubeconfigError) -> Self {
        AppError::KubeconfigRejected(e.to_string())
    }
}

fn scrub(s: &str) -> String {
    // Best-effort redaction of anything that smells like a bearer token.
    let mut out = String::with_capacity(s.len());
    for tok in s.split_whitespace() {
        if tok.len() > 24 && tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') {
            out.push_str("[redacted] ");
        } else {
            out.push_str(tok);
            out.push(' ');
        }
    }
    out.trim_end().to_string()
}

pub type AppResult<T> = std::result::Result<T, AppError>;
