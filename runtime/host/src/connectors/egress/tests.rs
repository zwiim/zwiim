//! The egress rules through an injected resolver, and end to end against
//! local test servers that count what reaches them.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap as AxumHeaders, StatusCode};
use axum::routing::{get, post};
use axum::Router;
use futures::StreamExt;
use http::header::{self, HeaderMap, HeaderName};
use http::Method;
use url::Url;

use super::*;

/// Answers only the names it was given.
struct FixedResolver(HashMap<&'static str, Vec<IpAddr>>);

#[async_trait]
impl Resolver for FixedResolver {
    async fn lookup(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        self.0
            .get(host)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "unknown name"))
    }
}

fn resolver(answers: &[(&'static str, &[&str])]) -> Arc<dyn Resolver> {
    let map = answers
        .iter()
        .map(|(host, ips)| (*host, ips.iter().map(|ip| ip.parse().unwrap()).collect()))
        .collect();
    Arc::new(FixedResolver(map))
}

fn public(origin: &str) -> EndpointRule {
    EndpointRule::new(origin, true, NetworkClass::Public).unwrap()
}

fn local(origin: &str) -> EndpointRule {
    EndpointRule::new(origin, true, NetworkClass::Local).unwrap()
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn get_request(url: &str) -> EgressRequest {
    EgressRequest {
        method: Method::GET,
        url: Url::parse(url).unwrap(),
        headers: HeaderMap::new(),
        body: None,
        credential: None,
    }
}

fn credential(value: &str) -> Credential {
    Credential::new(header::AUTHORIZATION, value).unwrap()
}

#[test]
fn public_rules_refuse_every_special_range() {
    let refused = [
        ("0.1.2.3", "0.0.0.0/8"),
        ("10.0.0.1", "10.0.0.0/8"),
        ("100.64.0.0", "100.64.0.0/10"),
        ("100.127.255.255", "100.64.0.0/10"),
        ("127.0.0.1", "127.0.0.0/8"),
        ("169.254.169.254", "169.254.0.0/16"),
        ("172.16.0.0", "172.16.0.0/12"),
        ("172.31.255.255", "172.16.0.0/12"),
        ("192.0.0.8", "192.0.0.0/24"),
        ("192.0.2.1", "192.0.2.0/24"),
        ("192.88.99.1", "192.88.99.0/24"),
        ("192.168.1.1", "192.168.0.0/16"),
        ("198.18.0.0", "198.18.0.0/15"),
        ("198.19.255.255", "198.18.0.0/15"),
        ("198.51.100.7", "198.51.100.0/24"),
        ("203.0.113.9", "203.0.113.0/24"),
        ("224.0.0.1", "224.0.0.0/4"),
        ("240.0.0.1", "240.0.0.0/4"),
        ("255.255.255.255", "240.0.0.0/4"),
        ("::", "::/96"),
        ("::1", "::/96"),
        ("::7f00:1", "::/96"),
        ("::ffff:127.0.0.1", "127.0.0.0/8"),
        ("::ffff:10.1.2.3", "10.0.0.0/8"),
        ("64:ff9b::a9fe:a9fe", "169.254.0.0/16"),
        ("64:ff9b:1::808:808", "64:ff9b:1::/48"),
        ("100::1", "100::/64"),
        ("2001::1", "2001::/23"),
        ("2001:1ff:ffff::", "2001::/23"),
        ("2001:db8::1", "2001:db8::/32"),
        ("2002:c0a8:0101::1", "192.168.0.0/16"),
        ("fc00::1", "fc00::/7"),
        ("fdff::1", "fc00::/7"),
        ("fe80::1", "fe80::/10"),
        ("ff02::1", "ff00::/8"),
    ];
    for (addr, range) in refused {
        let ip: IpAddr = addr.parse().unwrap();
        assert_eq!(
            refused_range(ip, NetworkClass::Public),
            Some(range),
            "{addr}"
        );
    }
    let allowed = [
        "1.1.1.1",
        "8.8.8.8",
        "100.63.255.255",
        "100.128.0.0",
        "172.15.255.255",
        "172.32.0.0",
        "198.17.255.255",
        "198.20.0.0",
        "223.255.255.255",
        "2606:4700:4700::1111",
        "2001:200::1",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
        "2002:0808:0808::1",
    ];
    for addr in allowed {
        let ip: IpAddr = addr.parse().unwrap();
        assert_eq!(refused_range(ip, NetworkClass::Public), None, "{addr}");
    }
}

#[test]
fn local_rules_refuse_only_unroutable_addresses() {
    for addr in ["127.0.0.1", "::1", "10.0.0.2", "192.168.1.20", "fe80::1"] {
        let ip: IpAddr = addr.parse().unwrap();
        assert_eq!(refused_range(ip, NetworkClass::Local), None, "{addr}");
    }
    for addr in [
        "0.0.0.0",
        "::",
        "::ffff:0.0.0.0",
        "255.255.255.255",
        "224.0.0.251",
        "ff02::fb",
    ] {
        let ip: IpAddr = addr.parse().unwrap();
        assert!(refused_range(ip, NetworkClass::Local).is_some(), "{addr}");
    }
}

#[test]
fn rules_are_plain_origins() {
    let invalid = |origin, class| EndpointRule::new(origin, true, class).unwrap_err();
    for (origin, class) in [
        ("http://mcp.example.com", NetworkClass::Public),
        ("ftp://mcp.example.com", NetworkClass::Local),
        ("https://user:pw@mcp.example.com", NetworkClass::Public),
        ("https://mcp.example.com/mcp", NetworkClass::Public),
        ("https://mcp.example.com/?a=1", NetworkClass::Public),
        ("https://127.0.0.1", NetworkClass::Public),
        // 2130706433 is 127.0.0.1 written as one number.
        ("https://2130706433", NetworkClass::Public),
        ("http://0.0.0.0:8080", NetworkClass::Local),
        ("http://[::]:8080", NetworkClass::Local),
        ("not a url", NetworkClass::Public),
    ] {
        assert!(
            matches!(invalid(origin, class), EgressError::InvalidRule { .. }),
            "{origin}"
        );
    }
    assert!(EndpointRule::new("https://mcp.example.com:443", true, NetworkClass::Public).is_ok());
    assert!(EndpointRule::new("https://93.184.216.34", true, NetworkClass::Public).is_ok());
    assert!(EndpointRule::new("http://localhost:3000", true, NetworkClass::Local).is_ok());
}

#[test]
fn only_listed_origins_pass() {
    let guard = EgressGuard::new(
        vec![
            public("https://mcp.example.com"),
            local("http://127.0.0.1:8080"),
        ],
        resolver(&[]),
    );
    assert!(guard.check_url(&url("https://mcp.example.com/mcp")).is_ok());
    assert!(guard
        .check_url(&url("https://MCP.example.com:443/x"))
        .is_ok());
    for refused in [
        "http://mcp.example.com/mcp",
        "https://mcp.example.com:8443/mcp",
        "https://evil.example.com/mcp",
        "https://mcp.example.com.evil.test/",
        "https://user@mcp.example.com/mcp",
        "http://127.0.0.1:8081/",
    ] {
        assert!(
            matches!(
                guard.check_url(&url(refused)),
                Err(EgressError::NotAllowed { .. })
            ),
            "{refused}"
        );
    }
}

#[test]
fn ipv4_spellings_name_the_same_address() {
    let guard = EgressGuard::new(vec![local("http://127.0.0.1:8080")], resolver(&[]));
    for spelling in [
        "http://2130706433:8080/",
        "http://0x7f.1:8080/",
        "http://0177.0.0.1:8080/",
        "http://127.1:8080/",
    ] {
        assert!(guard.check_url(&url(spelling)).is_ok(), "{spelling}");
    }
}

#[tokio::test]
async fn one_refused_answer_refuses_the_name() {
    let guard = EgressGuard::new(
        vec![
            public("https://mixed.example.com"),
            public("https://ok.example.com"),
        ],
        resolver(&[
            ("mixed.example.com", &["93.184.216.34", "10.0.0.1"]),
            ("ok.example.com", &["93.184.216.34", "2606:2800:220:1::1"]),
        ]),
    );
    assert_eq!(
        guard.resolve("mixed.example.com").await,
        Err(EgressError::RefusedAddress {
            host: "mixed.example.com".into(),
            addr: "10.0.0.1".parse().unwrap(),
            range: "10.0.0.0/8",
        })
    );
    assert_eq!(guard.resolve("ok.example.com").await.unwrap().len(), 2);
    assert!(matches!(
        guard.resolve("other.example.com").await,
        Err(EgressError::NotAllowed { .. })
    ));
}

#[tokio::test]
async fn a_name_under_both_kinds_of_rule_is_public() {
    let guard = EgressGuard::new(
        vec![
            local("http://dual.example.com:8080"),
            public("https://dual.example.com"),
        ],
        resolver(&[("dual.example.com", &["127.0.0.1"])]),
    );
    assert!(matches!(
        guard.resolve("dual.example.com").await,
        Err(EgressError::RefusedAddress { .. })
    ));
}

#[test]
fn credential_debug_hides_the_secret() {
    let debug = format!("{:?}", credential("Bearer s3cret"));
    assert!(!debug.contains("s3cret"), "{debug}");
}

/// A server that counts every TCP connection and never answers.
async fn counting_listener() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    tokio::spawn(async move {
        while let Ok((_stream, _)) = listener.accept().await {
            count.fetch_add(1, Ordering::SeqCst);
        }
    });
    (addr, accepted)
}

async fn serve(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });
    addr
}

