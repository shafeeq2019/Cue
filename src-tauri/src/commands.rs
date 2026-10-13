//! Tauri commands invoked by the UI. Secrets go *in* (API key entry) but
//! never come back out: no command returns a credential.

use std::sync::Arc;

use cue_core::chatgpt_oauth::{ChatGptAuth, ChatGptCredentials, Reauth, USAGE_URL};
use cue_core::providers::claude_code::{ClaudeCodeProvider, ClaudeCodeStatus};
use cue_core::secrets::{api_key_entry, oauth_entry};
use cue_core::settings::{AccountInfo, Connection, Mode, ModelRef, Settings, Theme};
use cue_core::types::{AuthMethod, ChatMessage, ChatRequest, ModelInfo, ProviderKind, Role};
use cue_core::{CueError, ErrorKind};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;

use crate::chat::{self, ChatEvent, SendArgs};
use crate::history::{Conversation, StoredMessage, title_from};
use crate::hotkey::{self, HotkeyCheck};
use crate::state::{AppState, DiagEntry, UsageTotals, now_secs};
use crate::window;

type S<'a> = State<'a, Arc<AppState>>;
type CmdResult<T> = Result<T, CueError>;

fn lock_err() -> CueError {
    CueError::new(ErrorKind::Unknown, "Internal state is unavailable.")
}

// ------------------------------------------------------------------ state

#[derive(Serialize)]
pub struct AppInfo {
    pub version: &'static str,
    pub platform: &'static str,
    pub default_hotkey: &'static str,
    pub hotkey_display: String,
    pub secret_store_error: Option<String>,
    pub hotkey_error: Option<String>,
    pub settings: Settings,
    pub session_usage: std::collections::HashMap<String, UsageTotals>,
    pub modes: ResolvedModes,
}

#[derive(Serialize)]
pub struct ResolvedModes {
    pub fast: Option<ModelRef>,
    pub reasoning: Option<ModelRef>,
    pub writing: Option<ModelRef>,
    pub default: Option<ModelRef>,
}

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

#[tauri::command]
pub fn get_app_info(state: S) -> CmdResult<AppInfo> {
    let settings = state.settings.lock().map_err(|_| lock_err())?.clone();
    let modes = ResolvedModes {
        fast: settings.resolve_mode(Mode::Fast),
        reasoning: settings.resolve_mode(Mode::Reasoning),
        writing: settings.resolve_mode(Mode::Writing),
        default: settings.effective_default(),
    };
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: platform(),
        default_hotkey: cue_core::settings::default_hotkey(),
        hotkey_display: hotkey::display(&settings.hotkey),
        secret_store_error: state.secret_store_error.clone(),
        hotkey_error: state.hotkey_error.lock().map_err(|_| lock_err())?.clone(),
        session_usage: state.session_usage.lock().map_err(|_| lock_err())?.clone(),
        settings,
        modes,
    })
}

// ------------------------------------------------------------ connections

#[derive(Deserialize)]
pub struct NewConnection {
    pub kind: ProviderKind,
    pub auth: AuthMethod,
    pub label: String,
    pub base_url: Option<String>,
    pub cli_path: Option<String>,
    /// Write-only. Stored in the OS credential store, never echoed back.
    pub api_key: Option<String>,
}

fn validate_new(n: &NewConnection) -> CmdResult<()> {
    let ok = matches!(
        (n.kind, n.auth),
        (ProviderKind::Anthropic, AuthMethod::ApiKey)
            | (ProviderKind::Openai, AuthMethod::ApiKey)
            | (ProviderKind::Openai, AuthMethod::ChatgptOauth)
            | (ProviderKind::Gemini, AuthMethod::ApiKey)
            | (ProviderKind::OpenaiCompatible, AuthMethod::ApiKey)
            | (ProviderKind::OpenaiCompatible, AuthMethod::None)
            | (ProviderKind::ClaudeCode, AuthMethod::LocalCli)
    );
    if !ok {
        return Err(CueError::new(ErrorKind::InvalidRequest, "This sign-in method is not supported for this provider."));
    }
    if n.auth == AuthMethod::ApiKey && n.kind != ProviderKind::OpenaiCompatible && n.api_key.as_deref().is_none_or(|k| k.trim().is_empty()) {
        return Err(CueError::new(ErrorKind::InvalidRequest, "Enter an API key."));
    }
    if let Some(u) = n.base_url.as_deref().filter(|u| !u.trim().is_empty()) {
        let parsed = url::Url::parse(u.trim()).map_err(|_| CueError::new(ErrorKind::InvalidRequest, "The server URL is not valid."))?;
        let local = matches!(parsed.host_str(), Some("localhost") | Some("127.0.0.1") | Some("[::1]"));
        if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
            return Err(CueError::new(ErrorKind::InvalidRequest, "Use https:// (plain http is only allowed for localhost)."));
        }
    } else if n.kind == ProviderKind::OpenaiCompatible {
        return Err(CueError::new(ErrorKind::InvalidRequest, "Enter the server's base URL, e.g. http://localhost:11434/v1."));
    }
    Ok(())
}

