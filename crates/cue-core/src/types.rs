//! Provider-neutral data types shared by all providers, the app shell and the UI.

use serde::{Deserialize, Serialize};

/// Which backend a connection talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Anthropic Claude API (platform.claude.com API key).
    Anthropic,
    /// OpenAI API (API key) or ChatGPT plan via official "Sign in with ChatGPT".
    Openai,
    /// Google Gemini API (Google AI Studio API key).
    Gemini,
    /// Any server that implements the OpenAI Chat Completions API
    /// (Ollama, LM Studio, OpenRouter, Groq, Mistral, ...).
    OpenaiCompatible,
    /// The user's locally installed, unmodified Claude Code CLI.
    ClaudeCode,
}

impl ProviderKind {
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::Openai => "OpenAI",
            Self::Gemini => "Google Gemini",
            Self::OpenaiCompatible => "OpenAI-compatible",
            Self::ClaudeCode => "Claude Code",
        }
    }
}

/// How a connection authenticates. Shown to the user so it is always clear
/// whether a subscription, an OAuth grant or a paid API key is being used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    /// API key stored in the OS credential store.
    ApiKey,
    /// Official OpenAI "Sign in with ChatGPT" OAuth (PKCE, loopback) with the
    /// ChatGPT plan usage scope.
    ChatgptOauth,
    /// Delegated to a locally installed official CLI that performs its own sign-in.
    LocalCli,
    /// No credentials (e.g. local Ollama).
    None,
}

/// Who pays for requests on a connection. Displayed next to every answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    /// Pay-per-token API billing on the user's provider account.
    ApiUsage,
    /// Counts against the user's existing chat subscription plan limits.
    SubscriptionPlan,
    /// Determined by the external tool's own sign-in (could be plan or API key).
    ExternalTool,
    /// Self-hosted / local or unknown third-party billing.
    SelfManaged,
}

impl AuthMethod {
    pub fn billing(self) -> Billing {
        match self {
            AuthMethod::ChatgptOauth => Billing::SubscriptionPlan,
            AuthMethod::LocalCli => Billing::ExternalTool,
            // Keyless servers (Ollama, LM Studio) are local/self-managed;
            // anything with an API key is treated as paid API usage.
            AuthMethod::None => Billing::SelfManaged,
            AuthMethod::ApiKey => Billing::ApiUsage,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: Role::Assistant, content: content.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    /// Alternating user/assistant turns, ending with a user turn.
    pub messages: Vec<ChatMessage>,
    pub system: Option<String>,
    /// Upper bound for generated tokens; providers clamp to model limits.
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Cost reported *by the provider itself* in USD, if any. Cue never invents
    /// prices.
    pub reported_cost_usd: Option<f64>,
}

/// Streaming events emitted by every provider in the same shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Incremental answer text.
    Delta { text: String },
    /// Provider confirmed the actual model that served the request.
    Model { model: String },
    /// Token usage (may arrive more than once; last value wins).
    Usage { usage: Usage },
    /// A non-fatal notice (e.g. output truncated at max tokens).
    Notice { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// ID passed to the provider API.
    pub id: String,
    pub display_name: String,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Complete,
    MaxTokens,
    Refusal,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatOutcome {
    pub stop: StopReason,
    pub usage: Usage,
}
