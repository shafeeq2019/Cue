//! Provider tests against local mock servers that replay the documented wire
//! formats (SSE event shapes and error bodies) of each API.

use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::types::{ChatMessage, StopReason};

fn req(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.into(),
        messages: vec![
            ChatMessage::user("Hi"),
            ChatMessage::assistant("Hello!"),
            ChatMessage::user("Say ok"),
        ],
        system: Some("Be brief.".into()),
        max_output_tokens: Some(256),
    }
}

fn sse(events: &[(&str, &str)]) -> ResponseTemplate {
    let mut body = String::new();
    for (ev, data) in events {
        if !ev.is_empty() {
            body.push_str(&format!("event: {ev}\n"));
        }
        body.push_str(&format!("data: {data}\n\n"));
    }
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

async fn run(
    p: &dyn Provider,
    r: &ChatRequest,
) -> (CueResult<ChatOutcome>, String, Vec<StreamEvent>) {
    let (tx, mut rx) = unbounded_channel();
    let out = p.chat(r, &tx, &CancellationToken::new()).await;
    drop(tx);
    let mut text = String::new();
    let mut evs = vec![];
    while let Some(ev) = rx.recv().await {
        if let StreamEvent::Delta { text: t } = &ev {
            text.push_str(t);
        }
        evs.push(ev);
    }
    (out, text, evs)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

// ---------------------------------------------------------------- Anthropic

#[tokio::test]
async fn anthropic_stream_success() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-test"))
        .and(header("anthropic-version", "2023-06-01"))
        .and(body_partial_json(serde_json::json!({
            "model": "claude-x", "stream": true, "system": "Be brief.", "max_tokens": 256,
            "messages": [{"role":"user","content":"Hi"},{"role":"assistant","content":"Hello!"},{"role":"user","content":"Say ok"}]
        })))
        .respond_with(sse(&[
            ("message_start", r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-x","usage":{"input_tokens":12,"output_tokens":1}}}"#),
            ("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#),
            ("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hidden"}}"#),
            ("content_block_start", r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#),
            ("ping", r#"{"type":"ping"}"#),
            ("content_block_delta", r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"o"}}"#),
            ("content_block_delta", r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"k"}}"#),
            ("content_block_stop", r#"{"type":"content_block_stop","index":1}"#),
            ("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ]))
        .mount(&s)
        .await;
    let p = anthropic::AnthropicProvider::new(http(), "sk-test".into(), Some(s.uri()));
    let (out, text, evs) = run(&p, &req("claude-x")).await;
    let out = out.unwrap();
    assert_eq!(text, "ok", "thinking must not leak into the answer");
    assert_eq!(out.stop, StopReason::Complete);
    assert_eq!(
        (out.usage.input_tokens, out.usage.output_tokens),
        (Some(12), Some(5))
    );
    assert!(evs.contains(&StreamEvent::Model {
        model: "claude-x".into()
    }));
}

#[tokio::test]
async fn anthropic_error_mapping() {
    let cases = [
        (
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
            ErrorKind::Auth,
        ),
        (
            404,
            r#"{"type":"error","error":{"type":"not_found_error","message":"model: nope"}}"#,
            ErrorKind::ModelNotFound,
        ),
        (
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow"}}"#,
            ErrorKind::RateLimited,
        ),
        (
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            ErrorKind::Overloaded,
        ),
        (
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#,
            ErrorKind::QuotaExceeded,
        ),
        (
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 300000 tokens > 200000 maximum"}}"#,
            ErrorKind::ContextTooLong,
        ),
    ];
    for (status, body, kind) in cases {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("request-id", "req_1")
                    .set_body_string(body),
            )
            .mount(&s)
            .await;
        let p = anthropic::AnthropicProvider::new(http(), "sk-test".into(), Some(s.uri()));
        let e = run(&p, &req("m")).await.0.unwrap_err();
        assert_eq!(e.kind, kind, "{body}");
        assert_eq!(e.status, Some(status));
        assert_eq!(e.request_id.as_deref(), Some("req_1"));
    }
}

#[tokio::test]
async fn anthropic_midstream_error_and_truncation() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(&[
            ("message_start", r#"{"type":"message_start","message":{"model":"m","usage":{"input_tokens":1}}}"#),
            ("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"par"}}"#),
            ("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#),
        ]))
        .mount(&s)
        .await;
    let p = anthropic::AnthropicProvider::new(http(), "k".into(), Some(s.uri()));
    let (out, text, _) = run(&p, &req("m")).await;
    assert_eq!(out.unwrap_err().kind, ErrorKind::Overloaded);
    assert_eq!(text, "par");

    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(&[(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}"#,
        )]))
        .mount(&s)
        .await;
    let p = anthropic::AnthropicProvider::new(http(), "k".into(), Some(s.uri()));
    assert_eq!(
        run(&p, &req("m")).await.0.unwrap_err().kind,
        ErrorKind::Network,
        "stream without message_stop is an error"
    );
}

#[tokio::test]
async fn anthropic_models_pagination() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("after_id", "b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data":[{"id":"c","display_name":"C","max_tokens":64000,"max_input_tokens":1000000}],"has_more":false,"last_id":"c"
        })))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data":[{"id":"a","display_name":"A"},{"id":"b","display_name":"B"}],"has_more":true,"last_id":"b"
        })))
        .mount(&s)
        .await;
    let p = anthropic::AnthropicProvider::new(http(), "k".into(), Some(s.uri()));
    let m = p.list_models().await.unwrap();
    assert_eq!(
        m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(m[2].max_output_tokens, Some(64000));
    assert_eq!(m[2].context_window, Some(1_000_000));
}

