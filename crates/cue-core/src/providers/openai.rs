//! OpenAI via the Responses API (`POST /v1/responses`, SSE).
//!
//! Two officially supported ways to authenticate:
//! * **API key** (platform.openai.com) – pay-per-token API billing.
//! * **ChatGPT plan** – an OAuth access token obtained through the official
//!   "Sign in with ChatGPT" flow for open-source/local apps with the
//!   `chatgpt.tokens.use.direct` scope. Requests then count against the user's
//!   ChatGPT plan. Per OpenAI's docs this route requires `store: false`,
//!   `stream: true`, and rejects parameters such as `temperature` or
//!   `max_output_tokens`, so Cue sends a minimal request body for both modes.
//!   Docs: https://developers.openai.com/siwc/token-sharing-open-source

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{EventTx, Provider, error_body, join_url, kind_for_status, mentions_context_length, send_cancellable, send_delta};
use crate::error::{CueError, CueResult, ErrorKind};
use crate::sse::{Flow, drive_sse};
use crate::types::{ChatOutcome, ChatRequest, ModelInfo, Role, StopReason, StreamEvent, Usage};

const DEFAULT_BASE: &str = "https://api.openai.com/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    ApiKey,
    ChatgptPlan,
}

pub struct OpenAiProvider {
    http: reqwest::Client,
    token: String,
    base: String,
    mode: Mode,
}

impl OpenAiProvider {
    pub fn with_api_key(http: reqwest::Client, key: String, base: Option<String>) -> Self {
        Self {
            http,
            token: key,
            base: base.filter(|b| !b.is_empty()).unwrap_or_else(|| DEFAULT_BASE.into()),
            mode: Mode::ApiKey,
        }
    }

    /// ChatGPT plan usage always targets the public API endpoint
    /// (never ChatGPT's internal backend).
    pub fn with_chatgpt_plan(http: reqwest::Client, access_token: String) -> Self {
        Self { http, token: access_token, base: DEFAULT_BASE.into(), mode: Mode::ChatgptPlan }
    }

    #[cfg(test)]
    pub(crate) fn with_chatgpt_plan_at(http: reqwest::Client, access_token: String, base: String) -> Self {
        Self { http, token: access_token, base, mode: Mode::ChatgptPlan }
    }

    fn name_str(&self) -> &'static str {
        match self.mode {
            Mode::ApiKey => "OpenAI",
            Mode::ChatgptPlan => "ChatGPT",
        }
    }
}

pub(crate) fn map_error(provider: &str, status: u16, req_id: Option<String>, body: &str) -> CueError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    // Error objects appear as {"error":{...}} or, inside stream events, flat.
    let err_obj = parsed.as_ref().map(|v| if v["error"].is_object() { &v["error"] } else { v });
    let code = err_obj.and_then(|e| e["code"].as_str()).unwrap_or("");
    let etype = err_obj.and_then(|e| e["type"].as_str()).unwrap_or("");
    let msg = err_obj.and_then(|e| e["message"].as_str()).unwrap_or(body).to_string();

    let (kind, hint): (ErrorKind, Option<&str>) = match code {
        "invalid_api_key" => (ErrorKind::Auth, None),
        "insufficient_quota" => (
            ErrorKind::QuotaExceeded,
            Some("Check billing at platform.openai.com. API usage is billed separately from ChatGPT subscriptions."),
        ),
        "model_not_found" => (ErrorKind::ModelNotFound, None),
        "context_length_exceeded" => (ErrorKind::ContextTooLong, None),
        "rate_limit_exceeded" => (ErrorKind::RateLimited, None),
        "subscription_sharing_usage_limit_exceeded" => (
            ErrorKind::SubscriptionLimit,
            Some("Your ChatGPT plan limit for apps is reached. Review usage at chatgpt.com/settings/usage."),
        ),
        "subscription_sharing_user_not_eligible" => (
            ErrorKind::PermissionDenied,
            Some("ChatGPT plan usage is not available for this account, workspace or policy. Use an API key instead."),
        ),
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => (
            ErrorKind::Overloaded,
            Some("ChatGPT plan usage is temporarily unavailable. Try again later."),
        ),
        "subscription_sharing_unsupported_capability" => (ErrorKind::InvalidRequest, None),
        "subscription_sharing_invalid_user" => (
            ErrorKind::Auth,
            Some("The ChatGPT sign-in was not accepted. If this persists, sign in again under AI Providers."),
        ),
        "subscription_sharing_route_not_supported"
        | "chatpass_v2_scope_not_authorized"
        | "chatpass_v2_invalid_authorization_context" => (
            ErrorKind::PermissionDenied,
            Some("The ChatGPT authorization does not permit this request. Sign in again and approve plan usage."),
        ),
        _ => {
            let k = match etype {
                "insufficient_quota" => ErrorKind::QuotaExceeded,
                "invalid_request_error" if mentions_context_length(&msg) => ErrorKind::ContextTooLong,
                _ if status == 400 && mentions_context_length(&msg) => ErrorKind::ContextTooLong,
                _ if status == 0 => ErrorKind::Unknown,
                _ => kind_for_status(status),
            };
            (k, None)
        }
    };
    let mut e = CueError::from_kind(kind, provider).with_detail(msg).with_request_id(req_id);
    if status != 0 {
        e = e.with_status(status);
    }
    if let Some(h) = hint {
        e = e.with_hint(h);
    }
    if !code.is_empty() {
        e.detail = Some(format!("{} ({code})", e.detail.unwrap_or_default()));
    }
    e
}

