//! Shared application state.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cue_core::secrets::SecretStore;
use cue_core::settings::Settings;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::history::History;

pub const KEYRING_SERVICE: &str = "dev.cue.desktop";

pub struct AppState {
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub history: Mutex<History>,
    pub history_path: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    /// Error from initialising the OS credential store, if any.
    pub secret_store_error: Option<String>,
    pub http: cue_core::reqwest::Client,
    /// Running chat requests, for cancellation.
    pub inflight: Mutex<HashMap<String, CancellationToken>>,
    pub sign_in: Mutex<Option<CancellationToken>>,
    /// Serialises OAuth token refreshes so rotating refresh tokens never race.
    pub refresh_lock: tokio::sync::Mutex<()>,
    pub diagnostics: Mutex<VecDeque<DiagEntry>>,
    /// Token usage in this app session, per connection id.
    pub session_usage: Mutex<HashMap<String, UsageTotals>>,
    /// Work directory for Claude Code runs.
    pub work_dir: PathBuf,
    /// Set when the global hotkey could not be registered at startup.
    pub hotkey_error: Mutex<Option<String>>,
    /// When the palette was last shown; blur events right after showing are
    /// ignored (window managers can emit a transient focus-out).
    pub shown_at: Mutex<Option<std::time::Instant>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagEntry {
    pub at: u64,
    pub level: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageTotals {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reported_cost_usd: f64,
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl AppState {
    pub fn log(&self, level: &'static str, message: impl AsRef<str>) {
        let message = cue_core::error::redact(message.as_ref());
        if let Ok(mut d) = self.diagnostics.lock() {
            if d.len() >= 300 {
                d.pop_front();
            }
            d.push_back(DiagEntry {
                at: now_secs(),
                level,
                message,
            });
        }
    }

    pub fn save_settings(&self) -> Result<(), cue_core::CueError> {
        let s = self.settings.lock().expect("settings lock").clone();
        s.save(&self.settings_path)
    }

    /// Persist history only when the user opted in; otherwise make sure no
    /// history file is left on disk.
    pub fn save_history(&self) {
        let persist = self
            .settings
            .lock()
            .map(|s| s.persist_history)
            .unwrap_or(false);
        let res = if persist {
            self.history
                .lock()
                .expect("history lock")
                .save(&self.history_path)
        } else {
            match std::fs::remove_file(&self.history_path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        };
        if let Err(e) = res {
            self.log("error", format!("history: {e}"));
        }
    }
}
