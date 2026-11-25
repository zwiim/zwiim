//! Outbound HTTP for connectors (design r3 §5.3). Every request a connector
//! makes goes through [`EgressHttpClient`]: it reaches only origins in the
//! connection's endpoint rules, connects only to addresses [`EgressGuard`]
//! checked, follows redirects only within the same origin and caps what it
//! reads back.
//!
//! Not here yet: cross-origin redirects for uncredentialed OAuth discovery
//! (comes with OAuth), admin-blocked origins (comes with `policy.json`), and refusing
//! the Host's own listen ports in `local` rules (checked when a connection's
//! endpoints are validated).

use std::error::Error as _;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Bytes;
use futures::stream::{BoxStream, StreamExt};
use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use http::Method;
use url::{Host, Url};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Same-origin redirects followed for one request.
const MAX_REDIRECTS: usize = 3;
/// Largest whole response body read into memory.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EgressError {
    #[error("invalid endpoint {origin}: {reason}")]
    InvalidRule {
        origin: String,
        reason: &'static str,
    },
    #[error("{origin} isn't an endpoint of this connection")]
    NotAllowed { origin: String },
    #[error("{host} is at {addr}, which is in the refused range {range}")]
    RefusedAddress {
        host: String,
        addr: IpAddr,
        range: &'static str,
    },
    #[error("could not resolve {host}: {reason}")]
    Resolve { host: String, reason: String },
    #[error("{origin} isn't allowed to receive credentials")]
    CredentialsNotAllowed { origin: String },
    #[error("header {0} carries credentials; pass them as the request's credential")]
    CredentialHeader(String),
    #[error("the credential isn't a valid header value")]
    InvalidCredential,
    #[error("the service redirected ({status}) to somewhere Zwiim doesn't follow")]
    Redirect {
        status: u16,
        location: Option<String>,
    },
    #[error("the response is larger than {limit} bytes")]
    BodyTooLarge { limit: usize },
    #[error("{0}")]
    Transport(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkClass {
    /// Internet services. Every special-purpose address range is refused.
    Public,
    /// An exact origin the user added for a server on their own machine or
    /// network. Only unspecified, multicast and broadcast addresses are refused.
    Local,
}

/// One origin a connection may reach (`connection_endpoint`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointRule {
    scheme: String,
    host: Host<String>,
    port: u16,
    credentials: bool,
    network_class: NetworkClass,
}

impl EndpointRule {
    /// `origin` is `scheme://host[:port]`. Public origins must be https.
    pub fn new(
        origin: &str,
        credentials: bool,
        network_class: NetworkClass,
    ) -> Result<Self, EgressError> {
        let invalid = |reason| EgressError::InvalidRule {
            origin: origin.to_string(),
            reason,
        };
        let url = Url::parse(origin).map_err(|_| invalid("not a URL"))?;
        match (url.scheme(), network_class) {
            ("https", _) | ("http", NetworkClass::Local) => {}
            ("http", NetworkClass::Public) => {
                return Err(invalid("public endpoints must use https"))
            }
            _ => return Err(invalid("only http and https are supported")),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(invalid("credentials don't go in the URL"));
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(invalid("an endpoint is an origin, without a path"));
        }
        let host = url.host().ok_or_else(|| invalid("no host"))?.to_owned();
        if let Some(ip) = literal_ip(&host) {
            if refused_range(ip, network_class).is_some() {
                return Err(invalid("the address is in a refused range"));
            }
        }
        Ok(Self {
            scheme: url.scheme().to_string(),
            host,
            port: url
                .port_or_known_default()
                .ok_or_else(|| invalid("no port"))?,
            credentials,
            network_class,
        })
    }

    /// Whether `url` is at this rule's origin.
    pub fn matches(&self, url: &Url) -> bool {
        url.scheme() == self.scheme
            && url.host().map(|h| h.to_owned()).as_ref() == Some(&self.host)
            && url.port_or_known_default() == Some(self.port)
    }
}