#[tokio::test]
async fn cancellation_stops_request() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(30)))
        .mount(&s)
        .await;
    let p = anthropic::AnthropicProvider::new(http(), "k".into(), Some(s.uri()));
    let (tx, _rx) = unbounded_channel();
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        c2.cancel();
    });
    let started = std::time::Instant::now();
    let e = p.chat(&req("m"), &tx, &cancel).await.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Cancelled);
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn network_error_is_reported() {
    // Nothing listens on this port.
    let p =
        anthropic::AnthropicProvider::new(http(), "k".into(), Some("http://127.0.0.1:9".into()));
    let e = run(&p, &req("m")).await.0.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Network);
    assert!(e.retryable);
}

// ---------------------------------------------------------------- OpenAI

fn openai_stream() -> ResponseTemplate {
    sse(&[
        (
            "response.created",
            r#"{"type":"response.created","response":{"id":"r1","model":"gpt-x-2025","status":"in_progress"}}"#,
        ),
        (
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","delta":"o"}"#,
        ),
        (
            "response.output_text.delta",
            r#"{"type":"response.output_text.delta","delta":"k"}"#,
        ),
        (
            "response.completed",
            r#"{"type":"response.completed","response":{"id":"r1","status":"completed","usage":{"input_tokens":7,"output_tokens":2}}}"#,
        ),
    ])
}

#[tokio::test]
async fn openai_api_key_stream() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer sk-o"))
        .and(body_partial_json(serde_json::json!({
            "model":"gpt-x","store":false,"stream":true,"instructions":"Be brief.","max_output_tokens":256,
            "input":[{"role":"user","content":"Hi"},{"role":"assistant","content":"Hello!"},{"role":"user","content":"Say ok"}]
        })))
        .respond_with(openai_stream())
        .mount(&s)
        .await;
    let p = openai::OpenAiProvider::with_api_key(http(), "sk-o".into(), Some(s.uri()));
    let (out, text, evs) = run(&p, &req("gpt-x")).await;
    let out = out.unwrap();
    assert_eq!(text, "ok");
    assert_eq!(out.usage.output_tokens, Some(2));
    assert!(evs.contains(&StreamEvent::Model {
        model: "gpt-x-2025".into()
    }));
}

#[tokio::test]
async fn chatgpt_plan_request_shape_and_limit_error() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(header("authorization", "Bearer oauth-at"))
        .respond_with(move |r: &wiremock::Request| {
            let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            // Documented requirements of the ChatGPT plan route.
            assert_eq!(body["store"], false);
            assert_eq!(body["stream"], true);
            assert!(body.get("max_output_tokens").is_none(), "unsupported on plan route");
            assert!(body.get("temperature").is_none());
            sse(&[
                ("response.output_text.delta", r#"{"type":"response.output_text.delta","delta":"pa"}"#),
                ("response.failed", r#"{"type":"response.failed","response":{"status":"failed","error":{"code":"subscription_sharing_usage_limit_exceeded","message":"Usage limit reached"}}}"#),
            ])
        })
        .mount(&s)
        .await;
    let p = openai::OpenAiProvider::with_chatgpt_plan_at(http(), "oauth-at".into(), s.uri());
    let (out, text, _) = run(&p, &req("gpt-x")).await;
    let e = out.unwrap_err();
    assert_eq!(e.kind, ErrorKind::SubscriptionLimit);
    assert!(e.hint.unwrap().contains("chatgpt.com/settings/usage"));
    assert_eq!(text, "pa");
}

#[tokio::test]
async fn chatgpt_plan_model_catalog() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models":[
                {"slug":"gpt-b","display_name":"GPT B","visibility":"list"},
                {"slug":"internal","display_name":"Hidden","visibility":"hide"},
                {"slug":"gpt-a","display_name":"GPT A","visibility":"list"}
            ]
        })))
        .mount(&s)
        .await;
    let p = openai::OpenAiProvider::with_chatgpt_plan_at(http(), "t".into(), s.uri());
    let m = p.list_models().await.unwrap();
    assert_eq!(
        m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["gpt-b", "gpt-a"],
        "server order kept, hidden dropped"
    );
}

