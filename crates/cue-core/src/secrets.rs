//! Credential storage.
//!
//! Secrets (API keys, OAuth tokens) are kept exclusively in the operating
//! system's credential store:
//! * macOS – Keychain Services
//! * Windows – Credential Manager
//! * Linux – Secret Service (GNOME Keyring / KWallet)
//!
//! They are never written to the settings file, never logged and never sent to
//! the web frontend.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{CueError, CueResult, ErrorKind};

pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> CueResult<Option<String>>;
    fn set(&self, key: &str, value: &str) -> CueResult<()>;
    fn delete(&self, key: &str) -> CueResult<()>;
}

pub fn api_key_entry(connection_id: &str) -> String {
    format!("connection/{connection_id}/api_key")
}

pub fn oauth_entry(connection_id: &str) -> String {
    format!("connection/{connection_id}/oauth")
}

/// In-memory store for tests and for environments without a keychain.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> CueResult<Option<String>> {
        Ok(self.0.lock().expect("lock").get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> CueResult<()> {
        self.0
            .lock()
            .expect("lock")
            .insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> CueResult<()> {
        self.0.lock().expect("lock").remove(key);
        Ok(())
    }
}

/// Windows Credential Manager limits a credential blob to 2560 bytes. OAuth
/// token sets (several JWTs) can exceed that, so long values are split into
/// numbered chunks. The head entry stores either `v1:<value>` or `v1c:<n>`.
const CHUNK: usize = 1200;

pub(crate) fn encode_chunks(value: &str) -> (String, Vec<String>) {
    if value.len() + 3 <= CHUNK {
        return (format!("v1:{value}"), vec![]);
    }
    let mut parts = Vec::new();
    let mut rest = value;
    while !rest.is_empty() {
        let mut cut = rest.len().min(CHUNK);
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        parts.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    (format!("v1c:{}", parts.len()), parts)
}

pub(crate) fn chunk_key(key: &str, i: usize) -> String {
    format!("{key}#{i}")
}

/// A [`SecretStore`] that adds chunking on top of a raw key/value backend.
pub struct ChunkedStore<B: RawBackend>(pub B);

pub trait RawBackend: Send + Sync {
    fn raw_get(&self, key: &str) -> CueResult<Option<String>>;
    fn raw_set(&self, key: &str, value: &str) -> CueResult<()>;
    fn raw_delete(&self, key: &str) -> CueResult<()>;
}

impl<B: RawBackend> SecretStore for ChunkedStore<B> {
    fn get(&self, key: &str) -> CueResult<Option<String>> {
        let Some(head) = self.0.raw_get(key)? else {
            return Ok(None);
        };
        if let Some(v) = head.strip_prefix("v1:") {
            return Ok(Some(v.to_string()));
        }
        if let Some(n) = head
            .strip_prefix("v1c:")
            .and_then(|n| n.parse::<usize>().ok())
        {
            let mut out = String::new();
            for i in 0..n {
                match self.0.raw_get(&chunk_key(key, i))? {
                    Some(p) => out.push_str(&p),
                    None => {
                        return Err(CueError::new(
                            ErrorKind::SecretStore,
                            "A stored credential is incomplete.",
                        )
                        .with_hint("Remove and re-add this connection."));
                    }
                }
            }
            return Ok(Some(out));
        }
        // Value written without a header (e.g. by an older version).
        Ok(Some(head))
    }

    fn set(&self, key: &str, value: &str) -> CueResult<()> {
        self.delete(key)?;
        let (head, parts) = encode_chunks(value);
        for (i, p) in parts.iter().enumerate() {
            self.0.raw_set(&chunk_key(key, i), p)?;
        }
        self.0.raw_set(key, &head)
    }

    fn delete(&self, key: &str) -> CueResult<()> {
        if let Some(head) = self.0.raw_get(key)? {
            if let Some(n) = head
                .strip_prefix("v1c:")
                .and_then(|n| n.parse::<usize>().ok())
            {
                for i in 0..n {
                    self.0.raw_delete(&chunk_key(key, i))?;
                }
            }
            self.0.raw_delete(key)?;
        }
        Ok(())
    }
}

#[cfg(feature = "keyring")]
pub use os::KeyringBackend;

#[cfg(feature = "keyring")]
mod os {
    use super::*;

    /// OS credential store via the `keyring` crate.
    pub struct KeyringBackend {
        pub service: String,
    }

    fn store_err(e: keyring::Error) -> CueError {
        CueError::from_kind(ErrorKind::SecretStore, "credential store").with_detail(e.to_string())
    }

    impl KeyringBackend {
        pub fn new(service: impl Into<String>) -> Self {
            Self {
                service: service.into(),
            }
        }

        /// Whether the platform credential store could be initialised.
        pub fn available() -> CueResult<()> {
            keyring::Entry::store_status()
                .as_ref()
                .map(|_| ())
                .map_err(|e| {
                    CueError::from_kind(ErrorKind::SecretStore, "credential store")
                        .with_detail(e.to_string())
                })
        }

        fn entry(&self, key: &str) -> CueResult<keyring::Entry> {
            keyring::Entry::new(&self.service, key).map_err(store_err)
        }
    }

    impl RawBackend for KeyringBackend {
        fn raw_get(&self, key: &str) -> CueResult<Option<String>> {
            match self.entry(key)?.get_password() {
                Ok(v) => Ok(Some(v)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(store_err(e)),
            }
        }
        fn raw_set(&self, key: &str, value: &str) -> CueResult<()> {
            self.entry(key)?.set_password(value).map_err(store_err)
        }
        fn raw_delete(&self, key: &str) -> CueResult<()> {
            match self.entry(key)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(store_err(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Raw(Mutex<HashMap<String, String>>);
    impl RawBackend for Raw {
        fn raw_get(&self, k: &str) -> CueResult<Option<String>> {
            Ok(self.0.lock().unwrap().get(k).cloned())
        }
        fn raw_set(&self, k: &str, v: &str) -> CueResult<()> {
            assert!(v.len() <= 2560, "value exceeds Windows blob limit");
            self.0.lock().unwrap().insert(k.into(), v.into());
            Ok(())
        }
        fn raw_delete(&self, k: &str) -> CueResult<()> {
            self.0.lock().unwrap().remove(k);
            Ok(())
        }
    }

    #[test]
    fn chunked_roundtrip_and_cleanup() {
        let s = ChunkedStore(Raw::default());
        let big = "ä".repeat(3000) + "end";
        s.set("k", &big).unwrap();
        assert_eq!(s.get("k").unwrap().as_deref(), Some(big.as_str()));
        s.set("k", "small").unwrap();
        assert_eq!(s.get("k").unwrap().as_deref(), Some("small"));
        assert_eq!(s.0.0.lock().unwrap().len(), 1, "old chunks removed");
        s.delete("k").unwrap();
        assert_eq!(s.get("k").unwrap(), None);
        assert!(s.0.0.lock().unwrap().is_empty());
    }
}
