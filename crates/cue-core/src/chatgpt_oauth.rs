//! Official OpenAI "Sign in with ChatGPT" for open-source / local apps,
//! including the optional ChatGPT plan usage permission.
//!
//! Implemented strictly after OpenAI's documentation:
//! <https://developers.openai.com/siwc/token-sharing-open-source/sign-in>
//! <https://developers.openai.com/siwc/token-sharing-open-source/profiles-and-sessions>
//!
//! * OAuth 2.0 authorization code flow with PKCE (S256) in the **system
//!   browser**; Cue never sees the user's password.
//! * Loopback redirect on `http://127.0.0.1:<port>/auth/callback`.
//! * First sign-in uses dynamic registration (`client_id=dynamic_agent_client`);
//!   the issued `client_id` is stored and reused afterwards.
//! * The ID token is verified against OpenAI's JWKS (signature, issuer,
//!   audience, expiry, nonce) and the granted scopes must contain
//!   `chatgpt.tokens.use.direct` before any plan-based inference.
//! * Tokens live only in the OS credential store.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::error::{CueError, CueResult, ErrorKind};

pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const RESOURCE: &str = "https://api.openai.com/v1";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const CALLBACK_PATH: &str = "/auth/callback";
const PREFERRED_PORT: u16 = 1455;
pub const APP_NAME: &str = "Cue";
pub const USAGE_URL: &str = "https://chatgpt.com/settings/usage";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub issuer: String,
    pub authorize: String,
    pub token: String,
    pub revoke: String,
    pub jwks: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        // Values from https://auth.openai.com/.well-known/openid-configuration
        Self {
            issuer: "https://auth.openai.com".into(),
            authorize: "https://auth.openai.com/api/accounts/authorize".into(),
            token: "https://auth.openai.com/api/accounts/oauth/token".into(),
            revoke: "https://auth.openai.com/api/accounts/oauth/revoke".into(),
            jwks: "https://auth.openai.com/.well-known/jwks.json".into(),
        }
    }
}

/// Everything stored (as JSON) in the OS credential store for one account.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatGptCredentials {
    pub email: Option<String>,
    pub subject: String,
    pub client_id: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub scopes: Vec<String>,
    /// Unix seconds.
    pub expires_at: u64,
}

impl std::fmt::Debug for ChatGptCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatGptCredentials")
            .field("email", &self.email)
            .field("client_id", &self.client_id)
            .field("scopes", &self.scopes)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl ChatGptCredentials {
    pub fn has_plan_scope(&self) -> bool {
        self.scopes.iter().any(|s| s == PLAN_SCOPE)
    }

    /// True when the access token expires within `margin`.
    pub fn needs_refresh(&self, margin: Duration) -> bool {
        now() + margin.as_secs() >= self.expires_at
    }
}

/// Information about an existing registration, for re-authorisation.
#[derive(Debug, Clone, Default)]
pub struct Reauth {
    pub client_id: String,
    pub id_token_hint: Option<String>,
    pub login_hint: Option<String>,
    pub expected_subject: Option<String>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::fill(&mut buf[..]);
    URL_SAFE_NO_PAD.encode(buf)
}

pub(crate) fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub struct PendingAuth {
    pub state: String,
    pub nonce: String,
    pub verifier: String,
    pub redirect_uri: String,
    /// `None` for a first-time dynamic registration.
    pub reauth: Option<Reauth>,
}

pub fn authorize_url(ep: &Endpoints, p: &PendingAuth, host_id: &str) -> String {
    let mut u = url::Url::parse(&ep.authorize).expect("valid authorize endpoint");
    {
        let mut q = u.query_pairs_mut();
        match &p.reauth {
            None => {
                q.append_pair("client_id", DYNAMIC_CLIENT);
                q.append_pair("agent_name_hint", APP_NAME);
            }
            Some(r) => {
                q.append_pair("client_id", &r.client_id);
                if let Some(h) = &r.id_token_hint {
                    q.append_pair("id_token_hint", h);
                }
                if let Some(h) = &r.login_hint {
                    q.append_pair("login_hint", h);
                }
            }
        }
        q.append_pair("ext_agent_host_id", host_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &p.redirect_uri)
            .append_pair("scope", SCOPES)
            .append_pair("resource", RESOURCE)
            .append_pair("state", &p.state)
            .append_pair("nonce", &p.nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &pkce_challenge(&p.verifier));
    }
    u.into()
}

