//! Zwiim Cloud requests, made here on the signed-in account's behalf.
//!
//! The web UI reaches the Cloud's account API only through [`request`]: it
//! names a method and path, and this adds the account's access token (kept
//! in the keychain, refreshed by `account::access_token`), so the token never
//! enters the webview. Only the three calls a device makes are allowed —
//! listing the account's hosts, a connection ticket for one, and a host
//! enrollment token. The relay socket itself is opened by the UI with a
//! ticket, which is single-use and names one host.
//!
//! The Cloud's URL is `ZWIIM_CLOUD_URL` when the app starts, else the one
//! built in; without either, remote hosts are unavailable.

use std::future::Future;
use std::time::Duration;

use serde::Serialize;

use crate::account::SIGN_IN_REQUIRED;

/// A Cloud response, passed to the UI as is: the SDK reads its status and
/// error code.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CloudReply {
    pub status: u16,
    pub body: serde_json::Value,
}

/// https, or http to this machine; an origin only.
pub fn normalize_url(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && local);
    let bare = url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    (scheme_ok && bare && url.host_str().is_some()).then(|| url.origin().ascii_serialization())
}

/// The Cloud this desktop uses, if any.
pub fn configured_url() -> Option<String> {
    std::env::var("ZWIIM_CLOUD_URL")
        .ok()
        .or_else(|| option_env!("ZWIIM_CLOUD_URL").map(str::to_string))
        .and_then(|u| normalize_url(&u))
}

pub(crate) fn valid_host_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The device calls, and nothing else.
fn allowed(method: &str, path: &str) -> bool {
    match (method, path) {
        ("GET", "/v1/hosts") | ("POST", "/v1/enrollments") => true,
        ("POST", p) => p
            .strip_prefix("/v1/hosts/")
            .and_then(|rest| rest.strip_suffix("/tickets"))
            .is_some_and(valid_host_id),
        _ => false,
    }
}

fn unauthenticated() -> CloudReply {
    CloudReply {
        status: 401,
        body: serde_json::json!({ "error": { "code": "unauthenticated", "message": SIGN_IN_REQUIRED } }),
    }
}

/// Make one allowed Cloud call with the account's token. `token(force)`
/// yields it (`force` after the Cloud refused it: refresh). A refused token
/// is refreshed and the call tried once more; signed out, the reply is a
/// 401 `unauthenticated`. `Err` only when the Cloud cannot be reached.
pub async fn request<F, Fut>(cloud_url: &str, token: F, method: &str, path: &str) -> Result<CloudReply, String>
where
    F: Fn(bool) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    if !allowed(method, path) {
        return Err(format!("Not a Zwiim Cloud request: {method} {path}"));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;
    let mut force = false;
    loop {
        let bearer = match token(force).await {
            Ok(t) => t,
            Err(e) if e == SIGN_IN_REQUIRED => return Ok(unauthenticated()),
            Err(e) => return Err(e),
        };
        let builder = if method == "GET" { client.get(format!("{cloud_url}{path}")) } else { client.post(format!("{cloud_url}{path}")) };
        let res = builder
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|_| "Zwiim Cloud unreachable".to_string())?;
        let status = res.status().as_u16();
        if status == 401 && !force {
            force = true;
            continue;
        }
        let text = res.text().await.map_err(|_| "Zwiim Cloud unreachable".to_string())?;
        let body = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        return Ok(CloudReply { status, body });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn urls() {
        assert_eq!(normalize_url("https://cloud.zwiim.dev/").as_deref(), Some("https://cloud.zwiim.dev"));
        assert_eq!(normalize_url("http://127.0.0.1:8080").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(normalize_url("http://cloud.zwiim.dev"), None);
        assert_eq!(normalize_url("https://cloud.zwiim.dev/api"), None);
        assert_eq!(normalize_url("https://u:p@cloud.zwiim.dev"), None);
        assert_eq!(normalize_url("https://cloud.zwiim.dev/?x=1"), None);
    }

    #[test]
    fn only_device_calls_are_allowed() {
        assert!(allowed("GET", "/v1/hosts"));
        assert!(allowed("POST", "/v1/hosts/87040963-abc_D/tickets"));
        assert!(allowed("POST", "/v1/enrollments"));
        assert!(!allowed("POST", "/v1/hosts"));
        assert!(!allowed("GET", "/v1/enrollments"));
        assert!(!allowed("POST", "/v1/hosts//tickets"));
        assert!(!allowed("POST", "/v1/hosts/../admin/tickets"));
        assert!(!allowed("POST", "/v1/hosts/a/b/tickets"));
        assert!(!allowed("GET", "/v1/relay/device"));
        assert!(!allowed("DELETE", "/v1/hosts"));
    }

    /// A Cloud that accepts only `Bearer good`, recording what it was sent.
    async fn fake_cloud() -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = log.clone();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                let line = req.lines().next().unwrap().to_string();
                let auth = req.lines().find_map(|l| l.strip_prefix("authorization: ")).unwrap_or("").to_string();
                seen.lock().unwrap().push(format!("{line} [{auth}]"));
                let (status, body) = if auth != "bearer good" {
                    ("401 Unauthorized", r#"{"error":{"code":"unauthenticated"}}"#)
                } else {
                    ("200 OK", r#"[{"host_id":"h1","online":true}]"#)
                };
                let resp = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        (base, log)
    }

    #[tokio::test]
    async fn adds_the_token_and_refreshes_a_refused_one() {
        let (base, log) = fake_cloud().await;
        let asked = Arc::new(Mutex::new(Vec::new()));
        let a = asked.clone();
        let token = move |force: bool| {
            a.lock().unwrap().push(force);
            async move { Ok(if force { "good" } else { "stale" }.to_string()) }
        };
        let reply = request(&base, token, "GET", "/v1/hosts").await.unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body[0]["host_id"], "h1");
        assert_eq!(*asked.lock().unwrap(), vec![false, true]);
        assert_eq!(log.lock().unwrap().len(), 2);

        // Refused even when fresh: the 401 is passed on, once.
        let reply = request(&base, |_| async { Ok("bad".to_string()) }, "GET", "/v1/hosts").await.unwrap();
        assert_eq!(reply.status, 401);
        assert_eq!(log.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn signed_out_or_disallowed_sends_nothing() {
        let (base, log) = fake_cloud().await;
        let reply = request(&base, |_| async { Err(SIGN_IN_REQUIRED.to_string()) }, "GET", "/v1/hosts")
            .await
            .unwrap();
        assert_eq!(reply, unauthenticated());
        assert!(request(&base, |_| async { Ok("good".to_string()) }, "POST", "/v1/admin").await.is_err());
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_cloud_is_an_error() {
        let err = request("http://127.0.0.1:9", |_| async { Ok("good".to_string()) }, "GET", "/v1/hosts")
            .await
            .unwrap_err();
        assert_eq!(err, "Zwiim Cloud unreachable");
    }
}
