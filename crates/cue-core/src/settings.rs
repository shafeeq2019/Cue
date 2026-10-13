//! Persistent, non-secret settings (JSON in the app config directory).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{CueError, CueResult, ErrorKind};
use crate::types::{AuthMethod, Billing, ModelInfo, ProviderKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    pub connection_id: String,
    pub model_id: String,
}

/// Account details shown in the UI (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AccountInfo {
    pub email: Option<String>,
    /// Stable account subject (OIDC `sub`).
    pub subject: Option<String>,
    /// e.g. "max" for Claude Code, as reported by the official tool.
    pub plan: Option<String>,
    /// e.g. "claude.ai" or "api_key" for Claude Code.
    pub auth_detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Untested,
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub state: ConnectionState,
    pub checked_at: u64,
    pub error: Option<CueError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub kind: ProviderKind,
    pub auth: AuthMethod,
    /// User-chosen label, e.g. "Work" or "Personal".
    pub label: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub cli_path: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub account: Option<AccountInfo>,
    /// Issued OAuth client id ("Sign in with ChatGPT" dynamic registration).
    /// Not a secret; kept to reuse the registration on re-authorisation.
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    /// Last successfully fetched model list.
    #[serde(default)]
    pub models: Vec<ModelInfo>,
    #[serde(default)]
    pub status: Option<ConnectionStatus>,
    /// Whether a secret is stored in the OS credential store (never the secret itself).
    #[serde(default)]
    pub has_secret: bool,
}

fn yes() -> bool {
    true
}

