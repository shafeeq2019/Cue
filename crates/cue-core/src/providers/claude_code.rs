//! Bridge to the user's locally installed, **unmodified** Claude Code CLI.
//!
//! Why this exists: Anthropic does not allow third-party apps to offer
//! Claude.ai sign-in or to use Free/Pro/Max credentials themselves. Anthropic's
//! Claude Code legal page does, however, state that this policy does not
//! prevent an end user from signing in to the unmodified Claude Code binary
//! with their own subscription. Cue therefore never sees, reads, stores or
//! forwards any Claude credential: it launches `claude -p` as a child process,
//! the CLI authenticates itself (sign-in happens only in Anthropic's own flow
//! via `claude auth login`), and Cue reads the streamed output.
//! See docs/SUBSCRIPTIONS.md for the full assessment and its conditions.
//!
//! The connection is opt-in and runs Claude Code with all tools disabled,
//! no MCP servers and no session persistence, in an empty working directory.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{EventTx, Provider, send_delta};
use crate::error::{CueError, CueResult, ErrorKind};
use crate::types::{ChatOutcome, ChatRequest, ModelInfo, Role, StopReason, StreamEvent, Usage};

const NAME: &str = "Claude Code";

/// Model aliases documented for `claude --model`. The CLI resolves each alias
/// to the latest model of that family available to the signed-in account.
pub const MODEL_ALIASES: [(&str, &str); 4] = [
    ("default", "Claude Code default"),
    ("sonnet", "Sonnet (latest, via Claude Code)"),
    ("opus", "Opus (latest, via Claude Code)"),
    ("haiku", "Haiku (latest, via Claude Code)"),
];

pub struct ClaudeCodeProvider {
    cli_path: Option<String>,
    work_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClaudeCodeStatus {
    pub executable: String,
    pub version: Option<String>,
    pub logged_in: bool,
    /// `authMethod` as reported by `claude auth status`:
    /// none | claude.ai | oauth_token | api_key | api_key_helper | third_party
    pub auth_method: Option<String>,
    /// Subscription type, if the CLI reports it.
    pub subscription: Option<String>,
}

impl ClaudeCodeProvider {
    pub fn new(cli_path: Option<String>) -> Self {
        let work_dir = std::env::temp_dir().join("cue-claude-code");
        Self { cli_path, work_dir }
    }

    pub fn with_work_dir(mut self, dir: PathBuf) -> Self {
        self.work_dir = dir;
        self
    }

    fn executable(&self) -> CueResult<PathBuf> {
        if let Some(p) = self.cli_path.as_deref().filter(|p| !p.trim().is_empty()) {
            let p = PathBuf::from(p);
            return if p.is_file() {
                Ok(p)
            } else {
                Err(CueError::not_configured(format!(
                    "Claude Code was not found at {}.",
                    p.display()
                ))
                .with_hint("Fix the path in AI Providers or leave it empty for auto-detection."))
            };
        }
        find_claude().ok_or_else(|| {
            CueError::not_configured("Claude Code CLI not found.").with_hint(
                "Install Claude Code (https://code.claude.com), sign in by running `claude` once, or set its path in AI Providers.",
            )
        })
    }

    fn command(&self, exe: &Path) -> Command {
        let mut cmd = Command::new(exe);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    /// Run `claude --version` and `claude auth status`.
    pub async fn status(&self) -> CueResult<ClaudeCodeStatus> {
        let exe = self.executable()?;
        let version = self
            .command(&exe)
            .arg("--version")
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        let out = self
            .command(&exe)
            .args(["auth", "status", "--json"])
            .output()
            .await
            .map_err(|e| {
                CueError::new(ErrorKind::NotConfigured, "Could not run Claude Code.")
                    .with_detail(e.to_string())
            })?;
        let json: Option<Value> = serde_json::from_slice(&out.stdout).ok();
        let field = |k: &str| {
            json.as_ref()
                .and_then(|v| v[k].as_str())
                .map(str::to_string)
        };
        let auth_method = field("authMethod");
        let logged_in = json
            .as_ref()
            .and_then(|v| v["loggedIn"].as_bool())
            .unwrap_or(out.status.success() && auth_method.as_deref() != Some("none"));
        Ok(ClaudeCodeStatus {
            executable: exe.display().to_string(),
            version,
            logged_in,
            auth_method,
            subscription: field("subscriptionType").or_else(|| field("plan")),
        })
    }
}

/// Look for the `claude` executable. GUI apps on macOS do not inherit the
/// login shell's PATH, so common install locations are checked as well.
pub fn find_claude() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd"]
    } else {
        &["claude"]
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let home =
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if let Some(h) = &home {
        dirs.push(h.join(".local/bin"));
        dirs.push(h.join(".claude/local"));
        dirs.push(h.join(".npm-global/bin"));
        dirs.push(h.join(".bun/bin"));
    }
    if cfg!(windows) {
        if let Some(app) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(app).join("npm"));
        }
    } else {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| p.is_file())
}

