//! Reaching the Cloud: WSS sockets and HTTPS requests, with an optional
//! extra trusted CA for a Cloud behind a private CA (`ZWIIM_CLOUD_CA_CERT`).
//! URLs are always normalized Cloud origins (`zwiim_cloud::proto`), which
//! are `https` unless the Cloud is on loopback.

use std::time::Duration;

use futures_util::StreamExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use zwiim_cloud::proto::CloudUrlError;

pub type CloudSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Environment variable naming a PEM file with an extra CA to trust for the
/// Cloud, in addition to the system roots.
pub const CA_CERT_ENV: &str = "ZWIIM_CLOUD_CA_CERT";

#[derive(Debug, thiserror::Error)]
pub enum CloudError {
    #[error(transparent)]
    Url(#[from] CloudUrlError),
    #[error("the Cloud is unreachable: {0}")]
    Unreachable(String),
    /// An HTTP error from the Cloud, with its error code.
    #[error("the Cloud refused the request: {code}")]
    Refused { status: u16, code: String, message: String },
    #[error("the host is offline")]
    HostOffline,
    #[error("the Cloud closed the connection ({0})")]
    Closed(u16),
    #[error("unexpected message from the Cloud")]
    Protocol,
}

/// TLS trust for Cloud connections.
#[derive(Clone, Default)]
pub struct Tls {
    extra_ca: Option<native_tls::Certificate>,
}

impl std::fmt::Debug for Tls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tls").field("extra_ca", &self.extra_ca.is_some()).finish()
    }
}

impl Tls {
    pub fn with_ca_pem(pem: &[u8]) -> Result<Self, String> {
        let ca = native_tls::Certificate::from_pem(pem).map_err(|e| format!("Cloud CA certificate: {e}"))?;
        Ok(Self { extra_ca: Some(ca) })
    }

    /// System roots, plus the CA in `ZWIIM_CLOUD_CA_CERT` if set.
    pub fn from_env() -> Result<Self, String> {
        match std::env::var(CA_CERT_ENV).ok().filter(|p| !p.is_empty()) {
            None => Ok(Self::default()),
            Some(path) => {
                let pem = std::fs::read(&path).map_err(|e| format!("{CA_CERT_ENV}: {e}"))?;
                Self::with_ca_pem(&pem)
            }
        }
    }

    fn connector(&self) -> Result<Option<Connector>, CloudError> {
        let Some(ca) = &self.extra_ca else { return Ok(None) };
        native_tls::TlsConnector::builder()
            .add_root_certificate(ca.clone())
            .build()
            .map(|c| Some(Connector::NativeTls(c)))
            .map_err(|e| CloudError::Unreachable(e.to_string()))
    }

    pub fn http(&self) -> Result<reqwest::Client, CloudError> {
        let mut builder = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(ca) = &self.extra_ca {
            let der = ca.to_der().map_err(|e| CloudError::Unreachable(e.to_string()))?;
            let cert = reqwest::Certificate::from_der(&der).map_err(|e| CloudError::Unreachable(e.to_string()))?;
            builder = builder.add_root_certificate(cert);
        }
        builder.build().map_err(|e| CloudError::Unreachable(e.to_string()))
    }
}

/// Open a WebSocket to `path` on the Cloud at `origin`, accepting messages
/// up to `max_message` bytes.
pub async fn connect(origin: &str, path: &str, max_message: usize, tls: &Tls) -> Result<CloudSocket, CloudError> {
    let origin = zwiim_cloud::proto::normalize_cloud_url(origin)?;
    let url = zwiim_cloud::proto::ws_url(&origin, path);
    let config = WebSocketConfig::default()
        .max_message_size(Some(max_message))
        .max_frame_size(Some(max_message));
    let connecting = tokio_tungstenite::connect_async_tls_with_config(url, Some(config), true, tls.connector()?);
    match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(e)) => Err(CloudError::Unreachable(e.to_string())),
        Err(_) => Err(CloudError::Unreachable("connection timed out".into())),
    }
}

/// The next text message, within `timeout`. A close is reported with its
/// code.
pub async fn recv_text(ws: &mut CloudSocket, timeout: Duration) -> Result<String, CloudError> {
    let next = async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => return Ok(text.to_string()),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(Message::Close(frame))) => {
                    return Err(CloudError::Closed(frame.map(|f| u16::from(f.code)).unwrap_or(1005)))
                }
                Some(Ok(_)) => return Err(CloudError::Protocol),
                Some(Err(e)) => return Err(CloudError::Unreachable(e.to_string())),
                None => return Err(CloudError::Closed(1006)),
            }
        }
    };
    tokio::time::timeout(timeout, next)
        .await
        .map_err(|_| CloudError::Unreachable("the Cloud did not answer in time".into()))?
}

pub async fn send_json<T: serde::Serialize>(ws: &mut CloudSocket, msg: &T) -> Result<(), CloudError> {
    use futures_util::SinkExt;
    let text = serde_json::to_string(msg).expect("serializable");
    ws.send(Message::Text(text.into()))
        .await
        .map_err(|e| CloudError::Unreachable(e.to_string()))
}

/// Turn a Cloud HTTP response into its body or a [`CloudError`].
pub async fn read_json<T: serde::de::DeserializeOwned>(res: reqwest::Response) -> Result<T, CloudError> {
    let status = res.status().as_u16();
    if (200..300).contains(&status) {
        return res.json().await.map_err(|_| CloudError::Protocol);
    }
    let body: Option<zwiim_cloud::proto::ErrorBody> = res.json().await.ok();
    let (code, message) = body
        .map(|b| (b.error.code, b.error.message))
        .unwrap_or_else(|| ("unknown".into(), format!("HTTP {status}")));
    if code == "host_offline" {
        return Err(CloudError::HostOffline);
    }
    Err(CloudError::Refused { status, code, message })
}

pub fn unreachable(e: reqwest::Error) -> CloudError {
    CloudError::Unreachable(e.without_url().to_string())
}
