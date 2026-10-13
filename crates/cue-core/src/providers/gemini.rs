//! Google Gemini API (Google AI Studio API key).
//! Docs: https://ai.google.dev/api/generate-content
//!
//! Google AI Pro/Ultra subscriptions do not cover Gemini API usage; per
//! Google's docs, API use via keys or external apps is billed separately.

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

const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const NAME: &str = "Google Gemini";

pub struct GeminiProvider {
    http: reqwest::Client,
    key: String,
    base: String,
}

impl GeminiProvider {
    pub fn new(http: reqwest::Client, key: String, base: Option<String>) -> Self {
        Self {
            http,
            key,
            base: base
                .filter(|b| !b.is_empty())
                .unwrap_or_else(|| DEFAULT_BASE.into()),
        }
    }
}

pub(crate) fn map_error(status: u16, req_id: Option<String>, body: &str) -> CueError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let err = parsed.as_ref().map(|v| &v["error"]);
    let msg = err
        .and_then(|e| e["message"].as_str())
        .unwrap_or(body)
        .to_string();
    let gstatus = err.and_then(|e| e["status"].as_str()).unwrap_or("");
    let reason = err
        .and_then(|e| e["details"].as_array())
        .and_then(|d| d.iter().find_map(|x| x["reason"].as_str()))
        .unwrap_or("");
    let kind = match (gstatus, reason) {
        (_, "API_KEY_INVALID") | ("UNAUTHENTICATED", _) => ErrorKind::Auth,
        ("PERMISSION_DENIED", _) => ErrorKind::PermissionDenied,
        ("NOT_FOUND", _) => ErrorKind::ModelNotFound,
        ("RESOURCE_EXHAUSTED", _) => {
            let l = msg.to_ascii_lowercase();
            if l.contains("quota") && (l.contains("billing") || l.contains("plan")) {
                ErrorKind::QuotaExceeded
            } else {
                ErrorKind::RateLimited
            }
        }
        ("UNAVAILABLE", _) | ("INTERNAL", _) => ErrorKind::Overloaded,
        ("FAILED_PRECONDITION", _) => ErrorKind::PermissionDenied,
        ("INVALID_ARGUMENT", _) if mentions_context_length(&msg) => ErrorKind::ContextTooLong,
        ("INVALID_ARGUMENT", _) if msg.to_ascii_lowercase().contains("api key") => ErrorKind::Auth,
        _ => kind_for_status(status),
    };
    let mut e = CueError::from_kind(kind, NAME)
        .with_detail(msg)
        .with_request_id(req_id);
    if status != 0 {
        e = e.with_status(status);
    }
    if kind == ErrorKind::QuotaExceeded || kind == ErrorKind::RateLimited {
        e = e.with_hint(
            "Check your quota at aistudio.google.com. Gemini API usage is separate from Google AI Pro/Ultra subscriptions.",
        );
    }
    e
}

#[async_trait]
impl Provider for GeminiProvider {
    fn name(&self) -> &str {
        NAME
    }

