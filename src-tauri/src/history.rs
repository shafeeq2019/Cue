//! Conversations. Kept in memory; written to disk only if the user enabled
//! "Keep history" (off by default).

use std::path::Path;

use cue_core::settings::ModelRef;
use cue_core::types::{Billing, ChatMessage, ProviderKind, Role, StopReason, Usage};
use cue_core::CueError;
use serde::{Deserialize, Serialize};

use crate::state::now_secs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attribution {
    pub connection_id: String,
    pub connection_label: String,
    pub provider: ProviderKind,
    pub model: String,
    /// Model id the provider reported as actually serving the request.
    pub served_model: Option<String>,
    pub billing: Billing,
    /// e.g. "claude.ai" when Claude Code runs on a Claude subscription.
    pub auth_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,
    pub role: Role,
    pub content: String,
    pub created_at: u64,
    #[serde(default)]
    pub attribution: Option<Attribution>,
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub stop: Option<StopReason>,
    #[serde(default)]
    pub error: Option<CueError>,
    /// Still streaming.
    #[serde(default)]
    pub pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub messages: Vec<StoredMessage>,
    #[serde(default)]
    pub last_model: Option<ModelRef>,
}

impl Conversation {
    pub fn new() -> Self {
        let t = now_secs();
        Self { id: uuid::Uuid::new_v4().to_string(), title: "New chat".into(), created_at: t, updated_at: t, messages: vec![], last_model: None }
    }

    /// Messages to send to a provider: completed turns only, so failed or
    /// cancelled answers never leak into context, and roles always alternate.
    pub fn context(&self) -> Vec<ChatMessage> {
        let mut out: Vec<ChatMessage> = Vec::new();
        for m in &self.messages {
            if m.pending || m.error.is_some() || m.content.trim().is_empty() {
                continue;
            }
            match (out.last_mut(), m.role) {
                // Merge consecutive user turns (e.g. after a failed answer).
                (Some(last), Role::User) if last.role == Role::User => {
                    // A retry repeats the same question; don't duplicate it.
                    if last.content != m.content {
                        last.content.push_str("\n\n");
                        last.content.push_str(&m.content);
                    }
                }
                (Some(last), Role::Assistant) if last.role == Role::Assistant => {}
                (None, Role::Assistant) => {}
                _ => out.push(ChatMessage { role: m.role, content: m.content.clone() }),
            }
        }
        out
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub conversations: Vec<Conversation>,
}

impl History {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let data = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, &data).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Conversation> {
        self.conversations.iter_mut().find(|c| c.id == id)
    }

    pub fn get(&self, id: &str) -> Option<&Conversation> {
        self.conversations.iter().find(|c| c.id == id)
    }
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(data)
}

pub fn title_from(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("New chat").trim();
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('…');
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str, error: bool) -> StoredMessage {
        StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role,
            content: content.into(),
            created_at: 0,
            attribution: None,
            usage: None,
            stop: None,
            error: error.then(|| CueError::new(cue_core::ErrorKind::Auth, "x")),
            pending: false,
        }
    }

    #[test]
    fn context_skips_failed_and_keeps_alternation() {
        let mut c = Conversation::new();
        c.messages = vec![
            msg(Role::User, "q1", false),
            msg(Role::Assistant, "", true),
            msg(Role::User, "q1 again", false),
            msg(Role::Assistant, "a1", false),
            msg(Role::User, "q2", false),
        ];
        let ctx = c.context();
        assert_eq!(ctx.len(), 3);
        assert_eq!(ctx[0].content, "q1\n\nq1 again");
        assert_eq!(ctx[1].role, Role::Assistant);
        assert_eq!(ctx[2].content, "q2");

        // Retrying the same question after a failure does not duplicate it.
        let mut c = Conversation::new();
        c.messages = vec![msg(Role::User, "q", false), msg(Role::Assistant, "", true), msg(Role::User, "q", false)];
        assert_eq!(c.context().len(), 1);
        assert_eq!(c.context()[0].content, "q");
    }

    #[test]
    fn titles() {
        assert_eq!(title_from("\n  Hello world \nmore"), "Hello world");
        assert!(title_from(&"x".repeat(100)).ends_with('…'));
    }
}
