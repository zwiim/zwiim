//! The device side of the Cloud: sign in with an account token, find the
//! account's hosts, and dial one through the relay. The result is a byte
//! stream for the existing tunnel handshake
//! (`DeviceIdentity::connect_tunnel`), which authenticates the host by the
//! key pinned at pairing — not by anything the Cloud says.
//!
//! Lives here, like `identity::device`, until a client crate exists.

use tokio::io::DuplexStream;
use zwiim_cloud::proto::{
    self, EnrollmentTokenResponse, HostInfo, RelayHello, RelayReady, TicketResponse, CLOSE_HOST_OFFLINE,
    ENROLLMENTS_PATH, HOSTS_PATH, MAX_RELAY_MESSAGE, RELAY_DEVICE_PATH,
};

use super::transport::{self, read_json, unreachable, CloudError, Tls};
use super::HELLO_TIMEOUT;

/// An account signed in to a Cloud. The access token is the identity
/// provider's (Supabase); it is only ever sent to the Cloud and grants
/// nothing on a host.
pub struct CloudAccount {
    origin: String,
    access_token: String,
    tls: Tls,
    http: reqwest::Client,
}

impl std::fmt::Debug for CloudAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudAccount").field("origin", &self.origin).finish_non_exhaustive()
    }
}

impl CloudAccount {
    pub fn new(cloud_url: &str, access_token: &str, tls: Tls) -> Result<Self, CloudError> {
        Ok(Self {
            origin: proto::normalize_cloud_url(cloud_url)?,
            access_token: access_token.to_string(),
            http: tls.http()?,
            tls,
        })
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// A single-use token for enrolling a host into this account.
    pub async fn enrollment_token(&self) -> Result<EnrollmentTokenResponse, CloudError> {
        let res = self
            .http
            .post(format!("{}{ENROLLMENTS_PATH}", self.origin))
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(unreachable)?;
        read_json(res).await
    }

    /// The account's hosts and whether each is online.
    pub async fn hosts(&self) -> Result<Vec<HostInfo>, CloudError> {
        let res = self
            .http
            .get(format!("{}{HOSTS_PATH}", self.origin))
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(unreachable)?;
        read_json(res).await
    }

    /// A single-use connection ticket for `host_id`.
    pub async fn ticket(&self, host_id: &str) -> Result<TicketResponse, CloudError> {
        let res = self
            .http
            .post(format!("{}{}", self.origin, proto::tickets_path(host_id)))
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(unreachable)?;
        read_json(res).await
    }

    /// Ticket, then dial: a relayed byte stream to `host_id`.
    pub async fn connect(&self, host_id: &str) -> Result<DuplexStream, CloudError> {
        let ticket = self.ticket(host_id).await?;
        dial(&self.origin, &ticket.ticket, &self.tls).await
    }
}

/// Present `ticket` on the device relay socket and wait for the host.
pub async fn dial(origin: &str, ticket: &str, tls: &Tls) -> Result<DuplexStream, CloudError> {
    let mut ws = transport::connect(origin, RELAY_DEVICE_PATH, MAX_RELAY_MESSAGE, tls).await?;
    transport::send_json(&mut ws, &RelayHello::Device { ticket: ticket.to_string() }).await?;
    // The Cloud waits for the host to dial back before answering.
    match transport::recv_text(&mut ws, HELLO_TIMEOUT * 2).await {
        Ok(text) => match serde_json::from_str::<RelayReady>(&text) {
            Ok(RelayReady::Ready) => Ok(crate::tunnel::relay::stream(ws)),
            Err(_) => Err(CloudError::Protocol),
        },
        Err(CloudError::Closed(CLOSE_HOST_OFFLINE)) => Err(CloudError::HostOffline),
        Err(e) => Err(e),
    }
}
