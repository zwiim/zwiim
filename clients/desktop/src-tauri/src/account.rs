//! Optional Zwiim account: "Continue with Google" through Supabase Auth.
//!
//! The sign-in runs in the system browser (OAuth PKCE) and returns to a
//! one-shot callback listener on 127.0.0.1. The resulting session is kept in
//! the OS keychain — never in host SQLite, never in web storage — and only
//! the account's id and email reach the UI. Signing out forgets the session
//! here and revokes it upstream when reachable; it never touches the host,
//! its agents, workspaces or sessions.
//!
//! The access token leaves the keychain only for Zwiim Cloud requests made
//! here (`crate::cloud`), refreshed when it is about to expire; the web UI
//! never holds it. A refresh the service refuses (the session was revoked or
//! has lapsed) forgets the session, so the UI asks to sign in again.
//!
//! Supabase's URL and anon key are `ZWIIM_SUPABASE_URL` and
//! `ZWIIM_SUPABASE_ANON_KEY` when the app starts, else the ones built in.
//! The anon key is public by design; no private secret is ever built in.
//! Without them, sign-in is unavailable and the desktop runs in local mode
//! only.

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// How long the browser has to come back before sign-in gives up.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
const KEYCHAIN_SERVICE: &str = "dev.zwiim.desktop";
const KEYCHAIN_ACCOUNT: &str = "account-session";

#[derive(Clone, Debug)]
pub struct AuthConfig {
    /// Supabase project URL, e.g. `https://abc.supabase.co`.
    pub url: String,
    pub anon_key: String,
}

impl AuthConfig {
    /// Sign-in's configuration, if any: the environment's, else the build's.
    pub fn configured() -> Option<Self> {
        let env = |name| std::env::var(name).ok();
        Self::pick(
            (env("ZWIIM_SUPABASE_URL"), env("ZWIIM_SUPABASE_ANON_KEY")),
            (
                option_env!("ZWIIM_SUPABASE_URL").map(str::to_string),
                option_env!("ZWIIM_SUPABASE_ANON_KEY").map(str::to_string),
            ),
        )
    }

    /// The first complete, valid pair of URL and anon key.
    fn pick(runtime: (Option<String>, Option<String>), build: (Option<String>, Option<String>)) -> Option<Self> {
        let pair = |(url, key): (Option<String>, Option<String>)| Self::new(&url?, &key?);
        pair(runtime).or_else(|| pair(build))
    }

    /// Only HTTPS, or plain HTTP to this machine (a local Supabase).
    pub fn new(url: &str, anon_key: &str) -> Option<Self> {
        let url = url.trim().trim_end_matches('/');
        let parsed = url::Url::parse(url).ok()?;
        let local = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
            return None;
        }
        if anon_key.trim().is_empty() {
            return None;
        }
        Some(Self { url: url.to_string(), anon_key: anon_key.trim().to_string() })
    }
}

/// What the UI learns about the signed-in account. Never a token.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub user_id: String,
    pub email: Option<String>,
}

/// The session as kept in the keychain.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub account: Account,
    pub access_token: String,
    pub refresh_token: String,
}

/// Never prints the tokens.
impl std::fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSession").field("account", &self.account).finish_non_exhaustive()
    }
}

/// Where the session lives: the OS keychain in the app, memory in tests.
pub trait SessionStore: Send + Sync {
    fn load(&self) -> Result<Option<StoredSession>, String>;
    fn save(&self, session: &StoredSession) -> Result<(), String>;
    fn clear(&self) -> Result<(), String>;
}

pub struct Keychain;

impl Keychain {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(|e| format!("Keychain unavailable: {e}"))
    }
}

impl SessionStore for Keychain {
    fn load(&self) -> Result<Option<StoredSession>, String> {
        match Self::entry()?.get_password() {
            Ok(raw) => Ok(serde_json::from_str(&raw).ok()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Cannot read the keychain: {e}")),
        }
    }
    fn save(&self, session: &StoredSession) -> Result<(), String> {
        let raw = serde_json::to_string(session).map_err(|e| e.to_string())?;
        Self::entry()?
            .set_password(&raw)
            .map_err(|e| format!("Cannot write the keychain: {e}"))
    }
    fn clear(&self) -> Result<(), String> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Cannot clear the keychain: {e}")),
        }
    }
}

/// The signed-in account, from the keychain alone: works offline.
pub fn current(store: &dyn SessionStore) -> Result<Option<Account>, String> {
    Ok(store.load()?.map(|s| s.account))
}