#[tauri::command]
pub async fn add_connection(state: S<'_>, conn: NewConnection) -> CmdResult<Connection> {
    validate_new(&conn)?;
    if conn.auth == AuthMethod::ChatgptOauth {
        return Err(CueError::new(ErrorKind::InvalidRequest, "Use “Continue with ChatGPT” to add this connection."));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut has_secret = false;
    if let Some(key) = conn.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        state.secrets.set(&api_key_entry(&id), key)?;
        has_secret = true;
    }
    let c = Connection {
        id: id.clone(),
        kind: conn.kind,
        auth: conn.auth,
        label: if conn.label.trim().is_empty() { conn.kind.display_name().to_string() } else { conn.label.trim().to_string() },
        base_url: conn.base_url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
        cli_path: conn.cli_path.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()),
        enabled: true,
        account: None,
        oauth_client_id: None,
        models: vec![],
        status: None,
        has_secret,
    };
    state.settings.lock().map_err(|_| lock_err())?.connections.push(c.clone());
    state.save_settings()?;
    state.log("info", format!("Added connection {} ({:?})", c.label, c.kind));
    // Test immediately so the user sees real status and models.
    test_connection(state, id).await
}

#[derive(Deserialize)]
pub struct ConnectionPatch {
    pub id: String,
    pub label: Option<String>,
    pub base_url: Option<String>,
    pub cli_path: Option<String>,
    pub enabled: Option<bool>,
    /// Replace the stored API key (write-only).
    pub api_key: Option<String>,
}

#[tauri::command]
pub fn update_connection(state: S, patch: ConnectionPatch) -> CmdResult<Connection> {
    if let Some(k) = patch.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        state.secrets.set(&api_key_entry(&patch.id), k)?;
    }
    let updated = {
        let mut s = state.settings.lock().map_err(|_| lock_err())?;
        let c = s.connection_mut(&patch.id).ok_or_else(|| CueError::new(ErrorKind::InvalidRequest, "Connection not found."))?;
        if let Some(l) = patch.label.filter(|l| !l.trim().is_empty()) {
            c.label = l.trim().into();
        }
        if let Some(u) = patch.base_url {
            c.base_url = Some(u.trim().to_string()).filter(|u| !u.is_empty());
        }
        if let Some(p) = patch.cli_path {
            c.cli_path = Some(p.trim().to_string()).filter(|u| !u.is_empty());
        }
        if let Some(e) = patch.enabled {
            c.enabled = e;
        }
        if patch.api_key.is_some_and(|k| !k.trim().is_empty()) {
            c.has_secret = true;
            c.status = None;
        }
        c.clone()
    };
    state.save_settings()?;
    Ok(updated)
}

#[tauri::command]
pub async fn remove_connection(state: S<'_>, id: String) -> CmdResult<RemoveResult> {
    let conn = state.settings.lock().map_err(|_| lock_err())?.connection(&id).cloned();
    let mut revoked = true;
    if let Some(c) = &conn {
        if c.auth == AuthMethod::ChatgptOauth {
            if let Ok(Some(raw)) = state.secrets.get(&oauth_entry(&id)) {
                if let Ok(creds) = serde_json::from_str::<ChatGptCredentials>(&raw) {
                    revoked = ChatGptAuth::new(state.http.clone()).revoke(&creds).await;
                }
            }
        }
    }
    state.secrets.delete(&api_key_entry(&id))?;
    state.secrets.delete(&oauth_entry(&id))?;
    {
        let mut s = state.settings.lock().map_err(|_| lock_err())?;
        s.connections.retain(|c| c.id != id);
        s.acknowledged_connections.retain(|c| c != &id);
        let clear = |r: &mut Option<ModelRef>| {
            if r.as_ref().is_some_and(|r| r.connection_id == id) {
                *r = None;
            }
        };
        clear(&mut s.default_model);
        clear(&mut s.modes.fast);
        clear(&mut s.modes.reasoning);
        clear(&mut s.modes.writing);
    }
    state.save_settings()?;
    state.log("info", "Removed a connection and its stored credentials");
    Ok(RemoveResult { remote_revocation_confirmed: revoked })
}

