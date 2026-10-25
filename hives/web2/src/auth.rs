//! Who web2 is to each service: the tokens it holds, and how it gets them.
//!
//! Tokens live in one file, `secrets/credentials.json` in web2's state
//! directory, mode 0600 in a 0700 directory, written whole and renamed into
//! place. `secrets/` is the directory a builder's shell is kept out of. A token is never put in
//! an environment, never logged, and never answered back. The serving bee
//! reads the file again whenever it changes, so `web2 login` in a terminal
//! takes effect on the next call without a restart.
//!
//! A token is stored by `web2 login <service>` (typed at a prompt, or
//! piped in), or read from the environment web2 runs in when the config
//! says `env`. An OpenAPI service with an `oauth` app signs in with the
//! authorization code flow and PKCE, through a callback on 127.0.0.1, and
//! its token is refreshed before it expires. An MCP server's own OAuth is
//! kept here too, through [`McpStore`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use parking_lot::Mutex;
use rand::Rng;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::config::{Auth, OAuthApp, Secret, Service};

pub const CREDENTIALS_FILE: &str = "credentials.json";
pub const SECRETS_DIR: &str = "secrets";
const REFRESH_AHEAD: u64 = 60;
const CALLBACK_WAIT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq)]
pub enum Denied {
    NeedsLogin(String),
    Failed(String),
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denied::NeedsLogin(why) | Denied::Failed(why) => f.write_str(why),
        }
    }
}

pub fn needs_login(service: &str, why: &str) -> Denied {
    Denied::NeedsLogin(format!("{service} {why}: a person runs `{} login {service}` in a terminal on this machine", cli()))
}

pub fn cli() -> String {
    let Ok(me) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return "web2".into();
    };
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).map(|d| d.join("web2")).find_map(|c| c.canonicalize().ok())
    });
    if on_path.as_deref() == Some(me.as_path()) { "web2".into() } else { me.display().to_string() }
}

pub struct Creds {
    path: PathBuf,
    cache: Mutex<(Option<SystemTime>, Map<String, Value>)>,
    refreshing: tokio::sync::Mutex<()>,
}

impl Creds {
    pub fn at(state: &Path) -> Arc<Creds> {
        Arc::new(Creds {
            path: state.join(SECRETS_DIR).join(CREDENTIALS_FILE),
            cache: Mutex::new((None, Map::new())),
            refreshing: tokio::sync::Mutex::new(()),
        })
    }

    fn read(&self) -> Map<String, Value> {
        let modified = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        let mut cache = self.cache.lock();
        if modified.is_some() && cache.0 == modified {
            return cache.1.clone();
        }
        let map = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        *cache = (modified, map.clone());
        map
    }

    pub fn get(&self, service: &str) -> Option<Value> {
        self.read().get(service).cloned()
    }

    pub fn has(&self, service: &str) -> bool {
        self.get(service).is_some()
    }

    pub fn put(&self, service: &str, entry: Value) -> Result<()> {
        let mut map = self.read();
        map.insert(service.to_string(), entry);
        self.write(map)
    }

    pub fn remove(&self, service: &str) -> Result<bool> {
        let mut map = self.read();
        let had = map.remove(service).is_some();
        if had {
            self.write(map)?;
        }
        Ok(had)
    }

    fn write(&self, map: Map<String, Value>) -> Result<()> {
        let dir = self.path.parent().ok_or_else(|| anyhow!("credentials have no directory"))?;
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let staged = dir.join(format!(".{CREDENTIALS_FILE}.{}", std::process::id()));
        let text = serde_json::to_string_pretty(&Value::Object(map.clone()))?;
        write_private(&staged, text.as_bytes())?;
        std::fs::rename(&staged, &self.path).with_context(|| format!("place {}", self.path.display()))?;
        let modified = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        *self.cache.lock() = (modified, map);
        Ok(())
    }
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).with_context(|| format!("open {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Applied {
    Header(String, String),
    Query(String, String),
}

fn secret(service: &str, secret: &Secret, creds: &Creds) -> Result<String, Denied> {
    match secret {
        Secret::Env(var) => std::env::var(var).map_err(|_| {
            Denied::Failed(format!("{service} reads its token from ${var}, which is not set where web2 runs"))
        }),
        Secret::Stored => creds
            .get(service)
            .and_then(|e| e.get("token").and_then(Value::as_str).map(str::to_string))
            .ok_or_else(|| needs_login(service, "has no token yet")),
    }
}

pub async fn apply(service: &Service, auth: &Auth, creds: &Creds, http: &reqwest::Client) -> Result<Vec<Applied>, Denied> {
    let name = &service.name;
    Ok(match auth {
        Auth::Bearer(s) => vec![Applied::Header("Authorization".into(), format!("Bearer {}", secret(name, s, creds)?))],
        Auth::Header { name: header, secret: s } => vec![Applied::Header(header.clone(), secret(name, s, creds)?)],
        Auth::Query { name: param, secret: s } => vec![Applied::Query(param.clone(), secret(name, s, creds)?)],
        Auth::Basic { username, secret: s } => {
            let pair = format!("{username}:{}", secret(name, s, creds)?);
            vec![Applied::Header("Authorization".into(), format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(pair)))]
        }
        Auth::OAuth2(app) => vec![Applied::Header("Authorization".into(), format!("Bearer {}", oauth_token(name, app, creds, http).await?))],
    })
}