fn literal_ip(host: &Host<String>) -> Option<IpAddr> {
    match host {
        Host::Ipv4(ip) => Some(IpAddr::V4(*ip)),
        Host::Ipv6(ip) => Some(IpAddr::V6(*ip)),
        Host::Domain(_) => None,
    }
}

/// DNS, injectable so tests can return any answer.
#[async_trait]
pub trait Resolver: Send + Sync + 'static {
    /// Every A and AAAA answer for `host`.
    async fn lookup(&self, host: &str) -> io::Result<Vec<IpAddr>>;
}

pub struct SystemResolver;

#[async_trait]
impl Resolver for SystemResolver {
    async fn lookup(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        Ok(tokio::net::lookup_host((host, 0))
            .await?
            .map(|addr| addr.ip())
            .collect())
    }
}

/// Decides where a connection's requests may go.
pub struct EgressGuard {
    rules: Vec<EndpointRule>,
    resolver: Arc<dyn Resolver>,
}

impl EgressGuard {
    pub fn new(rules: Vec<EndpointRule>, resolver: Arc<dyn Resolver>) -> Self {
        Self { rules, resolver }
    }

    /// The rule `url` falls under. An IP literal is checked here, since it is
    /// never resolved. The `url` crate has already normalized decimal, octal
    /// and hex IPv4 spellings.
    pub fn check_url(&self, url: &Url) -> Result<&EndpointRule, EgressError> {
        let origin = url.origin().ascii_serialization();
        if !url.username().is_empty() || url.password().is_some() {
            return Err(EgressError::NotAllowed { origin });
        }
        let rule = self
            .rules
            .iter()
            .find(|rule| rule.matches(url))
            .ok_or(EgressError::NotAllowed { origin })?;
        if let Some(ip) = literal_ip(&rule.host) {
            check_addr(&rule.host.to_string(), ip, rule.network_class)?;
        }
        Ok(rule)
    }

    /// Resolves `host` and checks every answer. One refused answer refuses
    /// them all, so a mix of public and private answers can't be played.
    pub async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, EgressError> {
        let class = self
            .rules
            .iter()
            .filter(|rule| matches!(&rule.host, Host::Domain(d) if d == host))
            .map(|rule| rule.network_class)
            // A name under both kinds of rule gets the stricter one.
            .min_by_key(|class| *class == NetworkClass::Local)
            .ok_or_else(|| EgressError::NotAllowed {
                origin: host.to_string(),
            })?;
        let resolve_error = |reason: String| EgressError::Resolve {
            host: host.to_string(),
            reason,
        };
        let addrs = self
            .resolver
            .lookup(host)
            .await
            .map_err(|e| resolve_error(e.to_string()))?;
        if addrs.is_empty() {
            return Err(resolve_error("no addresses".into()));
        }
        for &addr in &addrs {
            check_addr(host, addr, class)?;
        }
        Ok(addrs)
    }
}

fn check_addr(host: &str, addr: IpAddr, class: NetworkClass) -> Result<(), EgressError> {
    match refused_range(addr, class) {
        Some(range) => Err(EgressError::RefusedAddress {
            host: host.to_string(),
            addr,
            range,
        }),
        None => Ok(()),
    }
}

/// The range that refuses `addr` for `class`, if any.
fn refused_range(addr: IpAddr, class: NetworkClass) -> Option<&'static str> {
    match class {
        NetworkClass::Public => public_refused(addr),
        NetworkClass::Local => match addr.to_canonical() {
            IpAddr::V4(v4) if v4.is_unspecified() => Some("0.0.0.0/32"),
            IpAddr::V4(v4) if v4.is_broadcast() => Some("255.255.255.255/32"),
            IpAddr::V4(v4) if v4.is_multicast() => Some("224.0.0.0/4"),
            IpAddr::V6(v6) if v6.is_unspecified() => Some("::/128"),
            IpAddr::V6(v6) if v6.is_multicast() => Some("ff00::/8"),
            _ => None,
        },
    }
}