/// Render the conversation as a single prompt for `claude -p`.
pub(crate) fn render_prompt(req: &ChatRequest) -> String {
    if req.messages.len() == 1 {
        return req.messages[0].content.clone();
    }
    let mut s = String::from("<conversation>\n");
    for m in &req.messages {
        let tag = match m.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        };
        s.push_str(&format!("<{tag}>\n{}\n</{tag}>\n", m.content));
    }
    s.push_str(
        "</conversation>\n\nReply to the last user message, taking the earlier turns into account.",
    );
    s
}

fn map_result_error(text: &str) -> CueError {
    let l = text.to_ascii_lowercase();
    let kind = if l.contains("not logged in")
        || l.contains("/login")
        || l.contains("invalid api key")
        || l.contains("authentication")
        || l.contains("oauth token")
    {
        ErrorKind::Auth
    } else if l.contains("usage limit")
        || l.contains("limit reached")
        || l.contains("out of extra usage")
    {
        ErrorKind::SubscriptionLimit
    } else if l.contains("rate limit") {
        ErrorKind::RateLimited
    } else if l.contains("credit balance") {
        ErrorKind::QuotaExceeded
    } else if l.contains("overloaded") {
        ErrorKind::Overloaded
    } else if l.contains("model")
        && (l.contains("not found") || l.contains("not available") || l.contains("invalid"))
    {
        ErrorKind::ModelNotFound
    } else if l.contains("prompt is too long") {
        ErrorKind::ContextTooLong
    } else {
        ErrorKind::Unknown
    };
    let mut e = CueError::from_kind(kind, NAME).with_detail(text.to_string());
    if kind == ErrorKind::Auth {
        e = e.with_hint("Open a terminal, run `claude` and sign in with /login (Anthropic's own sign-in). Cue never handles your Claude credentials.");
    }
    e
}

/// Parse one stdout line of `--output-format stream-json`.
/// Returns Some(outcome) when the final `result` message arrives.
pub(crate) fn handle_line(
    line: &str,
    tx: &EventTx,
    got_partial: &mut bool,
) -> Option<CueResult<ChatOutcome>> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "system" if v["subtype"] == "init" => {
            if let Some(m) = v["model"].as_str() {
                let _ = tx.send(StreamEvent::Model {
                    model: m.to_string(),
                });
            }
        }
        "stream_event" => {
            let ev = &v["event"];
            if ev["type"] == "content_block_delta" && ev["delta"]["type"] == "text_delta" {
                *got_partial = true;
                send_delta(tx, ev["delta"]["text"].as_str().unwrap_or(""));
            }
        }
        "assistant" if !*got_partial => {
            // Older CLI versions without partial messages: emit whole blocks.
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                if block["type"] == "text" {
                    send_delta(tx, block["text"].as_str().unwrap_or(""));
                }
            }
        }
        "result" => {
            let is_error = v["is_error"].as_bool().unwrap_or(false)
                || v["subtype"].as_str().is_some_and(|s| s != "success");
            if is_error {
                let text = v["result"].as_str().map(str::to_string).unwrap_or_else(|| {
                    v["errors"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                        .unwrap_or_else(|| v["subtype"].to_string())
                });
                return Some(Err(map_result_error(&text)));
            }
            let u = &v["usage"];
            let input = u["input_tokens"].as_u64().map(|n| {
                n + u["cache_read_input_tokens"].as_u64().unwrap_or(0)
                    + u["cache_creation_input_tokens"].as_u64().unwrap_or(0)
            });
            // `total_cost_usd` is a client-side estimate, not a bill; for plan
            // users it is not charged. Cue therefore does not show it as cost.
            let usage = Usage {
                input_tokens: input,
                output_tokens: u["output_tokens"].as_u64(),
                reported_cost_usd: None,
            };
            let stop = match v["stop_reason"].as_str() {
                Some("max_tokens") => StopReason::MaxTokens,
                Some("refusal") => StopReason::Refusal,
                _ => StopReason::Complete,
            };
            let _ = tx.send(StreamEvent::Usage {
                usage: usage.clone(),
            });
            return Some(Ok(ChatOutcome { stop, usage }));
        }
        _ => {}
    }
    None
}