async fn oauth_token(service: &str, app: &OAuthApp, creds: &Creds, http: &reqwest::Client) -> Result<String, Denied> {
    let stored = |creds: &Creds| creds.get(service).and_then(|e| e.get("oauth").cloned());
    let fresh = |entry: &Value| {
        let expires = entry.get("expiresAt").and_then(Value::as_u64);
        let token = entry.get("accessToken").and_then(Value::as_str)?;
        (expires.is_none_or(|at| at > now() + REFRESH_AHEAD)).then(|| token.to_string())
    };
    let entry = stored(creds).ok_or_else(|| needs_login(service, "is not signed in"))?;
    if let Some(token) = fresh(&entry) {
        return Ok(token);
    }
    let _guard = creds.refreshing.lock().await;
    let entry = stored(creds).ok_or_else(|| needs_login(service, "is not signed in"))?;
    if let Some(token) = fresh(&entry) {
        return Ok(token);
    }
    let Some(refresh) = entry.get("refreshToken").and_then(Value::as_str) else {
        return Err(needs_login(service, "signed in once, and that sign-in has expired"));
    };
    let token_url = app.token_url.as_deref().ok_or_else(|| Denied::Failed(format!("{service}'s oauth has no tokenUrl")))?;
    let mut form = vec![("grant_type", "refresh_token".to_string()), ("refresh_token", refresh.to_string()), ("client_id", app.client_id.clone())];
    if let Some(secret) = &app.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let renewed = token_request(http, token_url, &form).await.map_err(|e| match e {
        TokenError::Rejected(why) => needs_login(service, &format!("could not renew its sign-in ({why})")),
        TokenError::Unreachable(why) => Denied::Failed(format!("{service}: renewing the sign-in failed: {why}")),
    })?;
    let merged = merge_token(&entry, &renewed);
    creds.put(service, json!({ "oauth": merged.clone() })).map_err(|e| Denied::Failed(e.to_string()))?;
    merged
        .get("accessToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Denied::Failed(format!("{service}: the renewed sign-in carried no access token")))
}

fn merge_token(old: &Value, response: &Value) -> Value {
    let mut entry = token_entry(response);
    if entry.get("refreshToken").is_none()
        && let Some(r) = old.get("refreshToken")
    {
        entry["refreshToken"] = r.clone();
    }
    entry
}

fn token_entry(response: &Value) -> Value {
    let mut entry = json!({
        "accessToken": response.get("access_token").cloned().unwrap_or(Value::Null),
        "tokenType": response.get("token_type").cloned().unwrap_or(json!("Bearer")),
        "receivedAt": now(),
    });
    if let Some(r) = response.get("refresh_token") {
        entry["refreshToken"] = r.clone();
    }
    if let Some(s) = response.get("scope") {
        entry["scope"] = s.clone();
    }
    let expires_in = response.get("expires_in").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    if let Some(secs) = expires_in {
        entry["expiresAt"] = json!(now() + secs);
    }
    entry
}

enum TokenError {
    Rejected(String),
    Unreachable(String),
}

async fn token_request(http: &reqwest::Client, url: &str, form: &[(&str, String)]) -> Result<Value, TokenError> {
    let resp = http
        .post(url)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|e| TokenError::Unreachable(e.to_string()))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| TokenError::Unreachable(e.to_string()))?;
    let body = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| form_to_json(&text));
    if !status.is_success() || body.get("error").is_some() {
        let why = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(TokenError::Rejected(why));
    }
    if body.get("access_token").and_then(Value::as_str).is_none() {
        return Err(TokenError::Rejected("the answer carried no access_token".into()));
    }
    Ok(body)
}

fn form_to_json(text: &str) -> Value {
    let Ok(url) = reqwest::Url::parse(&format!("http://form.invalid/?{}", text.trim())) else {
        return Value::Null;
    };
    Value::Object(url.query_pairs().map(|(k, v)| (k.into_owned(), Value::String(v.into_owned()))).collect())
}