fn client(rules: Vec<EndpointRule>, answers: &[(&'static str, &[&str])]) -> EgressHttpClient {
    EgressHttpClient::new(Arc::new(EgressGuard::new(rules, resolver(answers)))).unwrap()
}

#[tokio::test]
async fn a_public_name_that_resolves_to_loopback_is_never_contacted() {
    let (addr, accepted) = counting_listener().await;
    let origin = format!("https://mcp.example.test:{}", addr.port());
    let client = client(
        vec![public(&origin)],
        &[("mcp.example.test", &["127.0.0.1"])],
    );
    let err = client
        .send(get_request(&format!("{origin}/mcp")))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        EgressError::RefusedAddress {
            host: "mcp.example.test".into(),
            addr: "127.0.0.1".parse().unwrap(),
            range: "127.0.0.0/8",
        }
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unlisted_origin_is_never_contacted() {
    let (addr, accepted) = counting_listener().await;
    let client = client(vec![local("http://127.0.0.1:1")], &[]);
    let err = client
        .send(get_request(&format!("http://127.0.0.1:{}/", addr.port())))
        .await
        .unwrap_err();
    assert!(matches!(err, EgressError::NotAllowed { .. }), "{err:?}");
    assert_eq!(accepted.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn connects_to_the_checked_address() {
    let addr = serve(Router::new().route("/mcp", get(|| async { "pinned" }))).await;
    // The system resolver has never heard of this name; only the guard's answer
    // can get the request there.
    let origin = format!("http://fake.example.test:{}", addr.port());
    let client = client(
        vec![local(&origin)],
        &[("fake.example.test", &["127.0.0.1"])],
    );
    let response = client
        .send(get_request(&format!("{origin}/mcp")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap(), Bytes::from("pinned"));
}

fn redirect(location: &str) -> (StatusCode, [(HeaderName, String); 1]) {
    (
        StatusCode::TEMPORARY_REDIRECT,
        [(header::LOCATION, location.to_string())],
    )
}

#[tokio::test]
async fn same_origin_redirects_are_followed() {
    let loops = Arc::new(AtomicUsize::new(0));
    let loop_hits = loops.clone();
    let router = Router::new()
        .route("/mcp", post(|| async { redirect("/mcp/") }))
        .route(
            "/mcp/",
            post(|headers: AxumHeaders, body: Bytes| async move {
                let auth = headers.get(header::AUTHORIZATION).unwrap();
                format!(
                    "{} {}",
                    auth.to_str().unwrap(),
                    String::from_utf8_lossy(&body)
                )
            }),
        )
        .route(
            "/old",
            get(|| async { (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, "/new")]) }),
        )
        .route("/new", get(|| async { "moved" }))
        .route(
            "/see-other",
            post(|| async { (StatusCode::SEE_OTHER, [(header::LOCATION, "/new")]) }),
        )
        .route(
            "/loop",
            get(move || async move {
                loop_hits.fetch_add(1, Ordering::SeqCst);
                redirect("/loop")
            }),
        );
    let addr = serve(router).await;
    let origin = format!("http://127.0.0.1:{}", addr.port());
    let client = client(vec![local(&origin)], &[]);

    // 307 keeps the method, the body and the credential.
    let mut request = get_request(&format!("{origin}/mcp"));
    request.method = Method::POST;
    request.body = Some(Bytes::from("{}"));
    request.credential = Some(credential("Bearer s3cret"));
    let response = client.send(request).await.unwrap();
    assert_eq!(
        response.bytes().await.unwrap(),
        Bytes::from("Bearer s3cret {}")
    );

    // 301 is followed for GET.
    let response = client
        .send(get_request(&format!("{origin}/old")))
        .await
        .unwrap();
    assert_eq!(response.bytes().await.unwrap(), Bytes::from("moved"));

    // 303 would turn a POST into a GET, so it is an error.
    let mut request = get_request(&format!("{origin}/see-other"));
    request.method = Method::POST;
    assert!(matches!(
        client.send(request).await,
        Err(EgressError::Redirect { status: 303, .. })
    ));

    // The first request plus three followed redirects, then an error.
    let err = client
        .send(get_request(&format!("{origin}/loop")))
        .await
        .unwrap_err();
    assert!(
        matches!(err, EgressError::Redirect { status: 307, .. }),
        "{err:?}"
    );
    assert_eq!(loops.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn cross_origin_redirects_are_errors_and_the_target_is_never_contacted() {
    let (other, accepted) = counting_listener().await;
    let other_origin = format!("http://127.0.0.1:{}", other.port());
    let to_other = format!("{other_origin}/mcp");
    let location = to_other.clone();
    let router = Router::new()
        .route(
            "/mcp",
            get(move || {
                let location = location.clone();
                async move { redirect(&location) }
            }),
        )
        .route(
            "/userinfo",
            get(|| async { redirect("http://user:pw@127.0.0.1/") }),
        );
    let addr = serve(router).await;
    let origin = format!("http://127.0.0.1:{}", addr.port());
    // Even an origin that is itself an endpoint isn't reached by a redirect.
    let client = client(vec![local(&origin), local(&other_origin)], &[]);
    let err = client
        .send(get_request(&format!("{origin}/mcp")))
        .await
        .unwrap_err();
    assert_eq!(
        err,
        EgressError::Redirect {
            status: 307,
            location: Some(to_other),
        }
    );
    assert_eq!(accepted.load(Ordering::SeqCst), 0);

    let err = client
        .send(get_request(&format!("{origin}/userinfo")))
        .await
        .unwrap_err();
    assert!(matches!(err, EgressError::Redirect { .. }), "{err:?}");
}

#[tokio::test]
async fn credentials_go_only_where_the_rule_allows() {
    let router = Router::new().route(
        "/echo",
        get(|headers: AxumHeaders| async move {
            headers
                .get(header::AUTHORIZATION)
                .map(|v| v.to_str().unwrap().to_string())
                .unwrap_or_default()
        }),
    );
    let addr = serve(router).await;
    let origin = format!("http://127.0.0.1:{}", addr.port());
    let url = format!("{origin}/echo");

    let with_credentials = client(vec![local(&origin)], &[]);
    let mut request = get_request(&url);
    request.credential = Some(credential("Bearer s3cret"));
    let response = with_credentials.send(request).await.unwrap();
    assert_eq!(
        response.bytes().await.unwrap(),
        Bytes::from("Bearer s3cret")
    );

    let no_credentials = client(
        vec![EndpointRule::new(&origin, false, NetworkClass::Local).unwrap()],
        &[],
    );
    let mut request = get_request(&url);
    request.credential = Some(credential("Bearer s3cret"));
    assert!(matches!(
        no_credentials.send(request).await,
        Err(EgressError::CredentialsNotAllowed { .. })
    ));

    for name in [
        header::AUTHORIZATION,
        header::COOKIE,
        header::PROXY_AUTHORIZATION,
    ] {
        let mut request = get_request(&url);
        request.headers.insert(name.clone(), "x".parse().unwrap());
        assert_eq!(
            with_credentials.send(request).await.unwrap_err(),
            EgressError::CredentialHeader(name.to_string())
        );
    }
}

#[tokio::test]
async fn bodies_over_the_cap_are_refused() {
    let router = Router::new()
        .route("/sized", get(|| async { "x".repeat(100) }))
        .route(
            "/streamed",
            get(|| async {
                let chunks = (0..10).map(|_| Ok::<_, io::Error>(Bytes::from("x".repeat(10))));
                Body::from_stream(futures::stream::iter(chunks))
            }),
        );
    let addr = serve(router).await;
    let origin = format!("http://127.0.0.1:{}", addr.port());
    let client = client(vec![local(&origin)], &[]).with_max_body(50);
    for path in ["/sized", "/streamed"] {
        let response = client
            .send(get_request(&format!("{origin}{path}")))
            .await
            .unwrap();
        assert_eq!(
            response.bytes().await,
            Err(EgressError::BodyTooLarge { limit: 50 }),
            "{path}"
        );
    }
    let roomy = self::client(vec![local(&origin)], &[]).with_max_body(100);
    let response = roomy
        .send(get_request(&format!("{origin}/streamed")))
        .await
        .unwrap();
    assert_eq!(response.bytes().await.unwrap().len(), 100);
}

async fn sse_items(chunks: &[&str], max_event: usize) -> Vec<Result<Bytes, EgressError>> {
    let source = futures::stream::iter(
        chunks
            .iter()
            .map(|c| Ok::<_, reqwest::Error>(Bytes::from(c.to_string())))
            .collect::<Vec<_>>(),
    );
    cap_sse(source, max_event).collect().await
}

#[tokio::test]
async fn sse_events_are_capped_one_at_a_time() {
    // Each event is 8 bytes of field data; the cap resets at every blank line,
    // whichever line ending the server uses.
    for ending in ["\n\n", "\r\r", "\r\n\r\n"] {
        let event = format!("data: ab{ending}");
        let items = sse_items(&[&event, &event, &event], 8).await;
        assert!(items.iter().all(Result::is_ok), "{ending:?}: {items:?}");
    }
    // An event split over many chunks is still one event.
    let items = sse_items(&["data: ", "abc", "def", "\n\n"], 8).await;
    assert_eq!(
        items.last().unwrap(),
        &Err(EgressError::BodyTooLarge { limit: 8 })
    );
    // Nothing comes after the error.
    let items = sse_items(&["data: abcdefgh\n\n", "data: ok\n\n"], 8).await;
    assert_eq!(items, vec![Err(EgressError::BodyTooLarge { limit: 8 })]);
    // A multi-line event counts all its lines.
    let items = sse_items(&["data: a\ndata: b\n\n"], 10).await;
    assert_eq!(items, vec![Err(EgressError::BodyTooLarge { limit: 10 })]);
}