#[derive(Serialize)]
pub struct RemoveResult {
    pub remote_revocation_confirmed: bool,
}

/// Test a connection by fetching its live model list.
#[tauri::command]
pub async fn test_connection(state: S<'_>, id: String) -> CmdResult<Connection> {
    let conn = state
        .settings
        .lock()
        .map_err(|_| lock_err())?
        .connection(&id)
        .cloned()
        .ok_or_else(|| CueError::new(ErrorKind::InvalidRequest, "Connection not found."))?;

    let result: Result<(Vec<ModelInfo>, Option<AccountInfo>), CueError> = async {
        let account = if conn.kind == ProviderKind::ClaudeCode {
            let st = ClaudeCodeProvider::new(conn.cli_path.clone()).status().await?;
            Some(AccountInfo { email: None, subject: None, plan: st.subscription.clone(), auth_detail: st.auth_method.clone() })
        } else {
            conn.account.clone()
        };
        let provider = chat::provider_for(&state, &conn, false).await?;
        Ok((provider.list_models().await?, account))
    }
    .await;

    let updated = {
        let mut s = state.settings.lock().map_err(|_| lock_err())?;
        let c = s.connection_mut(&id).ok_or_else(|| CueError::new(ErrorKind::InvalidRequest, "Connection not found."))?;
        match &result {
            Ok((models, account)) => {
                c.models = models.clone();
                c.account = account.clone();
                c.status = Some(cue_core::settings::ConnectionStatus {
                    state: cue_core::settings::ConnectionState::Ok,
                    checked_at: now_secs(),
                    error: if models.is_empty() {
                        Some(CueError::new(ErrorKind::ModelNotFound, "Connected, but no chat models are available for this account."))
                    } else {
                        None
                    },
                });
            }
            Err(e) => {
                c.status = Some(cue_core::settings::ConnectionStatus {
                    state: cue_core::settings::ConnectionState::Error,
                    checked_at: now_secs(),
                    error: Some(e.clone()),
                });
            }
        }
        c.clone()
    };
    state.save_settings()?;
    if let Err(e) = &result {
        state.log("warn", format!("Test of {} failed: {:?} {}", conn.label, e.kind, e.detail.clone().unwrap_or_default()));
    }
    Ok(updated)
}

#[tauri::command]
pub async fn claude_code_status(cli_path: Option<String>) -> CmdResult<ClaudeCodeStatus> {
    ClaudeCodeProvider::new(cli_path).status().await
}