pub fn random_text(len: usize) -> String {
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut rng = rand::rng();
    (0..len).map(|_| UNRESERVED[rng.random_range(0..UNRESERVED.len())] as char).collect()
}

pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn redirect_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}/callback")
}

pub fn authorize_url(app: &OAuthApp, authorize: &str, state: &str, challenge: &str) -> Result<String> {
    let mut params = vec![
        ("response_type", "code".to_string()),
        ("client_id", app.client_id.clone()),
        ("redirect_uri", redirect_uri(app.redirect_port)),
        ("state", state.to_string()),
        ("code_challenge", challenge.to_string()),
        ("code_challenge_method", "S256".to_string()),
    ];
    if !app.scopes.is_empty() {
        params.push(("scope", app.scopes.join(" ")));
    }
    Ok(reqwest::Url::parse_with_params(authorize, &params).with_context(|| format!("authorizeUrl {authorize:?}"))?.to_string())
}

pub async fn login_oauth(
    service: &str,
    app: &OAuthApp,
    creds: &Creds,
    http: &reqwest::Client,
    present: impl FnOnce(&str),
) -> Result<()> {
    let authorize = app.authorize_url.as_deref().ok_or_else(|| anyhow!("{service}'s oauth has no authorizeUrl, in web2.json or its spec"))?;
    let token_url = app.token_url.as_deref().ok_or_else(|| anyhow!("{service}'s oauth has no tokenUrl, in web2.json or its spec"))?;
    let verifier = random_text(64);
    let state = random_text(32);
    let url = authorize_url(app, authorize, &state, &pkce_challenge(&verifier))?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", app.redirect_port))
        .await
        .with_context(|| format!("listen for the sign-in callback on 127.0.0.1:{}", app.redirect_port))?;
    present(&url);
    let callback = await_callback(listener).await?;
    if callback.get("state").map(String::as_str) != Some(state.as_str()) {
        bail!("the callback's state does not match this sign-in; nothing was stored");
    }
    let code = callback.get("code").ok_or_else(|| anyhow!("the callback carried no code"))?;
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.clone()),
        ("redirect_uri", redirect_uri(app.redirect_port)),
        ("client_id", app.client_id.clone()),
        ("code_verifier", verifier),
    ];
    if let Some(secret) = &app.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let token = token_request(http, token_url, &form).await.map_err(|e| match e {
        TokenError::Rejected(why) | TokenError::Unreachable(why) => anyhow!("{service} did not hand over a token: {why}"),
    })?;
    creds.put(service, json!({ "oauth": token_entry(&token) }))?;
    Ok(())
}

pub fn show_sign_in(service: &str, url: &str) {
    eprintln!("Sign {service} in at:\n\n  {url}\n");
    if let Some(browser) = std::env::var_os("WEB2_BROWSER") {
        match std::process::Command::new(&browser).arg(url).stdin(std::process::Stdio::null()).spawn() {
            Ok(_) => eprintln!("(handed to $WEB2_BROWSER; waiting for it to come back here)"),
            Err(e) => eprintln!("($WEB2_BROWSER would not start: {e}; open the link yourself)"),
        }
        return;
    }
    if open::that(url).is_ok() {
        eprintln!("(opened in your browser; waiting for it to come back here)");
    } else {
        eprintln!("(open it in a browser on this machine; waiting for it to come back here)");
    }
}

pub async fn await_callback(listener: tokio::net::TcpListener) -> Result<HashMap<String, String>> {
    tokio::time::timeout(CALLBACK_WAIT, async {
        loop {
            let (mut sock, _) = listener.accept().await?;
            let mut buf = vec![0u8; 8192];
            let mut len = 0;
            while len < buf.len() {
                let n = sock.read(&mut buf[len..]).await?;
                if n == 0 {
                    break;
                }
                len += n;
                if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf[..len]);
            let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("");
            let Some(query) = target.strip_prefix("/callback") else {
                let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                continue;
            };
            let params: HashMap<String, String> = reqwest::Url::parse(&format!("http://127.0.0.1{query}"))
                .map(|u| u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect())
                .unwrap_or_default();
            let page = if params.contains_key("error") {
                "<p>web2 was not signed in. You can close this tab.</p>"
            } else {
                "<p>web2 is signed in. You can close this tab.</p>"
            };
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
            if let Some(err) = params.get("error") {
                let why = params.get("error_description").cloned().unwrap_or_else(|| err.clone());
                bail!("the sign-in was refused: {why}");
            }
            return Ok(params);
        }
    })
    .await
    .map_err(|_| anyhow!("no sign-in came back within {} minutes", CALLBACK_WAIT.as_secs() / 60))?
}