const REFUSED_V4: [(Ipv4Addr, u8, &str); 15] = [
    (Ipv4Addr::new(0, 0, 0, 0), 8, "0.0.0.0/8"),
    (Ipv4Addr::new(10, 0, 0, 0), 8, "10.0.0.0/8"),
    (Ipv4Addr::new(100, 64, 0, 0), 10, "100.64.0.0/10"),
    (Ipv4Addr::new(127, 0, 0, 0), 8, "127.0.0.0/8"),
    (Ipv4Addr::new(169, 254, 0, 0), 16, "169.254.0.0/16"),
    (Ipv4Addr::new(172, 16, 0, 0), 12, "172.16.0.0/12"),
    (Ipv4Addr::new(192, 0, 0, 0), 24, "192.0.0.0/24"),
    (Ipv4Addr::new(192, 0, 2, 0), 24, "192.0.2.0/24"),
    (Ipv4Addr::new(192, 88, 99, 0), 24, "192.88.99.0/24"),
    (Ipv4Addr::new(192, 168, 0, 0), 16, "192.168.0.0/16"),
    (Ipv4Addr::new(198, 18, 0, 0), 15, "198.18.0.0/15"),
    (Ipv4Addr::new(198, 51, 100, 0), 24, "198.51.100.0/24"),
    (Ipv4Addr::new(203, 0, 113, 0), 24, "203.0.113.0/24"),
    (Ipv4Addr::new(224, 0, 0, 0), 4, "224.0.0.0/4"),
    // Includes 255.255.255.255.
    (Ipv4Addr::new(240, 0, 0, 0), 4, "240.0.0.0/4"),
];

const REFUSED_V6: [(Ipv6Addr, u8, &str); 8] = [
    // `::`, `::1` and the deprecated IPv4-compatible form.
    (Ipv6Addr::UNSPECIFIED, 96, "::/96"),
    // Local-use NAT64: where the IPv4 address sits depends on the network's
    // prefix length, so the whole range is refused.
    (
        Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0, 0),
        48,
        "64:ff9b:1::/48",
    ),
    (Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0), 64, "100::/64"),
    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23, "2001::/23"),
    (
        Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0),
        32,
        "2001:db8::/32",
    ),
    (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7, "fc00::/7"),
    (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10, "fe80::/10"),
    (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8, "ff00::/8"),
];

fn public_refused(addr: IpAddr) -> Option<&'static str> {
    match addr {
        IpAddr::V4(v4) => REFUSED_V4
            .iter()
            .find(|(net, len, _)| in_prefix(u32::from(v4).into(), u32::from(*net).into(), 32, *len))
            .map(|(_, _, range)| *range),
        IpAddr::V6(v6) => {
            if let Some(v4) = embedded_v4(v6) {
                return public_refused(IpAddr::V4(v4));
            }
            REFUSED_V6
                .iter()
                .find(|(net, len, _)| in_prefix(u128::from(v6), u128::from(*net), 128, *len))
                .map(|(_, _, range)| *range)
        }
    }
}

fn in_prefix(addr: u128, net: u128, bits: u32, len: u8) -> bool {
    let shift = bits - u32::from(len);
    addr.checked_shr(shift).unwrap_or(0) == net.checked_shr(shift).unwrap_or(0)
}

/// The IPv4 address inside an IPv4-mapped (`::ffff:0:0/96`), NAT64
/// (`64:ff9b::/96`) or 6to4 (`2002::/16`) address.
fn embedded_v4(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = v6.segments();
    let o = v6.octets();
    if s[..6] == [0, 0, 0, 0, 0, 0xffff] || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        Some(Ipv4Addr::new(o[12], o[13], o[14], o[15]))
    } else if s[0] == 0x2002 {
        Some(Ipv4Addr::new(o[2], o[3], o[4], o[5]))
    } else {
        None
    }
}

