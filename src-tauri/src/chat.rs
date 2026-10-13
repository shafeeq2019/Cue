//! Chat orchestration: resolve the connection's credential from the OS store,
//! build the provider, stream events to the UI and record the result.

use std::sync::Arc;
use std::time::Duration;

use cue_core::chatgpt_oauth::{ChatGptAuth, ChatGptCredentials};
use cue_core::providers::{Credential, ProviderConfig, build_provider};
use cue_core::secrets::{api_key_entry, oauth_entry};
use cue_core::settings::{Connection, ConnectionState, ConnectionStatus, ModelRef};
use cue_core::types::{AuthMethod, ChatRequest, ProviderKind, StopReason, StreamEvent, Usage};
use cue_core::{CueError, CueResult, ErrorKind, Provider};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::history::{Attribution, StoredMessage};
use crate::state::{AppState, now_secs};

/// Events delivered to the UI over a Tauri channel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatEvent {
    Started { request_id: String, conversation_id: String, message_id: String, attribution: Attribution },
    Delta { text: String },
    Model { model: String },
    Notice { message: String },
    Done { stop: StopReason, usage: Usage },
    Error { error: CueError },
}

/// Load the credential for a connection, refreshing ChatGPT tokens if needed.
pub async fn credential_for(state: &AppState, conn: &Connection, force_refresh: bool) -> CueResult<Credential> {
    match conn.auth {
        AuthMethod::ApiKey => match state.secrets.get(&api_key_entry(&conn.id))? {
            Some(k) if !k.is_empty() => Ok(Credential::ApiKey(k)),
            _ if conn.kind == ProviderKind::OpenaiCompatible => Ok(Credential::None),
            _ => Err(CueError::from_kind(ErrorKind::NotConfigured, conn.kind.display_name())
                .with_hint("No API key stored for this connection. Add one under AI Providers.")),
        },
        AuthMethod::ChatgptOauth => {
            let _guard = state.refresh_lock.lock().await;
            let raw = state.secrets.get(&oauth_entry(&conn.id))?.ok_or_else(|| {
                CueError::from_kind(ErrorKind::Auth, "ChatGPT").with_hint("Sign in with ChatGPT under AI Providers.")
            })?;
            let creds: ChatGptCredentials = serde_json::from_str(&raw).map_err(|_| {
                CueError::new(ErrorKind::SecretStore, "Stored ChatGPT credentials are unreadable.")
                    .with_hint("Sign in with ChatGPT again.")
            })?;
            if !creds.has_plan_scope() {
                return Err(CueError::new(ErrorKind::PermissionDenied, "ChatGPT plan usage was not granted for this account.")
                    .with_hint("Sign in again and approve plan usage."));
            }
            if force_refresh || creds.needs_refresh(Duration::from_secs(120)) {
                let auth = ChatGptAuth::new(state.http.clone());
                let next = auth.refresh(&creds).await?;
                let json = serde_json::to_string(&next).expect("serialize credentials");
                state.secrets.set(&oauth_entry(&conn.id), &json)?;
                state.log("info", format!("Refreshed ChatGPT session for connection {}", conn.label));
                return Ok(Credential::Bearer(next.access_token));
            }
            Ok(Credential::Bearer(creds.access_token))
        }
        AuthMethod::LocalCli | AuthMethod::None => Ok(Credential::None),
    }
}

pub async fn provider_for(state: &AppState, conn: &Connection, force_refresh: bool) -> CueResult<Box<dyn Provider>> {
    let credential = credential_for(state, conn, force_refresh).await?;
    if conn.kind == ProviderKind::ClaudeCode {
        return Ok(Box::new(
            cue_core::providers::claude_code::ClaudeCodeProvider::new(conn.cli_path.clone()).with_work_dir(state.work_dir.clone()),
        ));
    }
    build_provider(
        ProviderConfig {
            kind: conn.kind,
            auth: conn.auth,
            base_url: conn.base_url.clone(),
            credential,
            cli_path: conn.cli_path.clone(),
        },
        state.http.clone(),
    )
}

pub fn attribution(conn: &Connection, model: &str) -> Attribution {
    Attribution {
        connection_id: conn.id.clone(),
        connection_label: conn.label.clone(),
        provider: conn.kind,
        model: model.to_string(),
        served_model: None,
        billing: conn.billing(),
        auth_detail: conn.account.as_ref().and_then(|a| a.auth_detail.clone()),
    }
}

pub fn set_status(state: &AppState, conn_id: &str, result: Result<(), &CueError>) {
    if let Ok(mut s) = state.settings.lock() {
        if let Some(c) = s.connection_mut(conn_id) {
            c.status = Some(ConnectionStatus {
                state: if result.is_ok() { ConnectionState::Ok } else { ConnectionState::Error },
                checked_at: now_secs(),
                error: result.err().cloned(),
            });
        }
    }
}

pub struct SendArgs {
    pub conversation_id: String,
    pub request_id: String,
    pub message_id: String,
    pub model: ModelRef,
    pub conn: Connection,
    pub request: ChatRequest,
}