/// Bind the loopback listener (preferred port 1455, otherwise any free port).
async fn bind_loopback() -> CueResult<TcpListener> {
    if let Ok(l) = TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).await {
        return Ok(l);
    }
    TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| {
        CueError::new(
            ErrorKind::Network,
            "Could not open a local port for sign-in.",
        )
        .with_detail(e.to_string())
    })
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Cue</title><body style=\"font:15px system-ui;padding:48px;text-align:center\"><h2>You can close this tab.</h2><p>Return to Cue.</p></body>";

/// Wait for the browser redirect. Returns the callback query parameters.
async fn await_callback(
    listener: TcpListener,
    cancel: &CancellationToken,
) -> CueResult<HashMap<String, String>> {
    let deadline = tokio::time::sleep(Duration::from_secs(300));
    tokio::pin!(deadline);
    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => return Err(CueError::cancelled()),
            _ = &mut deadline => return Err(CueError::new(ErrorKind::Timeout, "Sign-in timed out.").with_hint("Start the sign-in again.")),
            a = listener.accept() => a,
        };
        let Ok((mut sock, _)) = accepted else {
            continue;
        };
        let mut buf = vec![0u8; 16 * 1024];
        let n = match tokio::time::timeout(Duration::from_secs(10), sock.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let req = String::from_utf8_lossy(&buf[..n]);
        let target = req
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("");
        let parsed = url::Url::parse(&format!("http://127.0.0.1{target}")).ok();
        match parsed {
            Some(u) if u.path() == CALLBACK_PATH => {
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{DONE_PAGE}",
                    DONE_PAGE.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                return Ok(u.query_pairs().into_owned().collect());
            }
            _ => {
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct IdClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

fn oauth_error(context: &str, status: u16, body: &str) -> CueError {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let code = v["error"]
        .as_str()
        .or_else(|| v["error"]["code"].as_str())
        .unwrap_or("");
    let desc = v["error_description"]
        .as_str()
        .or_else(|| v["error"]["message"].as_str())
        .unwrap_or(if code.is_empty() { body } else { "" });
    let terminal_refresh = matches!(
        code,
        "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused"
    );
    let kind = if terminal_refresh || status == 401 {
        ErrorKind::Auth
    } else if code == "invalid_client" {
        ErrorKind::NotConfigured
    } else if status >= 500 {
        ErrorKind::Overloaded
    } else {
        ErrorKind::InvalidRequest
    };
    let detail = match (desc.is_empty(), code.is_empty()) {
        (true, _) => code.to_string(),
        (false, true) => desc.to_string(),
        (false, false) => format!("{desc} ({code})"),
    };
    let mut e = CueError::new(kind, format!("{context} failed."))
        .with_detail(detail)
        .with_status(status);
    if kind == ErrorKind::Auth {
        e = e.with_hint("Sign in with ChatGPT again under AI Providers.");
    }
    e
}

/// Verify an ID token per OpenAI's instructions and return its claims.
async fn verify_id_token(
    http: &reqwest::Client,
    ep: &Endpoints,
    token: &str,
    client_id: &str,
    nonce: Option<&str>,
) -> CueResult<IdClaims> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
    let bad = |d: String| {
        CueError::new(
            ErrorKind::Auth,
            "The ChatGPT sign-in could not be verified.",
        )
        .with_detail(d)
    };
    let header = decode_header(token).map_err(|e| bad(e.to_string()))?;
    let jwks: JwkSet = http
        .get(&ep.jwks)
        .send()
        .await
        .map_err(|e| CueError::from_reqwest(&e, "OpenAI"))?
        .json()
        .await
        .map_err(|e| bad(format!("JWKS: {e}")))?;
    let jwk = match &header.kid {
        Some(kid) => jwks.find(kid),
        None => jwks.keys.first(),
    }
    .ok_or_else(|| bad("signing key not found in JWKS".into()))?;
    let key = DecodingKey::from_jwk(jwk).map_err(|e| bad(e.to_string()))?;
    let mut v = Validation::new(Algorithm::RS256);
    v.set_issuer(&[&ep.issuer]);
    v.set_audience(&[client_id]);
    v.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let data = decode::<IdClaims>(token, &key, &v).map_err(|e| bad(e.to_string()))?;
    if let Some(n) = nonce
        && data.claims.nonce.as_deref() != Some(n)
    {
        return Err(bad("nonce mismatch".into()));
    }
    Ok(data.claims)
}

pub struct ChatGptAuth {
    pub http: reqwest::Client,
    pub endpoints: Endpoints,
}

impl ChatGptAuth {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            endpoints: Endpoints::default(),
        }
    }

    /// Full interactive sign-in. `open_browser` must open the URL in the
    /// user's default browser.
    pub async fn sign_in(
        &self,
        host_id: &str,
        reauth: Option<Reauth>,
        open_browser: impl FnOnce(&str) -> Result<(), String>,
        cancel: &CancellationToken,
    ) -> CueResult<ChatGptCredentials> {
        let listener = bind_loopback().await?;
        let port = listener
            .local_addr()
            .map_err(|e| {
                CueError::new(ErrorKind::Network, "Local port error.").with_detail(e.to_string())
            })?
            .port();
        let pending = PendingAuth {
            state: random_token(32),
            nonce: random_token(32),
            verifier: random_token(48),
            redirect_uri: format!("http://127.0.0.1:{port}{CALLBACK_PATH}"),
            reauth,
        };
        let url = authorize_url(&self.endpoints, &pending, host_id);
        open_browser(&url).map_err(|e| {
            CueError::new(ErrorKind::Unknown, "Could not open the browser.").with_detail(e)
        })?;
        let params = await_callback(listener, cancel).await?;
        self.complete(&pending, &params).await
    }

    /// Validate the callback, exchange the code and verify the result.
    pub async fn complete(
        &self,
        p: &PendingAuth,
        params: &HashMap<String, String>,
    ) -> CueResult<ChatGptCredentials> {
        if params.get("state") != Some(&p.state) {
            return Err(CueError::new(
                ErrorKind::Auth,
                "Sign-in response did not match this request.",
            )
            .with_detail("state mismatch"));
        }
        if let Some(err) = params.get("error") {
            return Err(if err == "access_denied" {
                CueError::new(
                    ErrorKind::Auth,
                    "Sign-in was cancelled or ChatGPT plan use was not approved.",
                )
                .with_hint("You can sign in again at any time and approve plan usage.")
            } else {
                CueError::new(ErrorKind::Auth, "ChatGPT sign-in failed.").with_detail(
                    params
                        .get("error_description")
                        .cloned()
                        .unwrap_or_else(|| err.clone()),
                )
            });
        }
        let code = params.get("code").ok_or_else(|| {
            CueError::new(ErrorKind::Auth, "Sign-in returned no authorization code.")
        })?;
        let client_id = match (&p.reauth, params.get("client_id")) {
            (None, Some(issued)) if issued != DYNAMIC_CLIENT => issued.clone(),
            (None, _) => {
                return Err(
                    CueError::new(ErrorKind::Auth, "Registration did not complete.")
                        .with_detail("callback did not include an issued client_id"),
                );
            }
            (Some(r), Some(cb)) if cb != &r.client_id => {
                return Err(CueError::new(
                    ErrorKind::Auth,
                    "Sign-in returned a different app registration.",
                )
                .with_detail("client_id mismatch; result rejected"));
            }
            (Some(r), _) => r.client_id.clone(),
        };

        let resp = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id.as_str()),
                ("code", code.as_str()),
                ("code_verifier", p.verifier.as_str()),
                ("redirect_uri", p.redirect_uri.as_str()),
                ("resource", RESOURCE),
            ])
            .send()
            .await
            .map_err(|e| CueError::from_reqwest(&e, "OpenAI"))?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(oauth_error("Code exchange", status, &body));
        }
        let tok: TokenResponse = serde_json::from_str(&body).map_err(|e| {
            CueError::new(ErrorKind::Auth, "Unexpected token response.").with_detail(e.to_string())
        })?;
        let id_token = tok
            .id_token
            .clone()
            .ok_or_else(|| CueError::new(ErrorKind::Auth, "No ID token returned."))?;
        let claims = verify_id_token(
            &self.http,
            &self.endpoints,
            &id_token,
            &client_id,
            Some(&p.nonce),
        )
        .await?;
        if let Some(expected) = p
            .reauth
            .as_ref()
            .and_then(|r| r.expected_subject.as_deref())
            && expected != claims.sub
        {
            return Err(CueError::new(
                ErrorKind::Auth,
                "You signed in with a different ChatGPT account.",
            )
            .with_hint("Add it as a separate connection instead."));
        }
        let creds = ChatGptCredentials {
            email: claims.email,
            subject: claims.sub,
            client_id,
            id_token,
            access_token: tok.access_token,
            refresh_token: tok.refresh_token,
            scopes: tok
                .scope
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_string)
                .collect(),
            expires_at: now() + tok.expires_in.unwrap_or(3600),
        };
        if !creds.has_plan_scope() {
            return Err(CueError::new(
                ErrorKind::PermissionDenied,
                "ChatGPT plan usage was not granted.",
            )
            .with_hint(
                "Sign in again and allow Cue to use your ChatGPT plan, or use an OpenAI API key.",
            ));
        }
        Ok(creds)
    }

    /// Standard OAuth refresh (rotating refresh token).
    pub async fn refresh(&self, creds: &ChatGptCredentials) -> CueResult<ChatGptCredentials> {
        let rt = creds.refresh_token.as_deref().ok_or_else(|| {
            CueError::new(ErrorKind::Auth, "ChatGPT session expired.")
                .with_hint("Sign in with ChatGPT again.")
        })?;
        let resp = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", creds.client_id.as_str()),
                ("refresh_token", rt),
                ("resource", RESOURCE),
            ])
            .send()
            .await
            .map_err(|e| CueError::from_reqwest(&e, "OpenAI"))?;
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            return Err(oauth_error("Refreshing the ChatGPT session", status, &body));
        }
        let tok: TokenResponse = serde_json::from_str(&body).map_err(|e| {
            CueError::new(ErrorKind::Auth, "Unexpected token response.").with_detail(e.to_string())
        })?;
        let mut next = creds.clone();
        next.access_token = tok.access_token;
        next.expires_at = now() + tok.expires_in.unwrap_or(3600);
        if let Some(rt) = tok.refresh_token {
            next.refresh_token = Some(rt);
        }
        if let Some(s) = tok.scope {
            next.scopes = s.split_whitespace().map(str::to_string).collect();
        }
        if let Some(id) = tok.id_token {
            next.id_token = id;
        }
        Ok(next)
    }

    /// Revoke the renewable session on sign-out. Returns Ok(false) when remote
    /// revocation could not be confirmed (local tokens are cleared regardless).
    pub async fn revoke(&self, creds: &ChatGptCredentials) -> bool {
        let Some(rt) = creds.refresh_token.as_deref() else {
            return true;
        };
        for attempt in 0..3 {
            let r = self
                .http
                .post(&self.endpoints.revoke)
                .form(&[
                    ("token", rt),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", creds.client_id.as_str()),
                ])
                .send()
                .await;
            match r {
                Ok(resp) if resp.status().is_success() => return true,
                Ok(resp) if resp.status().is_client_error() => return false,
                _ => tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await,
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TEST_KEY_DER_B64: &str = include_str!("testdata/test_rsa_key.der.b64");
    const TEST_KEY_N: &str = include_str!("testdata/test_rsa_n.txt");

    fn sign(claims: serde_json::Value) -> String {
        let der = base64::engine::general_purpose::STANDARD
            .decode(TEST_KEY_DER_B64.trim())
            .unwrap();
        let mut h = Header::new(jsonwebtoken::Algorithm::RS256);
        h.kid = Some("k1".into());
        encode(&h, &claims, &EncodingKey::from_rsa_der(&der)).unwrap()
    }

    async fn server(id_token: &str, scope: &str) -> (MockServer, Endpoints) {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": [{ "kty": "RSA", "kid": "k1", "use": "sig", "alg": "RS256", "n": TEST_KEY_N.trim(), "e": "AQAB" }]
            })))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("client_id=oaiapp_123"))
            .and(body_string_contains("code_verifier=verifier"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at", "refresh_token": "rt", "id_token": id_token,
                "token_type": "Bearer", "expires_in": 3600, "scope": scope
            })))
            .mount(&s)
            .await;
        let ep = Endpoints {
            issuer: "https://auth.openai.com".into(),
            authorize: format!("{}/authorize", s.uri()),
            token: format!("{}/token", s.uri()),
            revoke: format!("{}/revoke", s.uri()),
            jwks: format!("{}/jwks", s.uri()),
        };
        (s, ep)
    }

    fn pending() -> PendingAuth {
        PendingAuth {
            state: "st".into(),
            nonce: "nn".into(),
            verifier: "verifier".into(),
            redirect_uri: "http://127.0.0.1:1455/auth/callback".into(),
            reauth: None,
        }
    }

    fn cb(extra: &[(&str, &str)]) -> HashMap<String, String> {
        let mut m: HashMap<String, String> = [
            ("state", "st"),
            ("code", "c0de"),
            ("client_id", "oaiapp_123"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        for (k, v) in extra {
            m.insert(k.to_string(), v.to_string());
        }
        m
    }

    fn claims(nonce: &str, aud: &str) -> serde_json::Value {
        serde_json::json!({
            "iss": "https://auth.openai.com", "aud": aud, "sub": "user-1", "email": "a@b.c",
            "nonce": nonce, "exp": now() + 600, "iat": now()
        })
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_has_documented_params() {
        let u = authorize_url(&Endpoints::default(), &pending(), "urn:uuid:x");
        let q: HashMap<String, String> = url::Url::parse(&u)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        assert_eq!(q["client_id"], "dynamic_agent_client");
        assert_eq!(q["agent_name_hint"], "Cue");
        assert_eq!(q["ext_agent_host_id"], "urn:uuid:x");
        assert_eq!(q["scope"], SCOPES);
        assert_eq!(q["resource"], RESOURCE);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
        let mut p = pending();
        p.reauth = Some(Reauth {
            client_id: "oaiapp_1".into(),
            login_hint: Some("a@b.c".into()),
            ..Default::default()
        });
        let q: HashMap<String, String> =
            url::Url::parse(&authorize_url(&Endpoints::default(), &p, "h"))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
        assert_eq!(q["client_id"], "oaiapp_1");
        assert!(!q.contains_key("agent_name_hint"));
        assert_eq!(q["login_hint"], "a@b.c");
    }

    #[tokio::test]
    async fn complete_success_verifies_id_token() {
        let tok = sign(claims("nn", "oaiapp_123"));
        let (_s, ep) = server(&tok, SCOPES).await;
        let auth = ChatGptAuth {
            http: reqwest::Client::new(),
            endpoints: ep,
        };
        let c = auth.complete(&pending(), &cb(&[])).await.unwrap();
        assert_eq!(c.client_id, "oaiapp_123");
        assert_eq!(c.subject, "user-1");
        assert!(c.has_plan_scope());
        assert_eq!(c.refresh_token.as_deref(), Some("rt"));
    }

    #[tokio::test]
    async fn rejects_bad_nonce_audience_state_and_missing_scope() {
        let auth_for = |ep| ChatGptAuth {
            http: reqwest::Client::new(),
            endpoints: ep,
        };

        let (_s, ep) = server(&sign(claims("other", "oaiapp_123")), SCOPES).await;
        assert_eq!(
            auth_for(ep)
                .complete(&pending(), &cb(&[]))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Auth
        );

        let (_s, ep) = server(&sign(claims("nn", "someone_else")), SCOPES).await;
        assert_eq!(
            auth_for(ep)
                .complete(&pending(), &cb(&[]))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Auth
        );

        let (_s, ep) = server(&sign(claims("nn", "oaiapp_123")), "openid email profile").await;
        assert_eq!(
            auth_for(ep)
                .complete(&pending(), &cb(&[]))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::PermissionDenied
        );

        let (_s, ep) = server(&sign(claims("nn", "oaiapp_123")), SCOPES).await;
        let a = auth_for(ep);
        assert!(
            a.complete(&pending(), &cb(&[("state", "forged")]))
                .await
                .unwrap_err()
                .detail
                .unwrap()
                .contains("state")
        );
        assert_eq!(
            a.complete(&pending(), &cb(&[("error", "access_denied")]))
                .await
                .unwrap_err()
                .kind,
            ErrorKind::Auth
        );
        let mut no_client = cb(&[]);
        no_client.remove("client_id");
        assert!(
            a.complete(&pending(), &no_client).await.is_err(),
            "new registration must return an issued client_id"
        );
    }

    #[tokio::test]
    async fn tampered_signature_is_rejected() {
        let mut tok = sign(claims("nn", "oaiapp_123"));
        tok.pop();
        tok.push(if tok.ends_with('A') { 'B' } else { 'A' });
        let (_s, ep) = server(&tok, SCOPES).await;
        let a = ChatGptAuth {
            http: reqwest::Client::new(),
            endpoints: ep,
        };
        assert_eq!(
            a.complete(&pending(), &cb(&[])).await.unwrap_err().kind,
            ErrorKind::Auth
        );
    }

    #[tokio::test]
    async fn refresh_rotates_and_terminal_errors_map_to_auth() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=rt1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at2", "refresh_token": "rt2", "expires_in": 10, "scope": SCOPES
            })))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("refresh_token=dead"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({ "error": "refresh_token_reused" })),
            )
            .mount(&s)
            .await;
        let ep = Endpoints {
            token: format!("{}/token", s.uri()),
            ..Endpoints::default()
        };
        let a = ChatGptAuth {
            http: reqwest::Client::new(),
            endpoints: ep,
        };
        let base = ChatGptCredentials {
            email: None,
            subject: "u".into(),
            client_id: "oaiapp_1".into(),
            id_token: "id".into(),
            access_token: "at1".into(),
            refresh_token: Some("rt1".into()),
            scopes: vec![PLAN_SCOPE.into()],
            expires_at: 0,
        };
        let n = a.refresh(&base).await.unwrap();
        assert_eq!(
            (n.access_token.as_str(), n.refresh_token.as_deref()),
            ("at2", Some("rt2"))
        );
        assert!(n.needs_refresh(Duration::from_secs(60)));
        let dead = ChatGptCredentials {
            refresh_token: Some("dead".into()),
            ..base
        };
        assert_eq!(a.refresh(&dead).await.unwrap_err().kind, ErrorKind::Auth);
    }

    #[tokio::test]
    async fn loopback_callback_roundtrip() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let cancel = CancellationToken::new();
        let h = tokio::spawn(async move { await_callback(listener, &cancel).await });
        let c = reqwest::Client::builder().no_proxy().build().unwrap();
        let r = c
            .get(format!("http://127.0.0.1:{port}/favicon.ico"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 404);
        let r = c
            .get(format!(
                "http://127.0.0.1:{port}/auth/callback?code=abc&state=xyz"
            ))
            .send()
            .await
            .unwrap();
        assert!(r.text().await.unwrap().contains("close this tab"));
        let q = h.await.unwrap().unwrap();
        assert_eq!(q["code"], "abc");
        assert_eq!(q["state"], "xyz");
    }
}