/// Hands reqwest only addresses the guard checked. reqwest asks again only
/// for a new connection, so a pooled connection keeps the address it was
/// checked with.
struct GuardResolver(Arc<EgressGuard>);

impl reqwest::dns::Resolve for GuardResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let guard = self.0.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = guard
                .resolve(&host)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
            let addrs: reqwest::dns::Addrs =
                Box::new(addrs.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// A secret sent as one request header, only to origins whose rule allows
/// credentials.
#[derive(Clone)]
pub struct Credential {
    header: HeaderName,
    value: HeaderValue,
}

impl Credential {
    pub fn new(header: HeaderName, value: &str) -> Result<Self, EgressError> {
        let mut value = HeaderValue::from_str(value).map_err(|_| EgressError::InvalidCredential)?;
        value.set_sensitive(true);
        Ok(Self { header, value })
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct EgressRequest {
    pub method: Method,
    pub url: Url,
    pub headers: HeaderMap,
    pub body: Option<Bytes>,
    pub credential: Option<Credential>,
}

/// Headers that carry credentials. They may only be set through
/// [`EgressRequest::credential`], so the origin check can't be skipped.
const CREDENTIAL_HEADERS: [HeaderName; 3] = [
    header::AUTHORIZATION,
    header::PROXY_AUTHORIZATION,
    header::COOKIE,
];

/// The only HTTP client connectors use.
pub struct EgressHttpClient {
    guard: Arc<EgressGuard>,
    http: reqwest::Client,
    max_body: usize,
}

impl EgressHttpClient {
    pub fn new(guard: Arc<EgressGuard>) -> Result<Self, EgressError> {
        let http = reqwest::Client::builder()
            .dns_resolver(Arc::new(GuardResolver(guard.clone())))
            .redirect(reqwest::redirect::Policy::none())
            // A system proxy would choose the address itself.
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(concat!("Zwiim/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(transport_error)?;
        Ok(Self {
            guard,
            http,
            max_body: MAX_BODY_BYTES,
        })
    }

    pub fn with_max_body(mut self, max_body: usize) -> Self {
        self.max_body = max_body;
        self
    }

    /// Sends one request. Nothing leaves the Host unless the origin is an
    /// endpoint, the credential (if any) may go there and every address it
    /// connects to passes the guard.
    ///
    /// Redirects within the same origin are followed, up to
    /// [`MAX_REDIRECTS`], so a server that moves `/mcp` to `/mcp/` still
    /// works. 307 and 308 keep the method and body; 301, 302 and 303 are
    /// followed only for GET. Any other 3xx, including every redirect to
    /// another origin, is an error and its target is never contacted.
    pub async fn send(&self, request: EgressRequest) -> Result<EgressResponse, EgressError> {
        let rule = self.guard.check_url(&request.url)?;
        if request.credential.is_some() && !rule.credentials {
            return Err(EgressError::CredentialsNotAllowed {
                origin: request.url.origin().ascii_serialization(),
            });
        }
        if let Some(name) = CREDENTIAL_HEADERS
            .iter()
            .find(|name| request.headers.contains_key(*name))
        {
            return Err(EgressError::CredentialHeader(name.to_string()));
        }
        let mut headers = request.headers;
        if let Some(credential) = request.credential {
            headers.insert(credential.header, credential.value);
        }
        let mut url = request.url;
        let mut redirects = 0;
        loop {
            let mut builder = self
                .http
                .request(request.method.clone(), url.clone())
                .headers(headers.clone());
            if let Some(body) = &request.body {
                builder = builder.body(body.clone());
            }
            let response = builder.send().await.map_err(transport_error)?;
            let status = response.status();
            if !status.is_redirection() {
                return Ok(EgressResponse {
                    response,
                    max_body: self.max_body,
                });
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let refused = || EgressError::Redirect {
                status: status.as_u16(),
                location: location.clone(),
            };
            let keeps_method = matches!(status.as_u16(), 307 | 308)
                || (matches!(status.as_u16(), 301..=303) && request.method == Method::GET);
            let next = location
                .as_deref()
                .and_then(|location| url.join(location).ok())
                .filter(|next| next.origin() == url.origin() && self.guard.check_url(next).is_ok());
            match next {
                Some(next) if keeps_method && redirects < MAX_REDIRECTS => {
                    redirects += 1;
                    url = next;
                }
                _ => return Err(refused()),
            }
        }
    }
}

/// reqwest wraps the guard's refusal from inside the resolver; dig it out so
/// callers see why.
fn transport_error(err: reqwest::Error) -> EgressError {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        if let Some(refusal) = cause.downcast_ref::<EgressError>() {
            return refusal.clone();
        }
        message = format!("{message}: {cause}");
        source = cause.source();
    }
    EgressError::Transport(message)
}

#[derive(Debug)]
pub struct EgressResponse {
    response: reqwest::Response,
    max_body: usize,
}

impl EgressResponse {
    pub fn status(&self) -> http::StatusCode {
        self.response.status()
    }

    pub fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }

    /// The whole body, refused once it passes the cap and before anything
    /// parses it.
    pub async fn bytes(self) -> Result<Bytes, EgressError> {
        let limit = self.max_body;
        self.bytes_up_to(limit).await
    }

    /// [`Self::bytes`] with a lower cap for this one response.
    pub async fn bytes_up_to(mut self, limit: usize) -> Result<Bytes, EgressError> {
        let limit = limit.min(self.max_body);
        let too_large = EgressError::BodyTooLarge { limit };
        if self
            .response
            .content_length()
            .is_some_and(|len| len > limit as u64)
        {
            return Err(too_large);
        }
        let mut body = Vec::new();
        while let Some(chunk) = self.response.chunk().await.map_err(transport_error)? {
            if body.len() + chunk.len() > limit {
                return Err(too_large);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body.into())
    }

    /// The body as an event stream that ends with an error once one event
    /// passes `max_event` bytes. A stream may stay open for the connection's
    /// life, so only events are capped, not the total.
    pub fn sse(self, max_event: usize) -> BoxStream<'static, Result<Bytes, EgressError>> {
        cap_sse(self.response.bytes_stream(), max_event)
    }
}

fn cap_sse<S>(chunks: S, max_event: usize) -> BoxStream<'static, Result<Bytes, EgressError>>
where
    S: futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    chunks
        .scan(Some(SseEventLimit::new(max_event)), |limit, chunk| {
            let item = match limit {
                Some(state) => {
                    let checked = chunk
                        .map_err(transport_error)
                        .and_then(|chunk| state.feed(&chunk).map(|()| chunk));
                    if checked.is_err() {
                        *limit = None;
                    }
                    Some(checked)
                }
                None => None,
            };
            futures::future::ready(item)
        })
        .boxed()
}

/// Counts the bytes of the current server-sent event. Lines end in CR, LF or
/// CRLF; a blank line ends the event.
struct SseEventLimit {
    max: usize,
    event: usize,
    line: usize,
    after_cr: bool,
}

impl SseEventLimit {
    fn new(max: usize) -> Self {
        Self {
            max,
            event: 0,
            line: 0,
            after_cr: false,
        }
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), EgressError> {
        for &byte in chunk {
            let after_cr = std::mem::replace(&mut self.after_cr, byte == b'\r');
            match byte {
                b'\n' if after_cr => {}
                b'\r' | b'\n' => {
                    if self.line == 0 {
                        self.event = 0;
                    }
                    self.line = 0;
                }
                _ => {
                    self.line += 1;
                    self.event += 1;
                    if self.event > self.max {
                        return Err(EgressError::BodyTooLarge { limit: self.max });
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