impl Connection {
    pub fn billing(&self) -> Billing {
        self.auth.billing()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Fast,
    Reasoning,
    Writing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModeMap {
    pub fast: Option<ModelRef>,
    pub reasoning: Option<ModelRef>,
    pub writing: Option<ModelRef>,
}

impl ModeMap {
    pub fn get(&self, m: Mode) -> Option<&ModelRef> {
        match m {
            Mode::Fast => self.fast.as_ref(),
            Mode::Reasoning => self.reasoning.as_ref(),
            Mode::Writing => self.writing.as_ref(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    /// Global shortcut in Tauri accelerator syntax, e.g. "Alt+Space".
    pub hotkey: String,
    pub theme: Theme,
    pub connections: Vec<Connection>,
    pub default_model: Option<ModelRef>,
    pub modes: ModeMap,
    /// Keep conversation history on disk. Off by default: conversations live
    /// in memory only and vanish when the window session is cleared or the app quits.
    pub persist_history: bool,
    pub hide_on_blur: bool,
    /// Connections for which the user acknowledged the "content is sent to an
    /// external provider" notice.
    pub acknowledged_connections: Vec<String>,
    /// Stable, opaque per-installation identifier (`ext_agent_host_id` for
    /// Sign in with ChatGPT). Not user-identifying.
    pub host_id: String,
    /// Optional system prompt applied to all chats.
    pub system_prompt: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            hotkey: default_hotkey().into(),
            theme: Theme::System,
            connections: vec![],
            default_model: None,
            modes: ModeMap::default(),
            persist_history: false,
            hide_on_blur: true,
            acknowledged_connections: vec![],
            host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            system_prompt: None,
        }
    }
}

/// Option+Space on macOS, Ctrl+Space elsewhere.
pub fn default_hotkey() -> &'static str {
    if cfg!(target_os = "macos") {
        "Alt+Space"
    } else {
        "Control+Space"
    }
}

impl Settings {
    pub fn load(path: &Path) -> CueResult<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| {
                CueError::new(ErrorKind::Unknown, "Settings file is corrupted.")
                    .with_hint("Cue will start with default settings; the old file was kept as settings.json.bak.")
                    .with_detail(e.to_string())
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(CueError::new(ErrorKind::Unknown, "Could not read settings.").with_detail(e.to_string())),
        }
    }

    /// Atomic write with owner-only permissions on Unix.
    pub fn save(&self, path: &Path) -> CueResult<()> {
        let io = |e: std::io::Error| {
            CueError::new(ErrorKind::Unknown, "Could not save settings.").with_detail(e.to_string())
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(self).expect("settings serialize");
        write_private(&tmp, &data).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    pub fn connection(&self, id: &str) -> Option<&Connection> {
        self.connections.iter().find(|c| c.id == id)
    }

    pub fn connection_mut(&mut self, id: &str) -> Option<&mut Connection> {
        self.connections.iter_mut().find(|c| c.id == id)
    }

    /// A model reference is usable only if its connection is enabled and the
    /// model is in that connection's last fetched (i.e. authorised) model list.
    pub fn is_available(&self, r: &ModelRef) -> bool {
        self.connection(&r.connection_id)
            .is_some_and(|c| c.enabled && c.models.iter().any(|m| m.id == r.model_id))
    }

    pub fn available_models(&self) -> impl Iterator<Item = (&Connection, &ModelInfo)> {
        self.connections
            .iter()
            .filter(|c| c.enabled)
            .flat_map(|c| c.models.iter().map(move |m| (c, m)))
    }

    /// The model to use when nothing else is selected.
    pub fn effective_default(&self) -> Option<ModelRef> {
        if let Some(d) = self.default_model.as_ref().filter(|d| self.is_available(d)) {
            return Some(d.clone());
        }
        self.available_models().next().map(|(c, m)| ModelRef {
            connection_id: c.id.clone(),
            model_id: m.id.clone(),
        })
    }

    /// Resolve a task mode to a model the user actually has. Uses the user's
    /// explicit choice when it is still available, otherwise suggests one from
    /// the fetched model lists by family name. Returns `None` when no suitable
    /// model is available – modes never point at models the user lacks.
    pub fn resolve_mode(&self, mode: Mode) -> Option<ModelRef> {
        if let Some(r) = self.modes.get(mode).filter(|r| self.is_available(r)) {
            return Some(r.clone());
        }
        let prefs: &[&str] = match mode {
            Mode::Fast => &[
                "haiku",
                "flash-lite",
                "flash",
                "mini",
                "nano",
                "small",
                "8b",
            ],
            Mode::Reasoning => &[
                "opus",
                "fable",
                "o3",
                "o4",
                "-pro",
                "reasoning",
                "thinking",
                "r1",
                "gpt-5",
            ],
            Mode::Writing => &["sonnet", "gpt-4.1", "gpt-5", "pro", "large", "opus"],
        };
        let default = self.effective_default();
        // Prefer the default connection, then any other.
        let mut candidates: Vec<(&Connection, &ModelInfo)> = self.available_models().collect();
        if let Some(d) = &default {
            candidates.sort_by_key(|(c, _)| c.id != d.connection_id);
        }
        for p in prefs {
            if let Some((c, m)) = candidates
                .iter()
                .find(|(_, m)| m.id.to_ascii_lowercase().contains(p))
            {
                return Some(ModelRef {
                    connection_id: c.id.clone(),
                    model_id: m.id.clone(),
                });
            }
        }
        None
    }
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(id: &str, kind: ProviderKind, models: &[&str]) -> Connection {
        Connection {
            id: id.into(),
            kind,
            auth: AuthMethod::ApiKey,
            label: id.into(),
            base_url: None,
            cli_path: None,
            enabled: true,
            account: None,
            oauth_client_id: None,
            models: models
                .iter()
                .map(|m| ModelInfo {
                    id: (*m).into(),
                    display_name: (*m).into(),
                    context_window: None,
                    max_output_tokens: None,
                })
                .collect(),
            status: None,
            has_secret: true,
        }
    }

    #[test]
    fn modes_only_use_available_models() {
        let mut s = Settings::default();
        assert_eq!(s.resolve_mode(Mode::Fast), None);
        s.connections.push(conn(
            "a",
            ProviderKind::Anthropic,
            &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
        ));
        s.connections
            .push(conn("g", ProviderKind::Gemini, &["gemini-2.5-flash"]));
        assert_eq!(
            s.resolve_mode(Mode::Fast).unwrap().model_id,
            "claude-haiku-5-5"
        );
        assert_eq!(
            s.resolve_mode(Mode::Reasoning).unwrap().model_id,
            "claude-opus-5-5"
        );
        assert_eq!(
            s.resolve_mode(Mode::Writing).unwrap().model_id,
            "claude-sonnet-5-5"
        );

        // Explicit choice wins while available…
        s.modes.fast = Some(ModelRef {
            connection_id: "g".into(),
            model_id: "gemini-2.5-flash".into(),
        });
        assert_eq!(s.resolve_mode(Mode::Fast).unwrap().connection_id, "g");
        // …and is ignored once the connection is disabled.
        s.connections[1].enabled = false;
        assert_eq!(
            s.resolve_mode(Mode::Fast).unwrap().model_id,
            "claude-haiku-5-5"
        );
        // Explicit choice of a model that vanished from the list is ignored.
        s.modes.reasoning = Some(ModelRef {
            connection_id: "a".into(),
            model_id: "gone".into(),
        });
        assert_eq!(
            s.resolve_mode(Mode::Reasoning).unwrap().model_id,
            "claude-opus-5-5"
        );
    }

    #[test]
    fn roundtrip_file() {
        let dir = std::env::temp_dir().join(format!("cue-settings-{}", uuid::Uuid::new_v4()));
        let path = dir.join("settings.json");
        let mut s = Settings::default();
        s.connections
            .push(conn("a", ProviderKind::Openai, &["gpt-4.1"]));
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("\"api_key\":") && !text.contains("token\":"),
            "no secret fields in settings"
        );
        assert_eq!(Settings::load(&path).unwrap(), s);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_fields_default() {
        let s: Settings = serde_json::from_str(r#"{"hotkey":"Alt+K"}"#).unwrap();
        assert_eq!(s.hotkey, "Alt+K");
        assert!(!s.persist_history);
    }
}
