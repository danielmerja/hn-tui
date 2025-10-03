use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rand::RngCore;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tiny_http::{Header, Method, Response, Server};
use url::Url;

use crate::reddit::{OAuthToken, TokenProvider};
use crate::storage::{self, Account, Token};

static HTML_SUCCESS: Lazy<String> = Lazy::new(|| {
    r#"<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Reddix Authorization Complete</title>
    <style>
      :root {
        color-scheme: dark light;
        --bg: #11151d;
        --panel: #1c2230;
        --accent: #52b4ff;
        --text: #e8edf5;
        --muted: #9aa3b7;
        font-family: "Inter", "Segoe UI", -apple-system, BlinkMacSystemFont, "Helvetica Neue", sans-serif;
      }
      body {
        margin: 0;
        min-height: 100vh;
        display: flex;
        align-items: center;
        justify-content: center;
        background: var(--bg);
        color: var(--text);
      }
      .card {
        background: var(--panel);
        padding: 2.5rem 3rem;
        border-radius: 16px;
        box-shadow: 0 18px 45px rgba(9, 17, 28, 0.45);
        max-width: 480px;
        text-align: center;
      }
      h1 {
        margin: 0 0 1rem;
        font-size: 1.9rem;
        color: var(--accent);
      }
      p {
        margin: 0 0 1.25rem;
        line-height: 1.5;
        color: var(--muted);
      }
      .cta {
        display: inline-block;
        margin-top: 1rem;
        padding: 0.75rem 1.75rem;
        border-radius: 999px;
        background: var(--accent);
        color: var(--bg);
        font-weight: 600;
        text-decoration: none;
        transition: transform 0.15s ease, box-shadow 0.15s ease;
      }
      .cta:hover {
        transform: translateY(-2px);
        box-shadow: 0 10px 30px rgba(82, 180, 255, 0.35);
      }
    </style>
  </head>
  <body>
    <main class="card">
      <h1>Authorization Complete</h1>
      <p>Reddix is now connected to your Reddit account. You can close this tab and return to the app.</p>
      <a class="cta" href="https://github.com/ck-zhang/reddix" target="_blank" rel="noreferrer">View project on GitHub</a>
    </main>
  </body>
</html>"#
        .to_string()
});

#[derive(Debug, Clone)]
pub struct Config {
    pub client_id: String,
    pub client_secret: String,
    pub scope: Vec<String>,
    pub user_agent: String,
    pub auth_url: String,
    pub token_url: String,
    pub identity_url: String,
    pub redirect_uri: String,
    pub refresh_skew: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            scope: vec![
                "identity".into(),
                "mysubreddits".into(),
                "read".into(),
                "vote".into(),
            ],
            user_agent: "reddix-dev/0.1".into(),
            auth_url: "https://www.reddit.com/api/v1/authorize".into(),
            token_url: "https://www.reddit.com/api/v1/access_token".into(),
            identity_url: "https://oauth.reddit.com/api/v1/me".into(),
            redirect_uri: "http://127.0.0.1:65010/reddix/callback".into(),
            refresh_skew: Duration::from_secs(30),
        }
    }
}

pub struct Flow {
    cfg: Config,
    store: Arc<storage::Store>,
    client: Client,
    refreshers: Mutex<HashMap<i64, RefreshHandle>>,
}

struct RefreshHandle {
    stop: Sender<()>,
    thread: thread::JoinHandle<()>,
}

pub struct AuthorizationRequest {
    pub browser_url: String,
    pub redirect_uri: String,
    verifier: String,
    rx: Receiver<AuthResult>,
    shutdown: Sender<()>,
}

impl Drop for AuthorizationRequest {
    fn drop(&mut self) {
        let _ = self.shutdown.send(());
    }
}

struct AuthResult {
    code: Option<String>,
    error: Option<anyhow::Error>,
}

#[derive(Clone)]
pub struct Session {
    pub account: Account,
    pub token: OAuthTokenDetails,
}