pub fn store_token(service: &str, token: &str, creds: &Creds) -> Result<()> {
    let token = token.trim();
    if token.is_empty() {
        bail!("an empty token signs nothing in");
    }
    creds.put(service, json!({ "token": token }))
}

pub struct McpStore {
    pub creds: Arc<Creds>,
    pub service: String,
}

#[async_trait::async_trait]
impl rmcp::transport::auth::CredentialStore for McpStore {
    async fn load(&self) -> Result<Option<rmcp::transport::auth::StoredCredentials>, rmcp::transport::auth::AuthError> {
        let Some(entry) = self.creds.get(&self.service).and_then(|e| e.get("mcpOauth").cloned()) else {
            return Ok(None);
        };
        serde_json::from_value(entry)
            .map(Some)
            .map_err(|e| rmcp::transport::auth::AuthError::CredentialStoreError(e.to_string()))
    }

    async fn save(&self, credentials: rmcp::transport::auth::StoredCredentials) -> Result<(), rmcp::transport::auth::AuthError> {
        let entry = serde_json::to_value(&credentials).map_err(|e| rmcp::transport::auth::AuthError::CredentialStoreError(e.to_string()))?;
        self.creds
            .put(&self.service, json!({ "mcpOauth": entry }))
            .map_err(|e| rmcp::transport::auth::AuthError::CredentialStoreError(e.to_string()))
    }

