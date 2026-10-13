//! Unified error model. Every provider maps its own error format onto
//! [`CueError`] so the UI can show one consistent, understandable message.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Credentials missing, invalid, expired or revoked.
    Auth,
    /// Credentials are valid but not allowed to do this (region, org policy, plan).
    PermissionDenied,
    /// The requested model does not exist or is not available to this account.
    ModelNotFound,
    /// Too many requests in a short time window. Usually resolves by waiting.
    RateLimited,
    /// Account has no remaining credit / quota / billing problem.
    QuotaExceeded,
    /// A subscription plan's usage window is exhausted (e.g. ChatGPT plan limits).
    SubscriptionLimit,
    /// The provider is temporarily overloaded or unavailable.
    Overloaded,
    /// The request was rejected as malformed or unsupported.
    InvalidRequest,
    /// Input too large for the model's context window.
    ContextTooLong,
    /// The model declined to answer (safety refusal).
    Refusal,
    /// Could not reach the provider (DNS, TLS, offline, proxy).
    Network,
    Timeout,
    Cancelled,
    /// Connection is not set up (missing key, not signed in, CLI not installed).
    NotConfigured,
    /// Credential storage (OS keychain) failed.
    SecretStore,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct CueError {
    pub kind: ErrorKind,
    /// Short, user-facing explanation.
    pub message: String,
    /// What the user can do about it, if anything.
    pub hint: Option<String>,
    /// Raw provider detail (never contains credentials).
    pub detail: Option<String>,
    pub status: Option<u16>,
    pub retryable: bool,
    /// Provider request id, useful for support tickets.
    pub request_id: Option<String>,
}

impl CueError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
            detail: None,
            status: None,
            retryable: matches!(
                kind,
                ErrorKind::RateLimited | ErrorKind::Overloaded | ErrorKind::Network | ErrorKind::Timeout
            ),
            request_id: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(redact(&detail.into()));
        self
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }

    pub fn with_request_id(mut self, id: Option<String>) -> Self {
        self.request_id = id;
        self
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorKind::Cancelled, "Request cancelled.")
    }

    pub fn not_configured(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotConfigured, message)
    }

    /// Default user-facing text for an error kind, used when a provider gives us
    /// nothing better.
    pub fn from_kind(kind: ErrorKind, provider: &str) -> Self {
        let (msg, hint) = match kind {
            ErrorKind::Auth => (
                format!("{provider} rejected the credentials."),
                "Check or replace the API key / sign in again under AI Providers.",
            ),
            ErrorKind::PermissionDenied => (
                format!("{provider} denied access to this resource."),
                "Your account, organization or region may not have access. Check the provider console.",
            ),
            ErrorKind::ModelNotFound => (
                format!("The selected model is not available on {provider}."),
                "Reload the model list under AI Providers and pick an available model.",
            ),
            ErrorKind::RateLimited => (
                format!("{provider} rate limit reached."),
                "Wait a moment and try again.",
            ),
            ErrorKind::QuotaExceeded => (
                format!("{provider} reports no remaining credit or quota."),
                "Check billing / credits in the provider's console. API usage is billed separately from chat subscriptions.",
            ),
            ErrorKind::SubscriptionLimit => (
                format!("Your {provider} plan usage limit is reached."),
                "Wait for your plan window to reset or switch to another connection.",
            ),
            ErrorKind::Overloaded => (
                format!("{provider} is temporarily overloaded."),
                "Try again shortly or switch to another model.",
            ),
            ErrorKind::InvalidRequest => (
                format!("{provider} rejected the request."),
                "See details below.",
            ),
            ErrorKind::ContextTooLong => (
                "The conversation is too long for this model.".to_string(),
                "Start a new conversation or choose a model with a larger context window.",
            ),
            ErrorKind::Refusal => (
                "The model declined to answer this request.".to_string(),
                "Rephrase the request or try another model.",
            ),
            ErrorKind::Network => (
                format!("Could not reach {provider}."),
                "Check your internet connection, VPN or proxy settings.",
            ),
            ErrorKind::Timeout => (format!("{provider} did not respond in time."), "Try again."),
            ErrorKind::Cancelled => ("Request cancelled.".to_string(), ""),
            ErrorKind::NotConfigured => (
                format!("{provider} is not configured."),
                "Open AI Providers to set up this connection.",
            ),
            ErrorKind::SecretStore => (
                "Could not access the system credential store.".to_string(),
                "Make sure the OS keychain / credential manager is available and unlocked.",
            ),
            ErrorKind::Unknown => (format!("Unexpected error from {provider}."), "See details below."),
        };
        let e = Self::new(kind, msg);
        if hint.is_empty() { e } else { e.with_hint(hint) }
    }

    /// Map a transport error from reqwest.
    pub fn from_reqwest(err: &reqwest::Error, provider: &str) -> Self {
        let kind = if err.is_timeout() {
            ErrorKind::Timeout
        } else {
            ErrorKind::Network
        };
        Self::from_kind(kind, provider).with_detail(err.to_string())
    }
}

/// Remove anything that looks like a credential from free text before it is
/// shown or logged.
pub fn redact(text: &str) -> String {
    let is_tok = |c: char| c.is_ascii_alphanumeric() || "-_.".contains(c);
    let mut out = String::with_capacity(text.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if looks_like_secret(run) {
            out.push_str("[redacted]");
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in text.chars() {
        if is_tok(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

fn looks_like_secret(w: &str) -> bool {
    const PREFIXES: [&str; 6] = ["sk-", "sk_", "AIza", "eyJ", "rt_", "sess-"];
    (PREFIXES.iter().any(|p| w.starts_with(p)) && w.len() >= 20)
        || (w.len() >= 40 && w.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && w.chars().any(|c| c.is_ascii_digit()))
}

pub type CueResult<T> = Result<T, CueError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_keys() {
        let s = redact("invalid key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123 provided");
        assert_eq!(s, "invalid key [redacted] provided");
        let s = redact("key=AIzaSyA1234567890abcdefghijklmnopq");
        assert_eq!(s, "key=[redacted]");
        assert_eq!(redact("model not found"), "model not found");
    }

    #[test]
    fn retryable_defaults() {
        assert!(CueError::new(ErrorKind::RateLimited, "x").retryable);
        assert!(!CueError::new(ErrorKind::Auth, "x").retryable);
    }
}
