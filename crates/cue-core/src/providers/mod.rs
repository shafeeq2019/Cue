//! Modular provider architecture.
//!
//! Every backend implements [`Provider`]. The app only ever talks to this
//! trait, so adding a provider means adding one module and one match arm in
//! [`build_provider`].

pub mod anthropic;
pub mod claude_code;
pub mod gemini;
pub mod openai;
pub mod openai_compatible;

use async_trait::async_trait;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::error::{CueError, CueResult, ErrorKind};
use crate::types::{AuthMethod, ChatOutcome, ChatRequest, ModelInfo, ProviderKind, StreamEvent};

pub type EventTx = UnboundedSender<StreamEvent>;

#[async_trait]
pub trait Provider: Send + Sync {
    /// Human-readable provider name used in messages.
    fn name(&self) -> &str;

    /// Models this connection can actually use (fetched live from the provider).
    async fn list_models(&self) -> CueResult<Vec<ModelInfo>>;

    /// Stream a chat completion. Text deltas are sent on `tx`; the final
    /// outcome (stop reason + usage) is returned.
    async fn chat(
        &self,
        req: &ChatRequest,
        tx: &EventTx,
        cancel: &CancellationToken,
    ) -> CueResult<ChatOutcome>;
}

/// Secret material resolved from the OS credential store just before use.
/// Never serialized, never sent to the frontend.
#[derive(Clone)]
pub enum Credential {
    ApiKey(String),
    /// OAuth bearer access token (ChatGPT plan usage).
    Bearer(String),
    None,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credential::ApiKey(_) => f.write_str("ApiKey(***)"),
            Credential::Bearer(_) => f.write_str("Bearer(***)"),
            Credential::None => f.write_str("None"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub auth: AuthMethod,
    /// Override for the API base URL (required for OpenAI-compatible servers).
    pub base_url: Option<String>,
    pub credential: Credential,
    /// Path to the `claude` executable for the Claude Code connection.
    pub cli_path: Option<String>,
}

pub fn build_provider(cfg: ProviderConfig, http: reqwest::Client) -> CueResult<Box<dyn Provider>> {
    let missing_key = |name: &str| {
        CueError::from_kind(ErrorKind::NotConfigured, name)
            .with_hint("Add an API key for this connection under AI Providers.")
    };
    Ok(match cfg.kind {
        ProviderKind::Anthropic => match cfg.credential {
            Credential::ApiKey(key) => {
                Box::new(anthropic::AnthropicProvider::new(http, key, cfg.base_url))
            }
            _ => return Err(missing_key("Anthropic")),
        },
        ProviderKind::Openai => match (cfg.auth, cfg.credential) {
            (AuthMethod::ApiKey, Credential::ApiKey(key)) => Box::new(
                openai::OpenAiProvider::with_api_key(http, key, cfg.base_url),
            ),
            (AuthMethod::ChatgptOauth, Credential::Bearer(token)) => {
                Box::new(openai::OpenAiProvider::with_chatgpt_plan(http, token))
            }
            (AuthMethod::ChatgptOauth, _) => {
                return Err(CueError::from_kind(ErrorKind::Auth, "ChatGPT")
                    .with_hint("Sign in with ChatGPT again under AI Providers."));
            }
            _ => return Err(missing_key("OpenAI")),
        },
        ProviderKind::Gemini => match cfg.credential {
            Credential::ApiKey(key) => {
                Box::new(gemini::GeminiProvider::new(http, key, cfg.base_url))
            }
            _ => return Err(missing_key("Google Gemini")),
        },
        ProviderKind::OpenaiCompatible => {
            let base = cfg
                .base_url
                .filter(|b| !b.trim().is_empty())
                .ok_or_else(|| {
                    CueError::not_configured("No server URL configured for this connection.")
                        .with_hint("Set the base URL, e.g. http://localhost:11434/v1 for Ollama.")
                })?;
            let key = match cfg.credential {
                Credential::ApiKey(k) if !k.is_empty() => Some(k),
                _ => None,
            };
            Box::new(openai_compatible::OpenAiCompatibleProvider::new(
                http, base, key,
            ))
        }
        ProviderKind::ClaudeCode => Box::new(claude_code::ClaudeCodeProvider::new(cfg.cli_path)),
    })
}

/// Shared HTTP client: platform TLS verification (works behind corporate
/// proxies whose CA is in the OS store), system proxy support.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("Cue/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("HTTP client configuration is valid")
}

pub(crate) fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Read an error body (bounded) for mapping. Never fails.
pub(crate) async fn error_body(resp: reqwest::Response) -> (u16, Option<String>, String) {
    let status = resp.status().as_u16();
    let req_id = ["request-id", "x-request-id", "x-goog-request-id"]
        .iter()
        .find_map(|h| {
            resp.headers()
                .get(*h)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        });
    let body = resp.text().await.unwrap_or_default();
    let body: String = body.chars().take(4000).collect();
    (status, req_id, body)
}

/// Generic HTTP status → error-kind mapping used when a provider-specific
/// error type is not recognised.
pub(crate) fn kind_for_status(status: u16) -> ErrorKind {
    match status {
        401 => ErrorKind::Auth,
        402 => ErrorKind::QuotaExceeded,
        403 => ErrorKind::PermissionDenied,
        404 => ErrorKind::ModelNotFound,
        408 => ErrorKind::Timeout,
        413 => ErrorKind::ContextTooLong,
        429 => ErrorKind::RateLimited,
        400 | 409 | 422 => ErrorKind::InvalidRequest,
        500..=599 => ErrorKind::Overloaded,
        _ => ErrorKind::Unknown,
    }
}

/// Heuristic detection of context-length errors in free-form provider messages.
pub(crate) fn mentions_context_length(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("context length")
        || m.contains("context window")
        || m.contains("too many tokens")
        || m.contains("prompt is too long")
        || m.contains("maximum context")
}

pub(crate) fn send_delta(tx: &EventTx, text: &str) {
    if !text.is_empty() {
        let _ = tx.send(StreamEvent::Delta {
            text: text.to_string(),
        });
    }
}

/// Run an HTTP request future while honoring cancellation.
pub(crate) async fn send_cancellable(
    req: reqwest::RequestBuilder,
    provider: &str,
    cancel: &CancellationToken,
) -> CueResult<reqwest::Response> {
    tokio::select! {
        _ = cancel.cancelled() => Err(CueError::cancelled()),
        r = req.send() => r.map_err(|e| CueError::from_reqwest(&e, provider)),
    }
}

#[cfg(test)]
mod tests;