#[tokio::test]
async fn openai_errors_and_incomplete() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error":{"message":"Incorrect API key provided: sk-abc***xyz.","type":"invalid_request_error","code":"invalid_api_key"}
        })))
        .mount(&s)
        .await;
    let p = openai::OpenAiProvider::with_api_key(http(), "k".into(), Some(s.uri()));
    assert_eq!(
        run(&p, &req("m")).await.0.unwrap_err().kind,
        ErrorKind::Auth
    );

    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(&[
            ("response.output_text.delta", r#"{"type":"response.output_text.delta","delta":"long"}"#),
            ("response.incomplete", r#"{"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":1,"output_tokens":256}}}"#),
        ]))
        .mount(&s)
        .await;
    let p = openai::OpenAiProvider::with_api_key(http(), "k".into(), Some(s.uri()));
    let (out, _, evs) = run(&p, &req("m")).await;
    assert_eq!(out.unwrap().stop, StopReason::MaxTokens);
    assert!(evs.iter().any(|e| matches!(e, StreamEvent::Notice { .. })));

    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(&[(
            "",
            r#"{"type":"error","code":"rate_limit_exceeded","message":"Rate limit reached"}"#,
        )]))
        .mount(&s)
        .await;
    let p = openai::OpenAiProvider::with_api_key(http(), "k".into(), Some(s.uri()));
    assert_eq!(
        run(&p, &req("m")).await.0.unwrap_err().kind,
        ErrorKind::RateLimited
    );
}

#[tokio::test]
async fn openai_api_model_list_filters_non_chat() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object":"list","data":[
                {"id":"gpt-old","created":1},{"id":"text-embedding-3-large","created":5},{"id":"gpt-new","created":9},{"id":"whisper-1","created":3}
            ]
        })))
        .mount(&s)
        .await;
    let p = openai::OpenAiProvider::with_api_key(http(), "k".into(), Some(s.uri()));
    let ids: Vec<String> = p
        .list_models()
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, ["gpt-new", "gpt-old"]);
}

// ---------------------------------------------------------------- Gemini

#[tokio::test]
async fn gemini_stream_success() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/models/gemini-x:streamGenerateContent"))
        .and(query_param("alt", "sse"))
        .and(header("x-goog-api-key", "AIza-test"))
        .and(body_partial_json(serde_json::json!({
            "systemInstruction":{"parts":[{"text":"Be brief."}]},
            "contents":[{"role":"user","parts":[{"text":"Hi"}]},{"role":"model","parts":[{"text":"Hello!"}]},{"role":"user","parts":[{"text":"Say ok"}]}],
            "generationConfig":{"maxOutputTokens":256}
        })))
        .respond_with(sse(&[
            ("", r#"{"candidates":[{"content":{"parts":[{"text":"thinking...","thought":true}],"role":"model"}}],"modelVersion":"gemini-x-001"}"#),
            ("", r#"{"candidates":[{"content":{"parts":[{"text":"o"}],"role":"model"}}]}"#),
            ("", r#"{"candidates":[{"content":{"parts":[{"text":"k"}],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":2,"thoughtsTokenCount":3}}"#),
        ]))
        .mount(&s)
        .await;
    let p = gemini::GeminiProvider::new(http(), "AIza-test".into(), Some(s.uri()));
    let (out, text, evs) = run(&p, &req("gemini-x")).await;
    let out = out.unwrap();
    assert_eq!(text, "ok");
    assert_eq!(out.usage.output_tokens, Some(5));
    assert!(evs.contains(&StreamEvent::Model {
        model: "gemini-x-001".into()
    }));
}

#[tokio::test]
async fn gemini_errors_and_models() {
    let cases = [
        (
            400,
            r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"API_KEY_INVALID"}]}}"#,
            ErrorKind::Auth,
        ),
        (
            404,
            r#"{"error":{"code":404,"message":"models/x is not found","status":"NOT_FOUND"}}"#,
            ErrorKind::ModelNotFound,
        ),
        (
            429,
            r#"{"error":{"code":429,"message":"Resource has been exhausted (e.g. check quota).","status":"RESOURCE_EXHAUSTED"}}"#,
            ErrorKind::RateLimited,
        ),
        (
            403,
            r#"{"error":{"code":403,"message":"denied","status":"PERMISSION_DENIED"}}"#,
            ErrorKind::PermissionDenied,
        ),
    ];
    for (status, body, kind) in cases {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&s)
            .await;
        let p = gemini::GeminiProvider::new(http(), "k".into(), Some(s.uri()));
        assert_eq!(run(&p, &req("x")).await.0.unwrap_err().kind, kind, "{body}");
    }

    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"models":[
            {"name":"models/gemini-x","displayName":"Gemini X","inputTokenLimit":1048576,"outputTokenLimit":65536,"supportedGenerationMethods":["generateContent","countTokens"]},
            {"name":"models/text-embedding-004","displayName":"Embedding","supportedGenerationMethods":["embedContent"]}
        ]})))
        .mount(&s)
        .await;
    let p = gemini::GeminiProvider::new(http(), "k".into(), Some(s.uri()));
    let m = p.list_models().await.unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(
        (m[0].id.as_str(), m[0].max_output_tokens),
        ("gemini-x", Some(65536))
    );
}

