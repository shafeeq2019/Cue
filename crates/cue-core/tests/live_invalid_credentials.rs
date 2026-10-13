//! Live checks against the real provider endpoints using deliberately invalid
//! credentials. They verify that real-world error responses are mapped to the
//! right error kind without needing (or spending on) a real account.
//!
//! Run with: `CUE_LIVE=1 cargo test -p cue-core --test live_invalid_credentials`

use cue_core::providers::{Credential, ProviderConfig, build_provider, http_client};
use cue_core::{AuthMethod, ChatMessage, ChatRequest, ErrorKind, ProviderKind};
use tokio_util::sync::CancellationToken;

fn live() -> bool {
    std::env::var("CUE_LIVE").is_ok_and(|v| v == "1")
}

async fn check(kind: ProviderKind, key: &str, model: &str) {
    let p = build_provider(
        ProviderConfig { kind, auth: AuthMethod::ApiKey, base_url: None, credential: Credential::ApiKey(key.into()), cli_path: None },
        http_client(),
    )
    .unwrap();
    let e = p.list_models().await.unwrap_err();
    println!("{kind:?} list_models -> {:?} {:?} {:?}", e.kind, e.status, e.detail);
    assert_eq!(e.kind, ErrorKind::Auth, "{kind:?}: {e:?}");

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let req = ChatRequest { model: model.into(), messages: vec![ChatMessage::user("hi")], system: None, max_output_tokens: Some(16) };
    let e = p.chat(&req, &tx, &CancellationToken::new()).await.unwrap_err();
    println!("{kind:?} chat -> {:?} {:?} {:?}", e.kind, e.status, e.detail);
    assert_eq!(e.kind, ErrorKind::Auth, "{kind:?}: {e:?}");
    assert!(!e.detail.unwrap_or_default().contains(key), "key must be redacted from details");
}

#[tokio::test]
async fn anthropic_invalid_key() {
    if live() {
        check(ProviderKind::Anthropic, "sk-ant-api03-invalid-cue-live-test-0000000000000000", "claude-haiku-4-5").await;
    }
}

#[tokio::test]
async fn openai_invalid_key() {
    if live() {
        check(ProviderKind::Openai, "sk-proj-invalid-cue-live-test-000000000000000000", "gpt-4.1-mini").await;
    }
}

#[tokio::test]
async fn gemini_invalid_key() {
    if live() {
        check(ProviderKind::Gemini, "AIzaSy-invalid-cue-live-test-00000000000", "gemini-2.5-flash").await;
    }
}

#[tokio::test]
async fn openai_jwks_is_usable_for_id_token_verification() {
    if live() {
        let ep = cue_core::chatgpt_oauth::Endpoints::default();
        let set: jsonwebtoken::jwk::JwkSet = http_client().get(&ep.jwks).send().await.unwrap().json().await.unwrap();
        assert!(!set.keys.is_empty());
        for k in &set.keys {
            jsonwebtoken::DecodingKey::from_jwk(k).expect("every published key converts");
        }
        println!("OpenAI JWKS: {} key(s)", set.keys.len());
    }
}