/// Filter the OpenAI API model list down to models usable for text chat.
fn is_chat_model(id: &str) -> bool {
    const EXCLUDE: [&str; 14] = [
        "embedding", "whisper", "tts", "dall-e", "moderation", "image", "audio", "realtime",
        "transcribe", "search", "babbage", "davinci", "sora", "computer-use",
    ];
    let id = id.to_ascii_lowercase();
    (id.starts_with("gpt") || id.starts_with('o') || id.starts_with("chatgpt"))
        && !EXCLUDE.iter().any(|x| id.contains(x))
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        self.name_str()
    }

    async fn list_models(&self) -> CueResult<Vec<ModelInfo>> {
        let name = self.name_str();
        let resp = self
            .http
            .get(join_url(&self.base, "models"))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| CueError::from_reqwest(&e, name))?;
        if !resp.status().is_success() {
            let (s, id, body) = error_body(resp).await;
            return Err(map_error(name, s, id, &body));
        }
        let v: Value = resp.json().await.map_err(|e| CueError::from_reqwest(&e, name))?;

        // ChatGPT plan catalog: {"models":[{slug, display_name, visibility}]},
        // keep server order and only entries meant for display.
        if let Some(models) = v["models"].as_array() {
            return Ok(models
                .iter()
                .filter(|m| m["visibility"].as_str().is_none_or(|vis| vis == "list"))
                .filter_map(|m| {
                    let slug = m["slug"].as_str().or_else(|| m["id"].as_str())?;
                    Some(ModelInfo {
                        id: slug.to_string(),
                        display_name: m["display_name"].as_str().unwrap_or(slug).to_string(),
                        context_window: m["context_window"].as_u64(),
                        max_output_tokens: None,
                    })
                })
                .collect());
        }
        // Standard API: {"data":[{id, created, owned_by}]}
        let mut out: Vec<(i64, ModelInfo)> = v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let id = m["id"].as_str()?;
                is_chat_model(id).then(|| {
                    (
                        m["created"].as_i64().unwrap_or(0),
                        ModelInfo { id: id.into(), display_name: id.into(), context_window: None, max_output_tokens: None },
                    )
                })
            })
            .collect();
        out.sort_by(|a, b| b.0.cmp(&a.0));
        Ok(out.into_iter().map(|(_, m)| m).collect())
    }

    async fn chat(&self, req: &ChatRequest, tx: &EventTx, cancel: &CancellationToken) -> CueResult<ChatOutcome> {
        let name = self.name_str();
        let input: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                json!({
                    "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
                    "content": m.content,
                })
            })
            .collect();
        let mut body = json!({
            "model": req.model,
            "input": input,
            "store": false,
            "stream": true,
        });
        if let Some(sys) = req.system.as_deref().filter(|s| !s.is_empty()) {
            body["instructions"] = json!(sys);
        }
        if self.mode == Mode::ApiKey {
            if let Some(m) = req.max_output_tokens {
                body["max_output_tokens"] = json!(m);
            }
        }

        let rb = self.http.post(join_url(&self.base, "responses")).bearer_auth(&self.token).json(&body);
        let resp = send_cancellable(rb, name, cancel).await?;
        if !resp.status().is_success() {
            let (s, id, body) = error_body(resp).await;
            return Err(map_error(name, s, id, &body));
        }

        let mut usage = Usage::default();
        let mut stop = StopReason::Other;
        let mut terminal = false;
        let mut model_sent = false;
        drive_sse(resp, name, cancel, |ev| {
            if ev.data == "[DONE]" {
                return Ok(Flow::Stop);
            }
            let v: Value = match serde_json::from_str(&ev.data) {
                Ok(v) => v,
                Err(_) => return Ok(Flow::Continue),
            };
            let etype = v["type"].as_str().or(ev.event.as_deref()).unwrap_or("").to_string();
            let read_usage = |resp: &Value, usage: &mut Usage| {
                if let Some(i) = resp["usage"]["input_tokens"].as_u64() {
                    usage.input_tokens = Some(i);
                }
                if let Some(o) = resp["usage"]["output_tokens"].as_u64() {
                    usage.output_tokens = Some(o);
                }
            };
            match etype.as_str() {
                "response.created" | "response.in_progress" => {
                    if let (false, Some(m)) = (model_sent, v["response"]["model"].as_str()) {
                        model_sent = true;
                        let _ = tx.send(StreamEvent::Model { model: m.to_string() });
                    }
                }
                "response.output_text.delta" => send_delta(tx, v["delta"].as_str().unwrap_or("")),
                "response.refusal.delta" => {
                    stop = StopReason::Refusal;
                    send_delta(tx, v["delta"].as_str().unwrap_or(""));
                }
                "response.completed" => {
                    read_usage(&v["response"], &mut usage);
                    if stop != StopReason::Refusal {
                        stop = StopReason::Complete;
                    }
                    terminal = true;
                    return Ok(Flow::Stop);
                }
                "response.incomplete" => {
                    read_usage(&v["response"], &mut usage);
                    let reason = v["response"]["incomplete_details"]["reason"].as_str().unwrap_or("unknown");
                    stop = if reason == "max_output_tokens" { StopReason::MaxTokens } else { StopReason::Other };
                    let _ = tx.send(StreamEvent::Notice { message: format!("Response incomplete: {reason}.") });
                    terminal = true;
                    return Ok(Flow::Stop);
                }
                "response.failed" => {
                    let err = &v["response"]["error"];
                    return Err(map_error(name, 0, None, &json!({ "error": err }).to_string()));
                }
                "error" => return Err(map_error(name, 0, None, &v.to_string())),
                _ => {}
            }
            Ok(Flow::Continue)
        })
        .await?;

        if !terminal {
            // OpenAI: treat inference as successful only after a terminal event.
            return Err(CueError::from_kind(ErrorKind::Network, name)
                .with_detail("The stream ended without a completion event."));
        }
        let _ = tx.send(StreamEvent::Usage { usage: usage.clone() });
        Ok(ChatOutcome { stop, usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_model_filter() {
        assert!(is_chat_model("gpt-4.1-mini"));
        assert!(is_chat_model("o4-mini"));
        assert!(!is_chat_model("text-embedding-3-small"));
        assert!(!is_chat_model("gpt-4o-realtime-preview"));
        assert!(!is_chat_model("omni-moderation-latest"));
        assert!(!is_chat_model("whisper-1"));
    }

    #[test]
    fn maps_plan_limit() {
        let e = map_error("ChatGPT", 429, None, r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"limit"}}"#);
        assert_eq!(e.kind, ErrorKind::SubscriptionLimit);
        let e = map_error("OpenAI", 429, None, r#"{"error":{"code":"insufficient_quota","type":"insufficient_quota","message":"You exceeded your current quota"}}"#);
        assert_eq!(e.kind, ErrorKind::QuotaExceeded);
        let e = map_error("OpenAI", 429, None, r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#);
        assert_eq!(e.kind, ErrorKind::RateLimited);
    }
}
