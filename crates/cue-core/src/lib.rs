//! Cue core: provider-neutral chat interface, provider implementations,
//! authentication helpers, secure credential storage and settings.
//!
//! This crate has no UI or Tauri dependency so it can be tested headlessly.

pub mod chatgpt_oauth;
pub mod error;
pub mod providers;
pub mod secrets;
pub mod settings;
pub mod sse;
pub mod types;

pub use error::{CueError, CueResult, ErrorKind};
pub use providers::{Credential, Provider, ProviderConfig, build_provider, http_client};
pub use types::*;

/// Re-exported so the app shell uses the exact same HTTP client type.
pub use reqwest;