/// Run one streaming request in the background.
pub fn spawn_chat(app: AppHandle, args: SendArgs, channel: Channel<ChatEvent>) {
    let cancel = CancellationToken::new();
    {
        let state = app.state::<Arc<AppState>>();
        state.inflight.lock().expect("inflight").insert(args.request_id.clone(), cancel.clone());
    }
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        let result = run_chat(&state, &args, &channel, &cancel).await;
        state.inflight.lock().expect("inflight").remove(&args.request_id);

        match result {
            Ok(((stop, usage), served)) => {
                finish_message(&state, &args, None, Some(stop), usage.clone(), served);
                let _ = channel.send(ChatEvent::Done { stop, usage });
            }
            Err(e) => {
                if e.kind != ErrorKind::Cancelled {
                    state.log(
                        "error",
                        format!("{} / {}: {:?} {}", args.conn.label, args.model.model_id, e.kind, e.detail.clone().unwrap_or_default()),
                    );
                }
                finish_message(&state, &args, Some(e.clone()), None, Usage::default(), None);
                if matches!(e.kind, ErrorKind::Auth | ErrorKind::PermissionDenied | ErrorKind::NotConfigured) {
                    set_status(&state, &args.conn.id, Err(&e));
                    let _ = state.save_settings();
                }
                let _ = channel.send(ChatEvent::Error { error: e });
            }
        }
        state.save_history();
    });
}

type Outcome = ((StopReason, Usage), Option<String>);

async fn run_chat(state: &AppState, args: &SendArgs, channel: &Channel<ChatEvent>, cancel: &CancellationToken) -> CueResult<Outcome> {
    let mut force_refresh = false;
    loop {
        let provider = provider_for(state, &args.conn, force_refresh).await?;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
        let mut text = String::new();
        let mut served: Option<String> = None;
        let fut = provider.chat(&args.request, &tx, cancel);
        tokio::pin!(fut);
        let mut flush_timer = tokio::time::interval(Duration::from_millis(250));
        let result = loop {
            tokio::select! {
                r = &mut fut => {
                    // Drain remaining events.
                    while let Ok(ev) = rx.try_recv() {
                        forward(channel, ev, &mut text, &mut served);
                    }
                    break r;
                }
                Some(ev) = rx.recv() => forward(channel, ev, &mut text, &mut served),
                _ = flush_timer.tick() => update_partial(state, args, &text),
            }
        };
        match result {
            // One transparent retry with a refreshed token if a ChatGPT
            // request is rejected before any output arrived.
            Err(e) if e.kind == ErrorKind::Auth && args.conn.auth == AuthMethod::ChatgptOauth && !force_refresh && text.is_empty() => {
                force_refresh = true;
                continue;
            }
            Err(e) => {
                update_partial(state, args, &text);
                return Err(e);
            }
            Ok(o) => {
                update_partial(state, args, &text);
                if o.stop == StopReason::MaxTokens {
                    let _ = channel.send(ChatEvent::Notice { message: "The answer was cut off at the output token limit.".into() });
                }
                if o.stop == StopReason::Refusal {
                    let _ = channel.send(ChatEvent::Notice { message: "The model declined to answer this request.".into() });
                }
                record_usage(state, &args.conn.id, &o.usage);
                return Ok(((o.stop, o.usage), served));
            }
        }
    }
}

fn forward(channel: &Channel<ChatEvent>, ev: StreamEvent, text: &mut String, served: &mut Option<String>) {
    match ev {
        StreamEvent::Delta { text: t } => {
            text.push_str(&t);
            let _ = channel.send(ChatEvent::Delta { text: t });
        }
        StreamEvent::Model { model } => {
            *served = Some(model.clone());
            let _ = channel.send(ChatEvent::Model { model });
        }
        StreamEvent::Notice { message } => {
            let _ = channel.send(ChatEvent::Notice { message });
        }
        StreamEvent::Usage { .. } => {}
    }
}

fn update_partial(state: &AppState, args: &SendArgs, text: &str) {
    if let Ok(mut h) = state.history.lock() {
        if let Some(m) = h.get_mut(&args.conversation_id).and_then(|c| c.messages.iter_mut().find(|m| m.id == args.message_id)) {
            m.content = text.to_string();
        }
    }
}

fn finish_message(state: &AppState, args: &SendArgs, error: Option<CueError>, stop: Option<StopReason>, usage: Usage, served: Option<String>) {
    if let Ok(mut h) = state.history.lock() {
        if let Some(conv) = h.get_mut(&args.conversation_id) {
            conv.updated_at = now_secs();
            if let Some(m) = conv.messages.iter_mut().find(|m| m.id == args.message_id) {
                m.pending = false;
                m.error = error;
                m.stop = stop;
                m.usage = Some(usage);
                if let Some(a) = m.attribution.as_mut() {
                    a.served_model = served;
                }
            }
        }
    }
}

fn record_usage(state: &AppState, conn_id: &str, usage: &Usage) {
    if let Ok(mut u) = state.session_usage.lock() {
        let t = u.entry(conn_id.to_string()).or_default();
        t.requests += 1;
        t.input_tokens += usage.input_tokens.unwrap_or(0);
        t.output_tokens += usage.output_tokens.unwrap_or(0);
        t.reported_cost_usd += usage.reported_cost_usd.unwrap_or(0.0);
    }
}

pub fn new_pending_message(attribution: Attribution) -> StoredMessage {
    StoredMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: cue_core::types::Role::Assistant,
        content: String::new(),
        created_at: now_secs(),
        attribution: Some(attribution),
        usage: None,
        stop: None,
        error: None,
        pending: true,
    }
}
