//! Anthropic Claude API (Messages API, SSE streaming).
//! Docs: https://platform.claude.com/docs/en/api/messages
//!
//! Authentication is exclusively by API key (`x-api-key`). Anthropic's terms
//! do not allow third-party apps to offer Claude.ai (Free/Pro/Max) sign-in, so
//! this provider never uses subscription OAuth tokens.

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{
    EventTx, Provider, error_body, join_url, kind_for_status, mentions_context_length,
    send_cancellable, send_delta,
};
use crate::error::{CueError, CueResult, ErrorKind};
use crate::sse::{Flow, drive_sse};
use crate::types::{ChatOutcome, ChatRequest, ModelInfo, Role, StopReason, StreamEvent, Usage};

const DEFAULT_BASE: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";
const NAME: &str = "Anthropic";
/// Used when the Models API does not report a model's output limit.
const FALLBACK_MAX_TOKENS: u32 = 8192;
const PREFERRED_MAX_TOKENS: u32 = 16000;

pub struct AnthropicProvider {
    http: reqwest::Client,
    key: String,
    base: String,
    /// Output limits learned from the Models API, keyed by model id.
    limits: std::sync::Mutex<std::collections::HashMap<String, u64>>,
}

impl AnthropicProvider {
    pub fn new(http: reqwest::Client, key: String, base: Option<String>) -> Self {
        Self {
            http,
            key,
            base: base
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE.into()),
            limits: Default::default(),
        }
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, join_url(&self.base, path))
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
    }

    fn max_tokens_for(&self, req: &ChatRequest) -> u32 {
        let model_cap = self
            .limits
            .lock()
            .ok()
            .and_then(|m| m.get(&req.model).copied());
        match (req.max_output_tokens, model_cap) {
            (Some(m), Some(cap)) => (m as u64).min(cap) as u32,
            (Some(m), None) => m,
            (None, Some(cap)) => cap.min(PREFERRED_MAX_TOKENS as u64) as u32,
            (None, None) => FALLBACK_MAX_TOKENS,
        }
    }
}