#[derive(Clone)]
pub struct OAuthTokenDetails {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_at: DateTime<Utc>,
    pub scope: Vec<String>,
}

impl Flow {
    pub fn new(store: Arc<storage::Store>, cfg: Config) -> Result<Self> {
        if cfg.client_id.trim().is_empty() {
            bail!("auth: client id is required");
        }
        if cfg.user_agent.trim().is_empty() {
            bail!("auth: user agent is required");
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .context("auth: build http client")?;

        Ok(Self {
            cfg,
            store,
            client,
            refreshers: Mutex::new(HashMap::new()),
        })
    }

    pub fn begin(&self) -> Result<AuthorizationRequest> {
        let verifier = random_string(64)?;
        let challenge = code_challenge(&verifier);
        let state = random_string(32)?;

        let redirect = Url::parse(&self.cfg.redirect_uri)?;

        let host = redirect.host_str().unwrap_or("127.0.0.1");
        let port = redirect.port().unwrap_or(0);
        let path = if redirect.path().is_empty() {
            "/"
        } else {
            redirect.path()
        };

        let listen_addr = format!("{}:{}", host, port);
        let server = Server::http(&listen_addr).map_err(|err| anyhow!("auth: listen: {}", err))?;
        let actual_addr = server.server_addr();

        let actual_redirect = Url::parse(&format!("http://{}{}", actual_addr, path))?;
        let auth_url = self.authorize_url(actual_redirect.as_str(), &state, &challenge)?;

        let (result_tx, result_rx) = bounded::<AuthResult>(1);
        let (shutdown_tx, shutdown_rx) = bounded::<()>(1);

        let expected_state = state.clone();

        thread::spawn(move || {
            for request in server.incoming_requests() {
                if shutdown_rx.try_recv().is_ok() {
                    break;
                }
                match handle_redirect(request, &expected_state, &result_tx) {
                    Ok(handled) => {
                        if handled {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = result_tx.send(AuthResult {
                            code: None,
                            error: Some(err),
                        });
                        break;
                    }
                }
            }
        });

        Ok(AuthorizationRequest {
            browser_url: auth_url,
            redirect_uri: actual_redirect.to_string(),
            verifier,
            rx: result_rx,
            shutdown: shutdown_tx,
        })
    }

    fn authorize_url(&self, redirect_uri: &str, state: &str, challenge: &str) -> Result<String> {
        let mut auth = Url::parse(&self.cfg.auth_url)?;
        auth.query_pairs_mut()
            .append_pair("client_id", &self.cfg.client_id)
            .append_pair("response_type", "code")
            .append_pair("state", state)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("duration", "permanent")
            .append_pair("scope", &self.cfg.scope.join(" "))
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(auth.to_string())
    }

    pub fn complete(&self, authz: AuthorizationRequest) -> Result<Session> {
        let code = self.wait_for_code(&authz)?;
        let token = self.exchange_code(&code, &authz)?;
        let identity = self.fetch_identity(&token)?;

        let mut account = Account {
            id: 0,
            reddit_id: identity.id.clone(),
            username: identity.name.clone(),
            display_name: identity.display_name.clone(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let account_id = self.store.upsert_account(account.clone())?;
        account.id = account_id;

        self.persist_token(account_id, &token)?;
        self.start_refresh(account_id, token.clone());

        Ok(Session { account, token })
    }

    pub fn resume(&self, account: Account, stored: Token) -> Result<Session> {
        if account.id == 0 {
            bail!("auth: account id required");
        }
        if stored.access_token.is_empty() || stored.refresh_token.is_empty() {
            bail!("auth: stored token incomplete");
        }

        let scope = if stored.scope.is_empty() {
            self.cfg.scope.clone()
        } else {
            stored.scope.clone()
        };

        let expiry = if stored.expires_at.timestamp() == 0 {
            Utc::now() + chrono::Duration::hours(1)
        } else {
            stored.expires_at
        };

        let token = OAuthTokenDetails {
            access_token: stored.access_token.clone(),
            refresh_token: stored.refresh_token.clone(),
            token_type: stored.token_type.clone(),
            expires_at: expiry,
            scope,
        };

        self.start_refresh(account.id, token.clone());

        Ok(Session { account, token })
    }

    pub fn close(&self) {
        let mut refreshers = self.refreshers.lock();
        for (_, handle) in refreshers.drain() {
            let _ = handle.stop.send(());
            let _ = handle.thread.join();
        }
    }

    fn wait_for_code(&self, authz: &AuthorizationRequest) -> Result<String> {
        match authz.rx.recv() {
            Ok(AuthResult {
                code: Some(code),
                error: None,
            }) => Ok(code),
            Ok(AuthResult {
                code: None,
                error: Some(err),
            }) => Err(err),
            Ok(_) => Err(anyhow!("auth: authorization cancelled")),
            Err(err) => Err(anyhow!("auth: wait error: {}", err)),
        }
    }

    fn exchange_code(&self, code: &str, authz: &AuthorizationRequest) -> Result<OAuthTokenDetails> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", authz.redirect_uri.as_str()),
            ("code_verifier", authz.verifier.as_str()),
        ];
        if self.cfg.client_secret.is_empty() {
            form.push(("client_id", self.cfg.client_id.as_str()));
        }

        let mut req = self
            .client
            .post(&self.cfg.token_url)
            .header(USER_AGENT, self.cfg.user_agent.clone())
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&form);

        let applied_basic_auth = if self.cfg.client_secret.is_empty() {
            false
        } else {
            req = req.basic_auth(&self.cfg.client_id, Some(self.cfg.client_secret.as_str()));
            true
        };

        let resp = req
            .send()
            .with_context(|| format!("auth: token request (basic_auth={})", applied_basic_auth))?;
        if !resp.status().is_success() {
            let body = resp.text().unwrap_or_default();
            if let Ok(err) = serde_json::from_str::<TokenError>(&body) {
                bail!("auth: token request failed: {}", err);
            }
            bail!("auth: token request failed: {}", body);
        }

        let payload: TokenResponse = resp.json().context("auth: decode token response")?;
        if payload.access_token.is_empty() {
            bail!("auth: missing access token");
        }
        if payload.refresh_token.is_empty() {
            bail!("auth: missing refresh token");
        }

        let expires_in = if payload.expires_in == 0 {
            3600
        } else {
            payload.expires_in
        };
        let expires_at = Utc::now() + chrono::Duration::seconds(expires_in as i64);
        let mut scope = scope_list(&payload.scope);
        if scope.is_empty() {
            scope = self.cfg.scope.clone();
        }

        Ok(OAuthTokenDetails {
            access_token: payload.access_token,
            refresh_token: payload.refresh_token,
            token_type: payload.token_type.unwrap_or_else(|| "bearer".into()),
            expires_at,
            scope,
        })
    }

    fn fetch_identity(&self, token: &OAuthTokenDetails) -> Result<Identity> {
        let resp = self
            .client
            .get(&self.cfg.identity_url)
            .header(USER_AGENT, self.cfg.user_agent.clone())
            .header(AUTHORIZATION, format!("Bearer {}", token.access_token))
            .send()
            .context("auth: identity request")?;

        if !resp.status().is_success() {
            let body = resp.text().unwrap_or_default();
            bail!("auth: identity request failed: {}", body);
        }

        let payload: IdentityResponse = resp.json().context("auth: decode identity")?;
        if payload.id.is_empty() || payload.name.is_empty() {
            bail!("auth: identity missing fields");
        }
        let display = if let Some(sub) = payload.subreddit {
            if !sub.display_name_prefixed.is_empty() {
                sub.display_name_prefixed
            } else if !sub.title.is_empty() {
                sub.title
            } else {
                payload.name.clone()
            }
        } else {
            payload.name.clone()
        };

        Ok(Identity {
            id: payload.id,
            name: payload.name,
            display_name: display,
        })
    }

    fn persist_token(&self, account_id: i64, token: &OAuthTokenDetails) -> Result<()> {
        let stored = Token {
            account_id,
            access_token: token.access_token.clone(),
            refresh_token: token.refresh_token.clone(),
            token_type: token.token_type.clone(),
            scope: token.scope.clone(),
            expires_at: token.expires_at,
        };
        self.store.upsert_token(stored)?;
        Ok(())
    }

    fn start_refresh(&self, account_id: i64, token: OAuthTokenDetails) {
        let mut refreshers = self.refreshers.lock();
        if let Some(existing) = refreshers.remove(&account_id) {
            let _ = existing.stop.send(());
            let _ = existing.thread.join();
        }

        let (stop_tx, stop_rx) = unbounded();
        let cfg = self.cfg.clone();
        let store = self.store.clone();
        let client = self.client.clone();

        let handle = thread::spawn(move || {
            let mut current = token.clone();
            loop {
                let wait = next_refresh_delay(&current, cfg.refresh_skew);
                if stop_rx.recv_timeout(wait).is_ok() {
                    break;
                }

                match refresh_token(&client, &cfg, &current) {
                    Ok(new_token) => {
                        let _ = store.upsert_token(Token {
                            account_id,
                            access_token: new_token.access_token.clone(),
                            refresh_token: new_token.refresh_token.clone(),
                            token_type: new_token.token_type.clone(),
                            scope: new_token.scope.clone(),
                            expires_at: new_token.expires_at,
                        });
                        current = new_token;
                    }
                    Err(err) => {
                        eprintln!("token refresh failed: {err:?}");
                        if stop_rx.recv_timeout(Duration::from_secs(5)).is_ok() {
                            break;
                        }
                    }
                }
            }
        });

        refreshers.insert(
            account_id,
            RefreshHandle {
                stop: stop_tx,
                thread: handle,
            },
        );
    }

    pub fn token_provider(&self, account_id: i64) -> Result<Arc<dyn TokenProvider>> {
        let store = self.store.clone();
        Ok(Arc::new(StoreTokenSource { store, account_id }))
    }
}

struct StoreTokenSource {
    store: Arc<storage::Store>,
    account_id: i64,
}

impl TokenProvider for StoreTokenSource {
    fn token(&self) -> Result<OAuthToken> {
        let stored = self
            .store
            .get_token(self.account_id)?
            .ok_or_else(|| anyhow!("token not found"))?;
        Ok(OAuthToken {
            access_token: stored.access_token,
            token_type: stored.token_type,
            expires_at: Some(stored.expires_at.into()),
        })
    }
}

fn next_refresh_delay(token: &OAuthTokenDetails, skew: Duration) -> Duration {
    let expiry = token.expires_at
        - chrono::Duration::from_std(skew).unwrap_or_else(|_| chrono::Duration::seconds(0));
    let now = Utc::now();
    if expiry <= now {
        Duration::from_secs(1)
    } else {
        (expiry - now).to_std().unwrap_or(Duration::from_secs(1))
    }
}

fn refresh_token(
    client: &Client,
    cfg: &Config,
    current: &OAuthTokenDetails,
) -> Result<OAuthTokenDetails> {
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", current.refresh_token.as_str()),
    ];
    if cfg.client_secret.is_empty() {
        form.push(("client_id", cfg.client_id.as_str()));
    }