/// Official "Sign in with ChatGPT" (system browser, PKCE, loopback).
/// With `connection_id`, re-authorises that existing registration.
#[tauri::command]
pub async fn chatgpt_sign_in(app: AppHandle, state: S<'_>, connection_id: Option<String>, label: Option<String>) -> CmdResult<Connection> {
    let (host_id, existing) = {
        let s = state.settings.lock().map_err(|_| lock_err())?;
        (s.host_id.clone(), connection_id.as_deref().and_then(|id| s.connection(id).cloned()))
    };
    let reauth = existing.as_ref().and_then(|c| {
        let client_id = c.oauth_client_id.clone()?;
        let stored: Option<ChatGptCredentials> =
            state.secrets.get(&oauth_entry(&c.id)).ok().flatten().and_then(|r| serde_json::from_str(&r).ok());
        Some(Reauth {
            client_id,
            id_token_hint: stored.as_ref().map(|s| s.id_token.clone()),
            login_hint: c.account.as_ref().and_then(|a| a.email.clone()),
            expected_subject: c.account.as_ref().and_then(|a| a.subject.clone()),
        })
    });
    let cancel = CancellationToken::new();
    if let Some(old) = state.sign_in.lock().map_err(|_| lock_err())?.replace(cancel.clone()) {
        old.cancel();
    }
    let auth = ChatGptAuth::new(state.http.clone());
    let opener = app.clone();
    let creds = auth
        .sign_in(&host_id, reauth, move |url| opener.opener().open_url(url, None::<&str>).map_err(|e| e.to_string()), &cancel)
        .await;
    state.sign_in.lock().map_err(|_| lock_err())?.take();
    window::show(&app);
    let creds = creds?;

    let id = existing.as_ref().map(|c| c.id.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    state.secrets.set(&oauth_entry(&id), &serde_json::to_string(&creds).expect("serialize"))?;
    {
        let mut s = state.settings.lock().map_err(|_| lock_err())?;
        let account = AccountInfo { email: creds.email.clone(), subject: Some(creds.subject.clone()), plan: None, auth_detail: Some("chatgpt_plan".into()) };
        match s.connection_mut(&id) {
            Some(c) => {
                c.account = Some(account);
                c.oauth_client_id = Some(creds.client_id.clone());
                c.has_secret = true;
            }
            None => s.connections.push(Connection {
                id: id.clone(),
                kind: ProviderKind::Openai,
                auth: AuthMethod::ChatgptOauth,
                label: label.filter(|l| !l.trim().is_empty()).unwrap_or_else(|| {
                    format!("ChatGPT ({})", creds.email.clone().unwrap_or_else(|| "account".into()))
                }),
                base_url: None,
                cli_path: None,
                enabled: true,
                account: Some(account),
                oauth_client_id: Some(creds.client_id.clone()),
                models: vec![],
                status: None,
                has_secret: true,
            }),
        }
    }
    state.save_settings()?;
    state.log("info", "ChatGPT sign-in completed");
    test_connection(state, id).await
}

#[tauri::command]
pub fn chatgpt_cancel_sign_in(state: S) {
    if let Ok(mut g) = state.sign_in.lock() {
        if let Some(c) = g.take() {
            c.cancel();
        }
    }
}

// ----------------------------------------------------------------- models

#[tauri::command]
pub fn set_default_model(state: S, model: Option<ModelRef>) -> CmdResult<()> {
    if let Some(m) = &model {
        if !state.settings.lock().map_err(|_| lock_err())?.is_available(m) {
            return Err(CueError::from_kind(ErrorKind::ModelNotFound, "this connection"));
        }
    }
    state.settings.lock().map_err(|_| lock_err())?.default_model = model;
    state.save_settings()
}

#[tauri::command]
pub fn set_mode_model(state: S, mode: Mode, model: Option<ModelRef>) -> CmdResult<()> {
    let mut s = state.settings.lock().map_err(|_| lock_err())?;
    if let Some(m) = &model {
        if !s.is_available(m) {
            return Err(CueError::from_kind(ErrorKind::ModelNotFound, "this connection"));
        }
    }
    match mode {
        Mode::Fast => s.modes.fast = model,
        Mode::Reasoning => s.modes.reasoning = model,
        Mode::Writing => s.modes.writing = model,
    }
    drop(s);
    state.save_settings()
}

// ------------------------------------------------------------ preferences

#[derive(Deserialize)]
pub struct Prefs {
    pub theme: Option<Theme>,
    pub hide_on_blur: Option<bool>,
    pub persist_history: Option<bool>,
    pub system_prompt: Option<String>,
}

#[tauri::command]
pub fn update_prefs(state: S, prefs: Prefs) -> CmdResult<()> {
    {
        let mut s = state.settings.lock().map_err(|_| lock_err())?;
        if let Some(t) = prefs.theme {
            s.theme = t;
        }
        if let Some(h) = prefs.hide_on_blur {
            s.hide_on_blur = h;
        }
        if let Some(p) = prefs.persist_history {
            s.persist_history = p;
        }
        if let Some(sp) = prefs.system_prompt {
            s.system_prompt = Some(sp).filter(|x| !x.trim().is_empty());
        }
    }
    state.save_settings()?;
    state.save_history();
    Ok(())
}

#[tauri::command]
pub fn check_hotkey(accelerator: String) -> HotkeyCheck {
    hotkey::check(&accelerator)
}

#[tauri::command]
pub fn set_hotkey(app: AppHandle, state: S, accelerator: String) -> CmdResult<HotkeyCheck> {
    let old = state.settings.lock().map_err(|_| lock_err())?.hotkey.clone();
    if old == accelerator {
        return Ok(hotkey::check(&accelerator));
    }
    let res = hotkey::apply(&app, Some(&old), &accelerator);
    if res.ok {
        *state.hotkey_error.lock().map_err(|_| lock_err())? = None;
        state.settings.lock().map_err(|_| lock_err())?.hotkey = accelerator;
        state.save_settings()?;
    }
    Ok(res)
}

#[tauri::command]
pub fn acknowledge_connection(state: S, id: String) -> CmdResult<()> {
    {
        let mut s = state.settings.lock().map_err(|_| lock_err())?;
        if !s.acknowledged_connections.contains(&id) {
            s.acknowledged_connections.push(id);
        }
    }
    state.save_settings()
}

#[tauri::command]
pub fn set_launch_at_login(app: AppHandle, enabled: bool) -> CmdResult<bool> {
    use tauri_plugin_autostart::ManagerExt;
    let m = app.autolaunch();
    let r = if enabled { m.enable() } else { m.disable() };
    r.map_err(|e| CueError::new(ErrorKind::Unknown, "Could not change the login item.").with_detail(e.to_string()))?;
    m.is_enabled().map_err(|e| CueError::new(ErrorKind::Unknown, "Could not read the login item.").with_detail(e.to_string()))
}

#[tauri::command]
pub fn get_launch_at_login(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

// ------------------------------------------------------------------- chat

#[derive(Deserialize)]
pub struct SendRequest {
    pub conversation_id: Option<String>,
    pub model: ModelRef,
    pub text: String,
}

#[derive(Serialize)]
pub struct SendResponse {
    pub request_id: String,
    pub conversation_id: String,
    pub user_message: StoredMessage,
    pub assistant_message: StoredMessage,
}

#[tauri::command]
pub fn send_message(app: AppHandle, state: S, req: SendRequest, channel: Channel<ChatEvent>) -> CmdResult<SendResponse> {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err(CueError::new(ErrorKind::InvalidRequest, "Type a message first."));
    }
    let (conn, system, model_info) = {
        let s = state.settings.lock().map_err(|_| lock_err())?;
        let conn = s
            .connection(&req.model.connection_id)
            .cloned()
            .ok_or_else(|| CueError::not_configured("This connection no longer exists.").with_hint("Pick another model."))?;
        if !conn.enabled {
            return Err(CueError::not_configured(format!("“{}” is disabled.", conn.label)).with_hint("Enable it under AI Providers."));
        }
        let model_info = conn.models.iter().find(|m| m.id == req.model.model_id).cloned();
        if model_info.is_none() {
            return Err(CueError::from_kind(ErrorKind::ModelNotFound, conn.kind.display_name()));
        }
        (conn, s.system_prompt.clone(), model_info)
    };

    let attribution = chat::attribution(&conn, &req.model.model_id);
    let user = StoredMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: Role::User,
        content: text.clone(),
        created_at: now_secs(),
        attribution: None,
        usage: None,
        stop: None,
        error: None,
        pending: false,
    };
    let assistant = chat::new_pending_message(attribution.clone());

    let (conversation_id, context) = {
        let mut h = state.history.lock().map_err(|_| lock_err())?;
        let id = match req.conversation_id.as_deref().filter(|id| h.get(id).is_some()) {
            Some(id) => id.to_string(),
            None => {
                let mut c = Conversation::new();
                c.title = title_from(&text);
                let id = c.id.clone();
                h.conversations.insert(0, c);
                id
            }
        };
        let conv = h.get_mut(&id).expect("conversation exists");
        conv.messages.push(user.clone());
        let context: Vec<ChatMessage> = conv.context();
        conv.messages.push(assistant.clone());
        conv.last_model = Some(req.model.clone());
        conv.updated_at = now_secs();
        (id, context)
    };

    let max_output_tokens = model_info.and_then(|m| m.max_output_tokens).map(|m| m.min(16_000) as u32);
    let request_id = uuid::Uuid::new_v4().to_string();
    let _ = channel.send(ChatEvent::Started {
        request_id: request_id.clone(),
        conversation_id: conversation_id.clone(),
        message_id: assistant.id.clone(),
        attribution,
    });
    chat::spawn_chat(
        app,
        SendArgs {
            conversation_id: conversation_id.clone(),
            request_id: request_id.clone(),
            message_id: assistant.id.clone(),
            model: req.model.clone(),
            request: ChatRequest { model: req.model.model_id.clone(), messages: context, system, max_output_tokens },
            conn,
        },
        channel,
    );
    Ok(SendResponse { request_id, conversation_id, user_message: user, assistant_message: assistant })
}