#[tokio::test]
async fn gemini_safety_block() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse(&[(
            "",
            r#"{"candidates":[{"content":{"parts":[],"role":"model"},"finishReason":"SAFETY"}]}"#,
        )]))
        .mount(&s)
        .await;
    let p = gemini::GeminiProvider::new(http(), "k".into(), Some(s.uri()));
    assert_eq!(
        run(&p, &req("x")).await.0.unwrap().stop,
        StopReason::Refusal
    );
}

// ---------------------------------------------------------------- OpenAI-compatible

#[tokio::test]
async fn compatible_stream_with_system_and_usage() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(serde_json::json!({
            "model":"llama3","stream":true,
            "messages":[{"role":"system","content":"Be brief."},{"role":"user","content":"Hi"},{"role":"assistant","content":"Hello!"},{"role":"user","content":"Say ok"}]
        })))
        .respond_with(sse(&[
            ("", r#"{"id":"c1","model":"llama3","choices":[{"index":0,"delta":{"role":"assistant","content":"o"}}]}"#),
            ("", r#"{"id":"c1","model":"llama3","choices":[{"index":0,"delta":{"content":"k"},"finish_reason":"stop"}]}"#),
            ("", r#"{"id":"c1","model":"llama3","choices":[],"usage":{"prompt_tokens":4,"completion_tokens":2,"cost":0.00001}}"#),
            ("", "[DONE]"),
        ]))
        .mount(&s)
        .await;
    let p =
        openai_compatible::OpenAiCompatibleProvider::new(http(), format!("{}/v1", s.uri()), None);
    let (out, text, _) = run(&p, &req("llama3")).await;
    let out = out.unwrap();
    assert_eq!(text, "ok");
    assert_eq!(out.usage.reported_cost_usd, Some(0.00001));
}

#[tokio::test]
async fn build_provider_requires_configuration() {
    let cfg = |kind, auth, credential| ProviderConfig {
        kind,
        auth,
        base_url: None,
        credential,
        cli_path: None,
    };
    let e = build_provider(
        cfg(
            ProviderKind::Anthropic,
            AuthMethod::ApiKey,
            Credential::None,
        ),
        http(),
    )
    .err()
    .unwrap();
    assert_eq!(e.kind, ErrorKind::NotConfigured);
    let e = build_provider(
        cfg(
            ProviderKind::OpenaiCompatible,
            AuthMethod::None,
            Credential::None,
        ),
        http(),
    )
    .err()
    .unwrap();
    assert_eq!(e.kind, ErrorKind::NotConfigured);
    let e = build_provider(
        cfg(
            ProviderKind::Openai,
            AuthMethod::ChatgptOauth,
            Credential::None,
        ),
        http(),
    )
    .err()
    .unwrap();
    assert_eq!(e.kind, ErrorKind::Auth);
    assert!(
        build_provider(
            cfg(
                ProviderKind::Gemini,
                AuthMethod::ApiKey,
                Credential::ApiKey("k".into())
            ),
            http()
        )
        .is_ok()
    );
}