    async fn clear(&self) -> Result<(), rmcp::transport::auth::AuthError> {
        self.creds
            .remove(&self.service)
            .map(|_| ())
            .map_err(|e| rmcp::transport::auth::AuthError::CredentialStoreError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str) -> Service {
        Service {
            name: name.into(),
            kind: crate::config::Kind::OpenApi { spec: "x".into(), base: None },
            description: None,
            writes: false,
            allow: vec![],
            deny: vec![],
            auth: None,
            timeout_ms: None,
        }
    }

    #[test]
    fn the_file_is_private_and_a_change_on_disk_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        let creds = Creds::at(dir.path());
        store_token("github", " ghp_abc \n", &creds).unwrap();
        let path = dir.path().join(SECRETS_DIR).join(CREDENTIALS_FILE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        assert_eq!(creds.get("github").unwrap()["token"], "ghp_abc");
        std::thread::sleep(Duration::from_millis(20));
        let other = Creds::at(dir.path());
        other.put("github", json!({ "token": "ghp_new" })).unwrap();
        assert_eq!(creds.get("github").unwrap()["token"], "ghp_new");
        assert!(creds.remove("github").unwrap());
        assert!(!creds.has("github"));
    }

    #[tokio::test]
    async fn each_kind_of_auth_lands_where_it_belongs() {
        let dir = tempfile::tempdir().unwrap();
        let creds = Creds::at(dir.path());
        let http = reqwest::Client::new();
        let svc = service("api");
        let none = apply(&svc, &Auth::Bearer(Secret::Stored), &creds, &http).await.unwrap_err();
        assert!(matches!(&none, Denied::NeedsLogin(why) if why.contains(" login api`")), "{none:?}");
        store_token("api", "t0k", &creds).unwrap();
        assert_eq!(
            apply(&svc, &Auth::Bearer(Secret::Stored), &creds, &http).await.unwrap(),
            [Applied::Header("Authorization".into(), "Bearer t0k".into())]
        );
        assert_eq!(
            apply(&svc, &Auth::Query { name: "key".into(), secret: Secret::Stored }, &creds, &http).await.unwrap(),
            [Applied::Query("key".into(), "t0k".into())]
        );
        assert_eq!(
            apply(&svc, &Auth::Basic { username: "me".into(), secret: Secret::Stored }, &creds, &http).await.unwrap(),
            [Applied::Header("Authorization".into(), "Basic bWU6dDBr".into())]
        );
        let unset = apply(&svc, &Auth::Bearer(Secret::Env("WEB2_TEST_NO_SUCH_TOKEN".into())), &creds, &http).await.unwrap_err();
        assert!(unset.to_string().contains("$WEB2_TEST_NO_SUCH_TOKEN"), "{unset}");
    }

    #[test]
    fn pkce_follows_rfc_7636() {
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let v = random_text(64);
        assert_eq!(v.len(), 64);
        assert!(v.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b)));
    }

    #[test]
    fn the_authorize_url_carries_the_whole_request() {
        let app = OAuthApp {
            client_id: "cid".into(),
            client_secret: None,
            authorize_url: None,
            token_url: None,
            scopes: vec!["repo".into(), "read:org".into()],
            redirect_port: 8976,
        };
        let url = authorize_url(&app, "https://example.com/authorize", "st", "ch").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let q: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "cid");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:8976/callback");
        assert_eq!(q["scope"], "repo read:org");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], "st");
    }

    #[test]
    fn a_token_answer_becomes_an_entry_and_a_refresh_keeps_the_old_refresh_token() {
        let first = token_entry(&json!({ "access_token": "a1", "refresh_token": "r1", "expires_in": 3600, "token_type": "bearer" }));
        assert_eq!(first["accessToken"], "a1");
        assert!(first["expiresAt"].as_u64().unwrap() > now());
        let renewed = merge_token(&first, &json!({ "access_token": "a2", "expires_in": "60" }));
        assert_eq!(renewed["accessToken"], "a2");
        assert_eq!(renewed["refreshToken"], "r1");
        assert_eq!(form_to_json("access_token=x&scope=repo")["access_token"], "x");
    }

    #[tokio::test]
    async fn the_callback_answers_the_browser_and_returns_the_code() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let waiting = tokio::spawn(await_callback(listener));
        let http = reqwest::Client::new();
        let miss = http.get(format!("http://127.0.0.1:{port}/favicon.ico")).send().await.unwrap();
        assert_eq!(miss.status(), 404);
        let page = http.get(format!("http://127.0.0.1:{port}/callback?code=c0de&state=s1")).send().await.unwrap();
        assert!(page.text().await.unwrap().contains("signed in"));
        let params = waiting.await.unwrap().unwrap();
        assert_eq!((params["code"].as_str(), params["state"].as_str()), ("c0de", "s1"));
    }
    #[tokio::test]
    async fn an_oauth_app_signs_in_with_pkce_and_renews_its_token() {
        use axum::{Form, Json, routing::post};
        let seen: Arc<Mutex<Vec<HashMap<String, String>>>> = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let app_router = axum::Router::new().route(
            "/token",
            post(move |Form(form): Form<HashMap<String, String>>| {
                let log = log.clone();
                async move {
                    let answer = match form.get("grant_type").map(String::as_str) {
                        Some("authorization_code") => json!({ "access_token": "at1", "refresh_token": "rt1", "expires_in": 1, "token_type": "bearer" }),
                        Some("refresh_token") => json!({ "access_token": "at2", "expires_in": 3600 }),
                        _ => json!({ "error": "unsupported_grant_type" }),
                    };
                    log.lock().push(form);
                    Json(answer)
                }
            }),
        );
        let server = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let token_url = format!("http://{}/token", server.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(server, app_router).await.unwrap() });
        let port = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap().local_addr().unwrap().port();
        let app = OAuthApp {
            client_id: "cid".into(),
            client_secret: Some("shh".into()),
            authorize_url: Some("https://auth.example/authorize".into()),
            token_url: Some(token_url),
            scopes: vec!["read".into()],
            redirect_port: port,
        };
        let dir = tempfile::tempdir().unwrap();
        let creds = Creds::at(dir.path());
        let http = reqwest::Client::new();
        let (tx, rx) = tokio::sync::oneshot::channel::<String>();
        let browser = tokio::spawn(async move {
            let url = reqwest::Url::parse(&rx.await.unwrap()).unwrap();
            let q: HashMap<String, String> = url.query_pairs().into_owned().collect();
            let back = format!("{}?code=c0de&state={}", q["redirect_uri"], q["state"]);
            reqwest::get(back).await.unwrap();
            q["code_challenge"].clone()
        });
        login_oauth("api", &app, &creds, &http, move |url| {
            let _ = tx.send(url.to_string());
        })
        .await
        .unwrap();
        let challenge = browser.await.unwrap();
        let exchange = seen.lock()[0].clone();
        assert_eq!(exchange["code"], "c0de");
        assert_eq!(exchange["client_secret"], "shh");
        assert_eq!(pkce_challenge(&exchange["code_verifier"]), challenge);
        assert_eq!(creds.get("api").unwrap()["oauth"]["accessToken"], "at1");

        let svc = service("api");
        let applied = apply(&svc, &Auth::OAuth2(app), &creds, &http).await.unwrap();
        assert_eq!(applied, [Applied::Header("Authorization".into(), "Bearer at2".into())]);
        assert_eq!(seen.lock()[1]["refresh_token"], "rt1");
        let stored = creds.get("api").unwrap()["oauth"].clone();
        assert_eq!((stored["accessToken"].as_str(), stored["refreshToken"].as_str()), (Some("at2"), Some("rt1")));
    }
}