    let mut req = client
        .post(&cfg.token_url)
        .header(USER_AGENT, cfg.user_agent.clone())
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .form(&form);

    if !cfg.client_secret.is_empty() {
        req = req.basic_auth(&cfg.client_id, Some(cfg.client_secret.as_str()));
    }

    let used_basic = !cfg.client_secret.is_empty();

    let resp = req
        .send()
        .with_context(|| format!("auth: refresh token request (basic_auth={})", used_basic))?;
    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        if let Ok(err) = serde_json::from_str::<TokenError>(&body) {
            bail!("auth: refresh failed: {}", err);
        }
        bail!("auth: refresh failed: {}", body);
    }

    let payload: TokenResponse = resp.json().context("auth: decode refresh response")?;
    if payload.access_token.is_empty() {
        bail!("auth: missing refreshed access token");
    }
    let expires_in = if payload.expires_in == 0 {
        3600
    } else {
        payload.expires_in
    };
    let expires_at = Utc::now() + chrono::Duration::seconds(expires_in as i64);
    let mut scope = scope_list(&payload.scope);
    if scope.is_empty() {
        scope = if current.scope.is_empty() {
            cfg.scope.clone()
        } else {
            current.scope.clone()
        };
    }

    let refresh_token = if !payload.refresh_token.is_empty() {
        payload.refresh_token
    } else if !current.refresh_token.is_empty() {
        current.refresh_token.clone()
    } else {
        bail!("auth: refresh response missing refresh token");
    };

    Ok(OAuthTokenDetails {
        access_token: payload.access_token,
        refresh_token,
        token_type: payload
            .token_type
            .unwrap_or_else(|| current.token_type.clone()),
        expires_at,
        scope,
    })
}