/// What `access_token` reports when there is no usable session.
pub const SIGN_IN_REQUIRED: &str = "Sign in to reach your hosts";
/// Refresh a token this close to its expiry.
const REFRESH_MARGIN_SECS: u64 = 60;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A JWT's `exp`, read without verifying it: the service verifies; this only
/// decides when to refresh.
fn token_expiry(token: &str) -> Option<u64> {
    let payload = token.split('.').nth(1)?;
    let claims: serde_json::Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    claims.get("exp")?.as_u64()
}

/// A current access token for the signed-in account: the kept one, or a
/// refreshed one when it expires within a minute (or `force`, after a
/// service refused it). Errors with [`SIGN_IN_REQUIRED`] when signed out or
/// the refresh was refused; the session is then forgotten.
pub async fn access_token(config: &AuthConfig, store: &dyn SessionStore, force: bool) -> Result<String, String> {
    let Some(session) = store.load()? else {
        return Err(SIGN_IN_REQUIRED.into());
    };
    let fresh = token_expiry(&session.access_token).is_some_and(|exp| exp > now_secs() + REFRESH_MARGIN_SECS);
    if fresh && !force {
        return Ok(session.access_token);
    }
    let res = http()
        .post(format!("{}/auth/v1/token?grant_type=refresh_token", config.url))
        .header("apikey", &config.anon_key)
        .json(&serde_json::json!({ "refresh_token": session.refresh_token }))
        .send()
        .await
        .map_err(|e| format!("Cannot reach the sign-in service: {e}"))?;
    if res.status().is_client_error() {
        store.clear()?;
        return Err(SIGN_IN_REQUIRED.into());
    }
    if !res.status().is_success() {
        return Err(format!("The sign-in service is unavailable ({})", res.status()));
    }
    let token: TokenResponse = res
        .json()
        .await
        .map_err(|e| format!("Unexpected sign-in response: {e}"))?;
    // The same account, with the rotated tokens.
    let renewed = StoredSession {
        account: session.account,
        access_token: token.access_token,
        refresh_token: token.refresh_token,
    };
    store.save(&renewed)?;
    Ok(renewed.access_token)
}

/// PKCE verifier and its S256 challenge.
fn pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// What came back to the callback listener.
#[derive(Debug, PartialEq)]
enum Callback {
    Code(String),
    Error(String),
}

/// Parse `GET /callback?… HTTP/1.1`; `None` for any other request.
fn parse_callback(request_line: &str) -> Option<Callback> {
    let target = request_line.strip_prefix("GET ")?.split(' ').next()?;
    let url = url::Url::parse(&format!("http://127.0.0.1{target}")).ok()?;
    if url.path() != "/callback" {
        return None;
    }
    let mut code = None;
    let mut error = None;
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "error_description" => error = Some(v.into_owned()),
            "error" if error.is_none() => error = Some(v.into_owned()),
            _ => {}
        }
    }
    match (code, error) {
        (_, Some(e)) => Some(Callback::Error(e)),
        (Some(c), None) if !c.is_empty() => Some(Callback::Code(c)),
        _ => Some(Callback::Error("The sign-in page returned no code".into())),
    }
}

const PAGE: &str = "<!doctype html><meta charset=utf-8><title>Zwiim</title>\
<body style=\"font-family:system-ui;margin:4rem;text-align:center\">";