#[tauri::command]
pub fn cancel_message(state: S, request_id: String) {
    if let Some(c) = state.inflight.lock().ok().and_then(|m| m.get(&request_id).cloned()) {
        c.cancel();
    }
}

#[derive(Serialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    pub message_count: usize,
}

#[tauri::command]
pub fn list_conversations(state: S) -> CmdResult<Vec<ConversationSummary>> {
    let h = state.history.lock().map_err(|_| lock_err())?;
    let mut list: Vec<ConversationSummary> = h
        .conversations
        .iter()
        .filter(|c| !c.messages.is_empty())
        .map(|c| ConversationSummary { id: c.id.clone(), title: c.title.clone(), updated_at: c.updated_at, message_count: c.messages.len() })
        .collect();
    list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(list)
}

#[tauri::command]
pub fn get_conversation(state: S, id: String) -> CmdResult<Option<Conversation>> {
    Ok(state.history.lock().map_err(|_| lock_err())?.get(&id).cloned())
}

#[tauri::command]
pub fn delete_conversation(state: S, id: String) -> CmdResult<()> {
    state.history.lock().map_err(|_| lock_err())?.conversations.retain(|c| c.id != id);
    state.save_history();
    Ok(())
}

#[tauri::command]
pub fn clear_history(state: S) -> CmdResult<()> {
    state.history.lock().map_err(|_| lock_err())?.conversations.clear();
    // Remove the file regardless of the persistence setting.
    let _ = std::fs::remove_file(&state.history_path);
    state.save_history();
    state.log("info", "Chat history cleared");
    Ok(())
}