fn scope_list(scope: &str) -> Vec<String> {
    if scope.trim().is_empty() {
        return Vec::new();
    }
    scope
        .split_whitespace()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn random_string(len: usize) -> Result<String> {
    let mut bytes = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hasher.finalize())
}

fn handle_redirect(req: tiny_http::Request, state: &str, tx: &Sender<AuthResult>) -> Result<bool> {
    if req.method() != &Method::Get {
        let _ = req.respond(Response::from_string("method not allowed").with_status_code(405));
        tx.send(AuthResult {
            code: None,
            error: Some(anyhow!("unexpected redirect method")),
        })
        .ok();
        return Ok(true);
    }

    let url = Url::parse(&format!("http://dummy{}", req.url()))?;
    let params: HashMap<_, _> = url.query_pairs().into_owned().collect();
    if params.get("state").map(String::as_str) != Some(state) {
        let _ = req.respond(Response::from_string("state mismatch").with_status_code(400));
        tx.send(AuthResult {
            code: None,
            error: Some(anyhow!("authorization state mismatch")),
        })
        .ok();
        return Ok(true);
    }

    if let Some(error) = params.get("error") {
        let description = params.get("error_description").cloned().unwrap_or_default();
        let _ = req.respond(Response::from_string("authorization denied").with_status_code(401));
        tx.send(AuthResult {
            code: None,
            error: Some(anyhow!("authorization error: {} ({})", error, description)),
        })
        .ok();
        return Ok(true);
    }

    let code = match params.get("code") {
        Some(code) if !code.is_empty() => code.clone(),
        _ => {
            let _ = req.respond(Response::from_string("code missing").with_status_code(400));
            tx.send(AuthResult {
                code: None,
                error: Some(anyhow!("authorization code missing")),
            })
            .ok();
            return Ok(true);
        }
    };

    let response = Response::from_string(HTML_SUCCESS.clone()).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
            .expect("valid header"),
    );
    let _ = req.respond(response);
    tx.send(AuthResult {
        code: Some(code),
        error: None,
    })
    .ok();
    Ok(true)
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    scope: String,
}

#[derive(Debug, Deserialize)]
struct TokenError {
    #[serde(default)]
    error: String,
    #[serde(default, rename = "error_description")]
    description: String,
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.error.is_empty() && self.description.is_empty() {
            return write!(f, "unknown token error");
        }
        if self.description.is_empty() {
            write!(f, "authorization error: {}", self.error)
        } else if self.error.is_empty() {
            write!(f, "authorization error: {}", self.description)
        } else {
            write!(
                f,
                "authorization error: {} ({})",
                self.error, self.description
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct IdentityResponse {
    id: String,
    name: String,
    #[serde(default)]
    subreddit: Option<IdentitySubreddit>,
}

#[derive(Debug, Deserialize)]
struct IdentitySubreddit {
    #[serde(default)]
    display_name_prefixed: String,
    #[serde(default)]
    title: String,
}

struct Identity {
    id: String,
    name: String,
    display_name: String,
}