/// Serve callbacks until one carries a code or an error.
async fn await_callback(listener: TcpListener) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..n]);
        let line = request.lines().next().unwrap_or_default();
        let (status, body, outcome) = match parse_callback(line) {
            None => ("404 Not Found", String::new(), None),
            Some(Callback::Code(code)) => (
                "200 OK",
                format!("{PAGE}<h2>Signed in to Zwiim</h2><p>You can close this tab and return to the app.</p>"),
                Some(Ok(code)),
            ),
            Some(Callback::Error(e)) => (
                "200 OK",
                format!("{PAGE}<h2>Sign-in did not complete</h2><p>You can close this tab and try again from Zwiim.</p>"),
                Some(Err(format!("Sign-in failed: {e}"))),
            ),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        if let Some(outcome) = outcome {
            return outcome;
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    user: TokenUser,
}

#[derive(Deserialize)]
struct TokenUser {
    id: String,
    email: Option<String>,
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .expect("HTTP client")
}

/// Run the whole sign-in: open the provider in the browser (`open_url`),
/// wait for its callback, exchange the code, keep the session.
pub async fn sign_in(
    config: &AuthConfig,
    store: &dyn SessionStore,
    open_url: impl FnOnce(&str) -> Result<(), String>,
    cancel: &Notify,
) -> Result<Account, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Cannot listen for the sign-in callback: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let (verifier, challenge) = pkce();
    let mut authorize = url::Url::parse(&format!("{}/auth/v1/authorize", config.url)).map_err(|e| e.to_string())?;
    authorize
        .query_pairs_mut()
        .append_pair("provider", "google")
        .append_pair("redirect_to", &redirect)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "s256");
    open_url(authorize.as_str())?;

    let code = tokio::select! {
        r = tokio::time::timeout(SIGN_IN_TIMEOUT, await_callback(listener)) => {
            r.map_err(|_| "Sign-in timed out".to_string())??
        }
        _ = cancel.notified() => return Err("Sign-in cancelled".into()),
    };

    let res = http()
        .post(format!("{}/auth/v1/token?grant_type=pkce", config.url))
        .header("apikey", &config.anon_key)
        .json(&serde_json::json!({ "auth_code": code, "code_verifier": verifier }))
        .send()
        .await
        .map_err(|e| format!("Cannot reach the sign-in service: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("The sign-in service refused the sign-in ({})", res.status()));
    }
    let token: TokenResponse = res
        .json()
        .await
        .map_err(|e| format!("Unexpected sign-in response: {e}"))?;
    let session = StoredSession {
        account: Account { user_id: token.user.id, email: token.user.email },
        access_token: token.access_token,
        refresh_token: token.refresh_token,
    };
    store.save(&session)?;
    Ok(session.account)
}

/// Forget the session here first (so sign-out works offline), then revoke
/// it upstream if the service is reachable.
pub async fn sign_out(config: Option<&AuthConfig>, store: &dyn SessionStore) -> Result<(), String> {
    let session = store.load().ok().flatten();
    store.clear()?;
    if let (Some(config), Some(session)) = (config, session) {
        let _ = http()
            .post(format!("{}/auth/v1/logout", config.url))
            .header("apikey", &config.anon_key)
            .bearer_auth(&session.access_token)
            .timeout(Duration::from_secs(5))
            .send()
            .await;
    }
    Ok(())
}