    async fn list_models(&self) -> CueResult<Vec<ModelInfo>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        for _ in 0..20 {
            let mut rb = self
                .http
                .get(join_url(&self.base, "models"))
                .header("x-goog-api-key", &self.key)
                .query(&[("pageSize", "1000")]);
            if let Some(p) = &page {
                rb = rb.query(&[("pageToken", p.as_str())]);
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
            for m in v["models"].as_array().into_iter().flatten() {
                let supports_chat = m["supportedGenerationMethods"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x == "generateContent"));
                let Some(name) = m["name"].as_str() else {
                    continue;
                };
                let id = name.strip_prefix("models/").unwrap_or(name);
                // Skip embedding / image / audio-only variants.
                let lower = id.to_ascii_lowercase();
                if !supports_chat
                    || ["embedding", "imagen", "veo", "-tts", "aqa"]
                        .iter()
                        .any(|x| lower.contains(x))
                {
                    continue;
                }
                out.push(ModelInfo {
                    id: id.to_string(),
                    display_name: m["displayName"].as_str().unwrap_or(id).to_string(),
                    context_window: m["inputTokenLimit"].as_u64(),
                    max_output_tokens: m["outputTokenLimit"].as_u64(),
                });
            }
            page = v["nextPageToken"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            if page.is_none() {
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
        let contents: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                json!({
                    "role": match m.role { Role::User => "user", Role::Assistant => "model" },
                    "parts": [{ "text": m.content }],
                })
            })
            .collect();
        let mut body = json!({ "contents": contents });
        if let Some(sys) = req.system.as_deref().filter(|s| !s.is_empty()) {
            body["systemInstruction"] = json!({ "parts": [{ "text": sys }] });
        }
        if let Some(m) = req.max_output_tokens {
            body["generationConfig"] = json!({ "maxOutputTokens": m });
        }
        let model = req.model.strip_prefix("models/").unwrap_or(&req.model);
        let url = join_url(&self.base, &format!("models/{model}:streamGenerateContent"));
        let rb = self
            .http
            .post(url)
            .query(&[("alt", "sse")])
            .header("x-goog-api-key", &self.key)
            .json(&body);
        let resp = send_cancellable(rb, NAME, cancel).await?;
        if !resp.status().is_success() {
            let (s, id, body) = error_body(resp).await;
            return Err(map_error(s, id, &body));
        }

        let mut usage = Usage::default();
        let mut stop = StopReason::Other;
        let mut model_sent = false;
        let mut saw_finish = false;
        drive_sse(resp, NAME, cancel, |ev| {
            let v: Value = match serde_json::from_str(&ev.data) {
                Ok(v) => v,
                Err(_) => return Ok(Flow::Continue),
            };
            if v["error"].is_object() {
                return Err(map_error(
                    v["error"]["code"].as_u64().unwrap_or(0) as u16,
                    None,
                    &v.to_string(),
                ));
            }
            if let (false, Some(m)) = (model_sent, v["modelVersion"].as_str()) {
                model_sent = true;
                let _ = tx.send(StreamEvent::Model {
                    model: m.to_string(),
                });
            }
            if let Some(reason) = v["promptFeedback"]["blockReason"].as_str() {
                stop = StopReason::Refusal;
                saw_finish = true;
                let _ = tx.send(StreamEvent::Notice {
                    message: format!("Prompt blocked by Gemini ({reason})."),
                });
            }
            if let Some(cand) = v["candidates"].get(0) {
                for part in cand["content"]["parts"].as_array().into_iter().flatten() {
                    // Thought summaries are flagged with "thought": true; not shown.
                    if part["thought"].as_bool() != Some(true) {
                        send_delta(tx, part["text"].as_str().unwrap_or(""));
                    }
                }
                if let Some(fr) = cand["finishReason"].as_str() {
                    saw_finish = true;
                    stop = match fr {
                        "STOP" => StopReason::Complete,
                        "MAX_TOKENS" => StopReason::MaxTokens,
                        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => {
                            StopReason::Refusal
                        }
                        _ => StopReason::Other,
                    };
                }
            }
            let um = &v["usageMetadata"];
            if let Some(p) = um["promptTokenCount"].as_u64() {
                usage.input_tokens = Some(p);
            }
            let out = um["candidatesTokenCount"].as_u64().unwrap_or(0)
                + um["thoughtsTokenCount"].as_u64().unwrap_or(0);
            if out > 0 {
                usage.output_tokens = Some(out);
            }
            Ok(Flow::Continue)
        })
        .await?;

        if !saw_finish {
            return Err(CueError::from_kind(ErrorKind::Network, NAME)
                .with_detail("The stream ended without a finish reason."));
        }
        let _ = tx.send(StreamEvent::Usage {
            usage: usage.clone(),
        });
        Ok(ChatOutcome { stop, usage })
    }
}
