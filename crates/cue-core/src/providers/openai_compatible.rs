//! Generic OpenAI Chat Completions–compatible servers: local runtimes such as
//! Ollama or LM Studio, and hosted gateways such as OpenRouter, Groq or
//! Mistral. Billing (if any) is handled by the respective service.

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::{EventTx, Provider, error_body, join_url, send_cancellable, send_delta};
use crate::error::{CueError, CueResult, ErrorKind};
use crate::sse::{Flow, drive_sse};
use crate::types::{ChatOutcome, ChatRequest, ModelInfo, Role, StopReason, StreamEvent, Usage};

const NAME: &str = "OpenAI-compatible server";

pub struct OpenAiCompatibleProvider {
    http: reqwest::Client,
    base: String,
    key: Option<String>,
}

impl OpenAiCompatibleProvider {
    pub fn new(http: reqwest::Client, base: String, key: Option<String>) -> Self {
        Self { http, base, key }
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.key {
            Some(k) => rb.bearer_auth(k),
            None => rb,
        }
    }
}

fn map(status: u16, req_id: Option<String>, body: &str) -> CueError {
    let mut e = super::openai::map_error(NAME, status, req_id, body);
    if e.kind == ErrorKind::Network || e.kind == ErrorKind::Unknown {
        e = e.with_hint("Make sure the server is running and the base URL is correct.");
    }
    e
}

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        NAME
    }

    async fn list_models(&self) -> CueResult<Vec<ModelInfo>> {
        let resp = self
            .auth(self.http.get(join_url(&self.base, "models")))
            .send()
            .await
            .map_err(|e| {
                CueError::from_reqwest(&e, NAME)
                    .with_hint("Make sure the server is running and the base URL is correct.")
            })?;
        if !resp.status().is_success() {
            let (s, id, body) = error_body(resp).await;
            return Err(map(s, id, &body));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| CueError::from_reqwest(&e, NAME))?;
        Ok(v["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let id = m["id"].as_str()?;
                Some(ModelInfo {
                    id: id.to_string(),
                    display_name: m["name"].as_str().unwrap_or(id).to_string(),
                    context_window: m["context_length"].as_u64(),
                    max_output_tokens: None,
                })
            })
            .collect())
    }

    async fn chat(
        &self,
        req: &ChatRequest,
        tx: &EventTx,
        cancel: &CancellationToken,
    ) -> CueResult<ChatOutcome> {
        let mut messages: Vec<Value> = Vec::new();
        if let Some(sys) = req.system.as_deref().filter(|s| !s.is_empty()) {
            messages.push(json!({ "role": "system", "content": sys }));
        }
        messages.extend(req.messages.iter().map(|m| {
            json!({
                "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
                "content": m.content,
            })
        }));
        let mut body = json!({
            "model": req.model,
            "messages": messages,
            "stream": true,
            "stream_options": { "include_usage": true },
        });
        if let Some(m) = req.max_output_tokens {
            body["max_tokens"] = json!(m);
        }
        let rb = self.auth(
            self.http
                .post(join_url(&self.base, "chat/completions"))
                .json(&body),
        );
        let resp = send_cancellable(rb, NAME, cancel).await?;
        if !resp.status().is_success() {
            let (s, id, body) = error_body(resp).await;
            return Err(map(s, id, &body));
        }

        let mut usage = Usage::default();
        let mut stop = StopReason::Other;
        let mut done = false;
        let mut model_sent = false;
        drive_sse(resp, NAME, cancel, |ev| {
            if ev.data.trim() == "[DONE]" {
                done = true;
                return Ok(Flow::Stop);
            }
            let v: Value = match serde_json::from_str(&ev.data) {
                Ok(v) => v,
                Err(_) => return Ok(Flow::Continue),
            };
            if v["error"].is_object() {
                return Err(map(0, None, &v.to_string()));
            }
            if let (false, Some(m)) = (model_sent, v["model"].as_str()) {
                model_sent = true;
                let _ = tx.send(StreamEvent::Model {
                    model: m.to_string(),
                });
            }
            if let Some(choice) = v["choices"].get(0) {
                send_delta(tx, choice["delta"]["content"].as_str().unwrap_or(""));
                match choice["finish_reason"].as_str() {
                    Some("stop") => stop = StopReason::Complete,
                    Some("length") => stop = StopReason::MaxTokens,
                    Some("content_filter") => stop = StopReason::Refusal,
                    _ => {}
                }
            }
            if v["usage"].is_object() {
                usage.input_tokens = v["usage"]["prompt_tokens"].as_u64();
                usage.output_tokens = v["usage"]["completion_tokens"].as_u64();
                // OpenRouter reports the actual charged cost in the usage object.
                usage.reported_cost_usd = v["usage"]["cost"].as_f64();
            }
            Ok(Flow::Continue)
        })
        .await?;

        if !done && stop == StopReason::Other {
            return Err(CueError::from_kind(ErrorKind::Network, NAME)
                .with_detail("The stream ended unexpectedly."));
        }
        let _ = tx.send(StreamEvent::Usage {
            usage: usage.clone(),
        });
        Ok(ChatOutcome { stop, usage })
    }
}