pub(crate) fn map_error(status: u16, req_id: Option<String>, body: &str) -> CueError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let etype = parsed
        .as_ref()
        .and_then(|v| v["error"]["type"].as_str())
        .unwrap_or("");
    let msg = parsed
        .as_ref()
        .and_then(|v| v["error"]["message"].as_str())
        .unwrap_or(body)
        .to_string();
    let kind = match etype {
        "authentication_error" => ErrorKind::Auth,
        "permission_error" => ErrorKind::PermissionDenied,
        "not_found_error" => ErrorKind::ModelNotFound,
        "rate_limit_error" => ErrorKind::RateLimited,
        "overloaded_error" | "api_error" => ErrorKind::Overloaded,
        "billing_error" => ErrorKind::QuotaExceeded,
        "request_too_large" => ErrorKind::ContextTooLong,
        "invalid_request_error" => {
            let lower = msg.to_ascii_lowercase();
            if lower.contains("credit balance") || lower.contains("billing") {
                ErrorKind::QuotaExceeded
            } else if mentions_context_length(&msg) {
                ErrorKind::ContextTooLong
            } else if lower.contains("model")
                && (lower.contains("not found") || lower.contains("does not exist"))
            {
                ErrorKind::ModelNotFound
            } else {
                ErrorKind::InvalidRequest
            }
        }
        _ => kind_for_status(status),
    };
    let mut err = CueError::from_kind(kind, NAME)
        .with_detail(msg)
        .with_request_id(req_id);
    if status != 0 {
        err = err.with_status(status);
    }
    if kind == ErrorKind::QuotaExceeded {
        err = err.with_hint(
            "Add credits at platform.claude.com → Billing. API usage is billed separately from Claude Pro/Max subscriptions.",
        );
    }
    err
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        NAME
    }

    async fn list_models(&self) -> CueResult<Vec<ModelInfo>> {
        let mut out = Vec::new();
        let mut after: Option<String> = None;
        for _ in 0..20 {
            let mut rb = self
                .req(reqwest::Method::GET, "/v1/models")
                .query(&[("limit", "1000")]);
            if let Some(a) = &after {
                rb = rb.query(&[("after_id", a.as_str())]);
            }
            let resp = rb
                .send()
                .await
                .map_err(|e| CueError::from_reqwest(&e, NAME))?;
            if !resp.status().is_success() {
                let (s, id, body) = error_body(resp).await;
                return Err(map_error(s, id, &body));
            }
            let v: Value = resp
                .json()
                .await
                .map_err(|e| CueError::from_reqwest(&e, NAME))?;
            for m in v["data"].as_array().into_iter().flatten() {
                let Some(id) = m["id"].as_str() else { continue };
                let max_out = m["max_tokens"].as_u64();
                if let (Some(mo), Ok(mut l)) = (max_out, self.limits.lock()) {
                    l.insert(id.to_string(), mo);
                }
                out.push(ModelInfo {
                    id: id.to_string(),
                    display_name: m["display_name"].as_str().unwrap_or(id).to_string(),
                    context_window: m["max_input_tokens"].as_u64(),
                    max_output_tokens: max_out,
                });
            }
            if v["has_more"].as_bool() == Some(true) {
                after = v["last_id"].as_str().map(str::to_string);
                if after.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(out)
    }

    async fn chat(
        &self,
        req: &ChatRequest,
        tx: &EventTx,
        cancel: &CancellationToken,
    ) -> CueResult<ChatOutcome> {
        let messages: Vec<Value> = req
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
            "max_tokens": self.max_tokens_for(req),
            "messages": messages,
            "stream": true,
        });
        if let Some(sys) = req.system.as_deref().filter(|s| !s.is_empty()) {
            body["system"] = json!(sys);
        }

        let resp = send_cancellable(
            self.req(reqwest::Method::POST, "/v1/messages").json(&body),
            NAME,
            cancel,
        )
        .await?;
        if !resp.status().is_success() {
            let (s, id, body) = error_body(resp).await;
            return Err(map_error(s, id, &body));
        }

        let mut usage = Usage::default();
        let mut stop = StopReason::Other;
        let mut finished = false;
        drive_sse(resp, NAME, cancel, |ev| {
            let v: Value = match serde_json::from_str(&ev.data) {
                Ok(v) => v,
                Err(_) => return Ok(Flow::Continue),
            };
            match v["type"].as_str().unwrap_or("") {
                "message_start" => {
                    let msg = &v["message"];
                    if let Some(m) = msg["model"].as_str() {
                        let _ = tx.send(StreamEvent::Model {
                            model: m.to_string(),
                        });
                    }
                    usage.input_tokens = msg["usage"]["input_tokens"].as_u64().map(|n| {
                        n + msg["usage"]["cache_read_input_tokens"]
                            .as_u64()
                            .unwrap_or(0)
                            + msg["usage"]["cache_creation_input_tokens"]
                                .as_u64()
                                .unwrap_or(0)
                    });
                }
                "content_block_delta" => {
                    // Only visible text is shown; thinking deltas are skipped.
                    if v["delta"]["type"] == "text_delta" {
                        send_delta(tx, v["delta"]["text"].as_str().unwrap_or(""));
                    }
                }
                "message_delta" => {
                    if let Some(o) = v["usage"]["output_tokens"].as_u64() {
                        usage.output_tokens = Some(o);
                    }
                    stop = match v["delta"]["stop_reason"].as_str() {
                        Some("end_turn") | Some("stop_sequence") => StopReason::Complete,
                        Some("max_tokens") => StopReason::MaxTokens,
                        Some("refusal") => StopReason::Refusal,
                        _ => stop,
                    };
                }
                "message_stop" => {
                    finished = true;
                    return Ok(Flow::Stop);
                }
                "error" => {
                    let body = v.to_string();
                    return Err(map_error(0, None, &body));
                }
                _ => {}
            }
            Ok(Flow::Continue)
        })
        .await?;

        if !finished {
            return Err(CueError::from_kind(ErrorKind::Network, NAME)
                .with_detail("The stream ended before the message was complete."));
        }
        let _ = tx.send(StreamEvent::Usage {
            usage: usage.clone(),
        });
        Ok(ChatOutcome { stop, usage })
    }
}