// ------------------------------------------------------------------- misc

#[tauri::command]
pub fn copy_text(app: AppHandle, text: String) -> CmdResult<()> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| CueError::new(ErrorKind::Unknown, "Could not copy to the clipboard.").with_detail(e.to_string()))
}

#[tauri::command]
pub fn hide_window(app: AppHandle) {
    window::hide(&app);
}

/// Only well-known provider pages may be opened from the UI.
#[tauri::command]
pub fn open_external(app: AppHandle, url: String) -> CmdResult<()> {
    const ALLOWED: [&str; 9] = [
        "https://platform.claude.com",
        "https://platform.openai.com",
        "https://aistudio.google.com",
        "https://code.claude.com",
        "https://developers.openai.com",
        "https://ai.google.dev",
        "https://ollama.com",
        "https://openrouter.ai",
        USAGE_URL,
    ];
    if !ALLOWED.iter().any(|a| url == *a || url.starts_with(&format!("{a}/"))) {
        return Err(CueError::new(ErrorKind::InvalidRequest, "This link is not allowed."));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| CueError::new(ErrorKind::Unknown, "Could not open the browser.").with_detail(e.to_string()))
}

#[tauri::command]
pub fn get_diagnostics(state: S) -> Vec<DiagEntry> {
    state.diagnostics.lock().map(|d| d.iter().cloned().collect()).unwrap_or_default()
}

#[tauri::command]
pub fn clear_diagnostics(state: S) {
    if let Ok(mut d) = state.diagnostics.lock() {
        d.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(kind: ProviderKind, auth: AuthMethod, key: Option<&str>, url: Option<&str>) -> NewConnection {
        NewConnection { kind, auth, label: "x".into(), base_url: url.map(Into::into), cli_path: None, api_key: key.map(Into::into) }
    }

    #[test]
    fn connection_validation() {
        assert!(validate_new(&n(ProviderKind::Anthropic, AuthMethod::ApiKey, Some("k"), None)).is_ok());
        assert!(validate_new(&n(ProviderKind::Anthropic, AuthMethod::ApiKey, Some("  "), None)).is_err());
        // Claude subscription OAuth is deliberately not offered.
        assert!(validate_new(&n(ProviderKind::Anthropic, AuthMethod::ChatgptOauth, None, None)).is_err());
        assert!(validate_new(&n(ProviderKind::Gemini, AuthMethod::LocalCli, None, None)).is_err());
        assert!(validate_new(&n(ProviderKind::OpenaiCompatible, AuthMethod::None, None, Some("http://localhost:11434/v1"))).is_ok());
        assert!(validate_new(&n(ProviderKind::OpenaiCompatible, AuthMethod::None, None, Some("http://example.com/v1"))).is_err());
        assert!(validate_new(&n(ProviderKind::OpenaiCompatible, AuthMethod::ApiKey, Some("k"), Some("https://openrouter.ai/api/v1"))).is_ok());
        assert!(validate_new(&n(ProviderKind::OpenaiCompatible, AuthMethod::None, None, None)).is_err());
        assert!(validate_new(&n(ProviderKind::ClaudeCode, AuthMethod::LocalCli, None, None)).is_ok());
    }
}