/// Open `url` in the user's default browser.
pub fn open_in_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Cannot open the browser: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Memory(Mutex<Option<StoredSession>>);
    impl SessionStore for Memory {
        fn load(&self) -> Result<Option<StoredSession>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, s: &StoredSession) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(s.clone());
            Ok(())
        }
        fn clear(&self) -> Result<(), String> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn config_requires_https_except_locally() {
        assert!(AuthConfig::new("https://abc.supabase.co/", "anon").is_some());
        assert!(AuthConfig::new("http://127.0.0.1:54321", "anon").is_some());
        assert!(AuthConfig::new("http://abc.supabase.co", "anon").is_none());
        assert!(AuthConfig::new("https://abc.supabase.co", " ").is_none());
        assert_eq!(AuthConfig::new("https://abc.supabase.co/", "anon").unwrap().url, "https://abc.supabase.co");
    }

    #[test]
    fn the_environment_overrides_the_build_only_as_a_whole() {
        let some = |s: &str| Some(s.to_string());
        let built = (some("https://built.supabase.co"), some("built-anon"));
        let picked = AuthConfig::pick((some("https://env.supabase.co"), some("env-anon")), built.clone()).unwrap();
        assert_eq!((picked.url.as_str(), picked.anon_key.as_str()), ("https://env.supabase.co", "env-anon"));
        // Half a pair, or an invalid one, leaves the build's.
        let partial_or_bad =
            [(some("https://env.supabase.co"), None), (None, some("env-anon")), (some("http://env.example"), some("k"))];
        for runtime in partial_or_bad {
            assert_eq!(AuthConfig::pick(runtime, built.clone()).unwrap().url, "https://built.supabase.co");
        }
        assert!(AuthConfig::pick((None, None), (None, None)).is_none());
    }

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        let (v, c) = pkce();
        assert_eq!(v.len(), 43);
        assert_eq!(c, URL_SAFE_NO_PAD.encode(Sha256::digest(v.as_bytes())));
        assert_ne!(pkce().0, v);
    }

    #[test]
    fn parses_callbacks() {
        assert_eq!(parse_callback("GET /callback?code=abc HTTP/1.1"), Some(Callback::Code("abc".into())));
        assert_eq!(
            parse_callback("GET /callback?error=access_denied&error_description=User+said+no HTTP/1.1"),
            Some(Callback::Error("User said no".into()))
        );
        assert!(matches!(parse_callback("GET /callback HTTP/1.1"), Some(Callback::Error(_))));
        assert_eq!(parse_callback("GET /favicon.ico HTTP/1.1"), None);
        assert_eq!(parse_callback("POST /callback?code=abc HTTP/1.1"), None);
    }

    /// A stand-in for Supabase Auth: `/authorize` bounces to `redirect_to`
    /// with a code, `/token` checks the PKCE pair, `/logout` records it.
    async fn fake_supabase(user_email: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        tokio::spawn(async move {
            let mut challenge = String::new();
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 16384];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = req.lines().next().unwrap().to_string();
                seen.lock().unwrap().push(line.clone());
                let target = line.split(' ').nth(1).unwrap();
                let url = url::Url::parse(&format!("http://x{target}")).unwrap();
                let q = |k: &str| url.query_pairs().find(|(a, _)| a == k).map(|(_, v)| v.into_owned());
                let resp = match url.path() {
                    "/auth/v1/authorize" => {
                        challenge = q("code_challenge").unwrap();
                        assert_eq!(q("provider").as_deref(), Some("google"));
                        format!("HTTP/1.1 302 Found\r\nLocation: {}?code=the-code\r\nContent-Length: 0\r\n\r\n", q("redirect_to").unwrap())
                    }
                    "/auth/v1/token" if q("grant_type").as_deref() == Some("refresh_token") => {
                        let body: serde_json::Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                        let (status, body) = if body["refresh_token"] == "rt" {
                            let access = jwt_expiring(now_secs() + 3600);
                            ("200 OK", serde_json::json!({"access_token":access,"refresh_token":"rt2","expires_in":3600,"user":{"id":"user-1","email":user_email}}).to_string())
                        } else {
                            ("400 Bad Request", r#"{"error":"invalid_grant"}"#.to_string())
                        };
                        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                    }
                    "/auth/v1/token" => {
                        assert!(req.to_lowercase().contains("apikey: anon"));
                        let body: serde_json::Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                        let verifier = body["code_verifier"].as_str().unwrap();
                        let ok = body["auth_code"] == "the-code"
                            && URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge;
                        let body = if ok {
                            serde_json::json!({"access_token":"at","refresh_token":"rt","expires_in":3600,"user":{"id":"user-1","email":user_email}}).to_string()
                        } else {
                            "{}".to_string()
                        };
                        let status = if ok { "200 OK" } else { "400 Bad Request" };
                        format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                    }
                    _ => "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_string(),
                };
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        (base, log)
    }

    /// The browser: follows the authorize redirect to the callback.
    fn browser(url: &str) -> Result<(), String> {
        let url = url.to_string();
        tokio::spawn(async move {
            reqwest::get(url).await.unwrap();
        });
        Ok(())
    }

    /// Touches the real OS keychain: run by hand with `--ignored`.
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let before = Keychain.load().unwrap();
        let s = StoredSession {
            account: Account { user_id: "test-user".into(), email: Some("t@example.com".into()) },
            access_token: "at".into(),
            refresh_token: "rt".into(),
        };
        Keychain.save(&s).unwrap();
        assert_eq!(current(&Keychain).unwrap(), Some(s.account.clone()));
        Keychain.clear().unwrap();
        assert_eq!(current(&Keychain).unwrap(), None);
        Keychain.clear().unwrap();
        if let Some(before) = before {
            Keychain.save(&before).unwrap();
        }
    }

    #[tokio::test]
    async fn signs_in_keeps_the_session_and_signs_out() {
        let (base, log) = fake_supabase("me@example.com").await;
        let config = AuthConfig::new(&base, "anon").unwrap();
        let store = Memory::default();
        let account = sign_in(&config, &store, browser, &Notify::new()).await.unwrap();
        assert_eq!(account, Account { user_id: "user-1".into(), email: Some("me@example.com".into()) });
        let kept = store.load().unwrap().unwrap();
        assert_eq!((kept.access_token.as_str(), kept.refresh_token.as_str()), ("at", "rt"));
        // Offline: the account comes from the store alone.
        assert_eq!(current(&store).unwrap(), Some(account));

        sign_out(Some(&config), &store).await.unwrap();
        assert_eq!(current(&store).unwrap(), None);
        assert!(log.lock().unwrap().iter().any(|l| l.starts_with("POST /auth/v1/logout")));
    }

    /// An unsigned token carrying only `exp`.
    fn jwt_expiring(exp: u64) -> String {
        format!("h.{}.s", URL_SAFE_NO_PAD.encode(serde_json::json!({ "exp": exp }).to_string()))
    }

    fn signed_in(store: &Memory, access_token: String, refresh_token: &str) {
        store
            .save(&StoredSession {
                account: Account { user_id: "user-1".into(), email: None },
                access_token,
                refresh_token: refresh_token.into(),
            })
            .unwrap();
    }

    #[tokio::test]
    async fn access_tokens_are_refreshed_near_expiry() {
        let (base, log) = fake_supabase("me@example.com").await;
        let config = AuthConfig::new(&base, "anon").unwrap();
        let refreshes = || log.lock().unwrap().iter().filter(|l| l.contains("grant_type=refresh_token")).count();

        // Signed out: nothing to send.
        let store = Memory::default();
        assert_eq!(access_token(&config, &store, false).await.unwrap_err(), SIGN_IN_REQUIRED);

        // Current: used as is.
        let current = jwt_expiring(now_secs() + 3600);
        signed_in(&store, current.clone(), "rt");
        assert_eq!(access_token(&config, &store, false).await.unwrap(), current);
        assert_eq!(refreshes(), 0);

        // About to expire: refreshed, and the rotated tokens are kept.
        signed_in(&store, jwt_expiring(now_secs() + 10), "rt");
        let renewed = access_token(&config, &store, false).await.unwrap();
        assert!(token_expiry(&renewed).unwrap() > now_secs() + 3000);
        let kept = store.load().unwrap().unwrap();
        assert_eq!((kept.access_token.as_str(), kept.refresh_token.as_str()), (renewed.as_str(), "rt2"));
        assert_eq!(kept.account.user_id, "user-1");
        assert_eq!(refreshes(), 1);

        // Refused by a service (forced): the refresh runs even for a current token.
        signed_in(&store, current, "rt");
        access_token(&config, &store, true).await.unwrap();
        assert_eq!(refreshes(), 2);
    }

    #[tokio::test]
    async fn a_refused_refresh_signs_out() {
        let (base, _) = fake_supabase("me@example.com").await;
        let config = AuthConfig::new(&base, "anon").unwrap();
        let store = Memory::default();
        signed_in(&store, jwt_expiring(1), "revoked");
        assert_eq!(access_token(&config, &store, false).await.unwrap_err(), SIGN_IN_REQUIRED);
        assert_eq!(current(&store).unwrap(), None);

        // Unreachable: the session is kept for later.
        let offline = AuthConfig::new("http://127.0.0.1:9", "anon").unwrap();
        signed_in(&store, jwt_expiring(1), "rt");
        assert!(access_token(&offline, &store, false).await.unwrap_err().contains("Cannot reach"));
        assert!(current(&store).unwrap().is_some());
    }

    #[tokio::test]
    async fn sign_out_works_when_the_service_is_unreachable() {
        let store = Memory::default();
        store
            .save(&StoredSession {
                account: Account { user_id: "u".into(), email: None },
                access_token: "at".into(),
                refresh_token: "rt".into(),
            })
            .unwrap();
        let config = AuthConfig::new("http://127.0.0.1:9", "anon").unwrap();
        sign_out(Some(&config), &store).await.unwrap();
        assert_eq!(current(&store).unwrap(), None);
    }

    #[tokio::test]
    async fn a_provider_error_or_cancel_keeps_nothing() {
        let store = Memory::default();
        let config = AuthConfig::new("http://127.0.0.1:9", "anon").unwrap();
        let err = sign_in(
            &config,
            &store,
            |url| {
                let redirect = url::Url::parse(url)
                    .unwrap()
                    .query_pairs()
                    .find(|(k, _)| k == "redirect_to")
                    .unwrap()
                    .1
                    .into_owned();
                tokio::spawn(async move {
                    reqwest::get(format!("{redirect}?error=access_denied&error_description=Denied")).await.unwrap();
                });
                Ok(())
            },
            &Notify::new(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("Denied"), "{err}");
        assert_eq!(current(&store).unwrap(), None);

        let cancel = Arc::new(Notify::new());
        let c = cancel.clone();
        let pending = tokio::spawn(async move {
            let store = Memory::default();
            sign_in(&config, &store, |_| Ok(()), &c).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.notify_waiters();
        assert_eq!(pending.await.unwrap().unwrap_err(), "Sign-in cancelled");
    }
}