#[async_trait]
impl Provider for ClaudeCodeProvider {
    fn name(&self) -> &str {
        NAME
    }

    async fn list_models(&self) -> CueResult<Vec<ModelInfo>> {
        let st = self.status().await?;
        if !st.logged_in {
            return Err(map_result_error("Not logged in"));
        }
        Ok(MODEL_ALIASES
            .iter()
            .map(|(id, name)| ModelInfo {
                id: (*id).into(),
                display_name: (*name).into(),
                context_window: None,
                max_output_tokens: None,
            })
            .collect())
    }

    async fn chat(
        &self,
        req: &ChatRequest,
        tx: &EventTx,
        cancel: &CancellationToken,
    ) -> CueResult<ChatOutcome> {
        let exe = self.executable()?;
        std::fs::create_dir_all(&self.work_dir).map_err(|e| {
            CueError::new(ErrorKind::Unknown, "Could not prepare a working directory.")
                .with_detail(e.to_string())
        })?;

        let mut cmd = self.command(&exe);
        cmd.current_dir(&self.work_dir)
            .stdin(Stdio::piped())
            .args([
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
            ])
            .args([
                "--no-session-persistence",
                "--strict-mcp-config",
                "--disable-slash-commands",
            ])
            .args(["--tools", ""]);
        if req.model != "default" && !req.model.is_empty() {
            cmd.args(["--model", &req.model]);
        }
        let sys_file = match req.system.as_deref().filter(|s| !s.is_empty()) {
            Some(sys) => {
                let path = self
                    .work_dir
                    .join(format!("system-{}.txt", uuid::Uuid::new_v4()));
                std::fs::write(&path, sys).map_err(|e| {
                    CueError::new(ErrorKind::Unknown, "Could not write the system prompt.")
                        .with_detail(e.to_string())
                })?;
                cmd.arg("--system-prompt-file").arg(&path);
                Some(path)
            }
            None => None,
        };
        struct Cleanup(Option<PathBuf>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                if let Some(p) = &self.0 {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        let _cleanup = Cleanup(sys_file);

        let mut child = cmd.spawn().map_err(|e| {
            CueError::new(ErrorKind::NotConfigured, "Could not start Claude Code.")
                .with_detail(e.to_string())
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            let prompt = render_prompt(req);
            stdin.write_all(prompt.as_bytes()).await.map_err(|e| {
                CueError::new(
                    ErrorKind::Unknown,
                    "Could not send the prompt to Claude Code.",
                )
                .with_detail(e.to_string())
            })?;
            drop(stdin);
        }
        let stdout = child.stdout.take().expect("stdout piped");
        let mut stderr = child.stderr.take().expect("stderr piped");
        let stderr_task = tokio::spawn(async move {
            let mut s = String::new();
            let _ = (&mut stderr).take(16 * 1024).read_to_string(&mut s).await;
            s
        });

        let mut lines = BufReader::new(stdout).lines();
        let mut got_partial = false;
        let mut outcome: Option<CueResult<ChatOutcome>> = None;
        loop {
            let line = tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    return Err(CueError::cancelled());
                }
                l = lines.next_line() => l,
            };
            match line {
                Ok(Some(l)) => {
                    if let Some(o) = handle_line(&l, tx, &mut got_partial) {
                        outcome = Some(o);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    outcome.get_or_insert(Err(CueError::new(
                        ErrorKind::Unknown,
                        "Reading Claude Code output failed.",
                    )
                    .with_detail(e.to_string())));
                    break;
                }
            }
        }
        let status = child.wait().await.ok();
        let stderr = stderr_task.await.unwrap_or_default();
        match outcome {
            Some(o) => o,
            None => {
                let detail = if stderr.trim().is_empty() {
                    format!(
                        "Claude Code exited ({}) without a result.",
                        status.map(|s| s.to_string()).unwrap_or_default()
                    )
                } else {
                    stderr.trim().to_string()
                };
                Err(map_result_error(&detail))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    fn chan() -> (EventTx, tokio::sync::mpsc::UnboundedReceiver<StreamEvent>) {
        tokio::sync::mpsc::unbounded_channel()
    }

    #[test]
    fn parses_stream_json() {
        let (tx, mut rx) = chan();
        let mut partial = false;
        let lines = [
            r#"{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":[]}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"}]}}"#,
        ];
        for l in lines {
            assert!(handle_line(l, &tx, &mut partial).is_none());
        }
        let res = handle_line(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello","usage":{"input_tokens":3,"cache_read_input_tokens":10,"output_tokens":2},"total_cost_usd":0.01}"#,
            &tx,
            &mut partial,
        )
        .unwrap()
        .unwrap();
        assert_eq!(res.usage.input_tokens, Some(13));
        assert_eq!(res.usage.reported_cost_usd, None);
        let mut text = String::new();
        while let Ok(ev) = rx.try_recv() {
            if let StreamEvent::Delta { text: t } = ev {
                text.push_str(&t);
            }
        }
        assert_eq!(
            text, "Hello",
            "assistant message must not duplicate partial deltas"
        );
    }

    #[test]
    fn maps_not_logged_in() {
        let (tx, _rx) = chan();
        let mut p = false;
        let e = handle_line(r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#, &tx, &mut p)
            .unwrap()
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::Auth);
    }

    #[test]
    fn renders_multi_turn_prompt() {
        let req = ChatRequest {
            model: "sonnet".into(),
            messages: vec![
                ChatMessage::user("a"),
                ChatMessage::assistant("b"),
                ChatMessage::user("c"),
            ],
            system: None,
            max_output_tokens: None,
        };
        let p = render_prompt(&req);
        assert!(
            p.contains("<user>\na\n</user>")
                && p.contains("<assistant>\nb\n</assistant>")
                && p.ends_with("account.")
        );
    }

    /// End-to-end test against a fake `claude` executable (Unix only).
    #[cfg(unix)]
    #[tokio::test]
    async fn runs_fake_cli() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cue-fake-claude-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("claude");
        std::fs::write(
            &exe,
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "9.9.9 (Claude Code)"; exit 0; fi
if [ "$1" = "auth" ]; then echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max"}'; exit 0; fi
input=$(cat)
echo '{"type":"system","subtype":"init","model":"claude-test"}'
printf '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"echo: %s"}}}\n' "$input"
echo '{"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":1,"output_tokens":2}}'
"#,
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = ClaudeCodeProvider::new(Some(exe.display().to_string()))
            .with_work_dir(dir.join("work"));
        let st = p.status().await.unwrap();
        assert!(st.logged_in);
        assert_eq!(st.auth_method.as_deref(), Some("claude.ai"));
        assert_eq!(st.subscription.as_deref(), Some("max"));
        assert_eq!(p.list_models().await.unwrap().len(), MODEL_ALIASES.len());

        let (tx, mut rx) = chan();
        let req = ChatRequest {
            model: "sonnet".into(),
            messages: vec![ChatMessage::user("hi")],
            system: Some("be brief".into()),
            max_output_tokens: None,
        };
        let out = p.chat(&req, &tx, &CancellationToken::new()).await.unwrap();
        assert_eq!(out.usage.output_tokens, Some(2));
        let mut text = String::new();
        while let Ok(ev) = rx.try_recv() {
            if let StreamEvent::Delta { text: t } = ev {
                text.push_str(&t);
            }
        }
        assert_eq!(text, "echo: hi");
        // System prompt temp file is removed after the run.
        let leftovers = std::fs::read_dir(dir.join("work")).unwrap().count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
