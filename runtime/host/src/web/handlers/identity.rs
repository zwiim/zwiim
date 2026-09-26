//! Zwiim-owned identity endpoints: host id, QR pairing, trusted devices.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::identity::host::ensure_host_id;
use crate::identity::pairing::{self, PairingClaim, PairingGrant};
use crate::identity::trusted_devices::{self, TrustedDeviceInfo};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostIdentity {
    pub host_id: String,
}

pub async fn host_identity(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<HostIdentity>, AppCommandError> {
    Ok(Json(HostIdentity {
        host_id: ensure_host_id(&state.db.conn).await?,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingInvitationCreated {
    pub host_id: String,
    pub expires_at: DateTime<Utc>,
    /// JSON `PairingInvitation` — the exact string to encode in the QR.
    pub qr_payload: String,
}

pub async fn pairing_invitation_create(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<PairingInvitationCreated>, AppCommandError> {
    let host_id = ensure_host_id(&state.db.conn).await?;
    let addresses = crate::web::do_get_web_server_status(&state.web_server_state)
        .map(|info| info.addresses)
        .unwrap_or_default();
    let invitation = state.pairing_invitations.create(host_id, addresses);
    let qr_payload = serde_json::to_string(&invitation)
        .map_err(|e| AppCommandError::task_execution_failed(e.to_string()))?;
    Ok(Json(PairingInvitationCreated {
        host_id: invitation.host_id,
        expires_at: invitation.expires_at,
        qr_payload,
    }))
}

pub async fn pairing_claim(
    Extension(state): Extension<Arc<AppState>>,
    Json(claim): Json<PairingClaim>,
) -> Result<Json<PairingGrant>, AppCommandError> {
    let host_id = ensure_host_id(&state.db.conn).await?;
    Ok(Json(
        pairing::claim(&state.db.conn, &state.pairing_invitations, host_id, claim).await?,
    ))
}

pub async fn trusted_device_list(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Vec<TrustedDeviceInfo>>, AppCommandError> {
    Ok(Json(trusted_devices::list(&state.db.conn).await?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdParams {
    pub device_id: String,
}

pub async fn trusted_device_revoke(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<DeviceIdParams>,
) -> Result<Json<()>, AppCommandError> {
    if !trusted_devices::revoke(&state.db.conn, &params.device_id).await? {
        return Err(AppCommandError::not_found("Unknown device"));
    }
    tracing::info!("[identity] device {} revoked", params.device_id);
    Ok(Json(()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use axum::http::{HeaderName, HeaderValue, StatusCode};
    use axum_test::TestServer;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    use crate::app_state::AppState;
    use crate::identity::device::DeviceIdentity;
    use crate::identity::pairing::{PairingGrant, PairingInvitation};
    use crate::web::router::build_router;
    use crate::web::shutdown::ShutdownSignal;

    const HOST_TOKEN: &str = "host-token";

    /// Boot a host on the on-disk data dir `dir` (reusing it = host restart).
    async fn boot_host(dir: &Path) -> TestServer {
        let db = crate::db::init_database(dir, "test").await.unwrap();
        let state = Arc::new(AppState::new_for_test(db, dir.to_path_buf()));
        let router = build_router(
            state,
            HOST_TOKEN.to_string(),
            dir.to_path_buf(),
            Arc::new(ShutdownSignal::new()),
        );
        TestServer::new(router).unwrap()
    }

    async fn health(server: &TestServer, bearer: &str) -> StatusCode {
        server
            .post("/api/health")
            .authorization_bearer(bearer)
            .await
            .status_code()
    }

    /// WS upgrade auth via the token subprotocol. A request that passes auth
    /// reaches the WS handler (and fails the upgrade for lack of a real
    /// handshake); one that fails auth is a 401.
    async fn ws_status(server: &TestServer, bearer: &str) -> StatusCode {
        let protocol = format!(
            "zwiim-events, zwiim-token.{}",
            URL_SAFE_NO_PAD.encode(bearer)
        );
        server
            .get("/ws/events")
            .add_header(
                HeaderName::from_static("sec-websocket-protocol"),
                HeaderValue::from_str(&protocol).unwrap(),
            )
            .await
            .status_code()
    }

    async fn create_invitation(server: &TestServer) -> PairingInvitation {
        let created: serde_json::Value = server
            .post("/api/pairing_invitation_create")
            .authorization_bearer(HOST_TOKEN)
            .await
            .json();
        serde_json::from_str(created["qrPayload"].as_str().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn pair_reconnect_restart_and_revoke() {
        let host_dir = tempfile::tempdir().unwrap();
        let device_dir = tempfile::tempdir().unwrap();
        let device_file = device_dir.path().join("device.json");

        // Fresh host: stable host_id; local desktop works with the host token
        // and no pairing at all.
        let host = boot_host(host_dir.path()).await;
        let host_id = host
            .post("/api/host_identity")
            .authorization_bearer(HOST_TOKEN)
            .await
            .json::<serde_json::Value>()["hostId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(health(&host, HOST_TOKEN).await, StatusCode::OK);

        // Unpaired device is rejected.
        assert_eq!(health(&host, "made-up").await, StatusCode::UNAUTHORIZED);

        // Host creates an invitation; the QR payload names this host.
        let invitation = create_invitation(&host).await;
        assert_eq!(invitation.host_id, host_id);

        // Device scans the QR and claims with its persistent device_id.
        let mut device = DeviceIdentity::load_or_create(&device_file, "Phone").unwrap();
        let grant: PairingGrant = host
            .post("/api/pairing_claim")
            .json(&json!({
                "code": invitation.code,
                "deviceId": device.device_id,
                "deviceName": device.device_name,
            }))
            .await
            .json();
        assert_eq!(grant.host_id, host_id);
        assert_eq!(grant.device_id, device.device_id);
        device
            .record_pairing(&device_file, &grant, invitation.addresses.clone())
            .unwrap();

        // The invitation is single-use.
        let replay = host
            .post("/api/pairing_claim")
            .json(&json!({ "code": invitation.code, "deviceId": "other-device" }))
            .expect_failure()
            .await;
        assert_eq!(replay.status_code(), StatusCode::FORBIDDEN);

        // Paired device appears as trusted.
        let devices: serde_json::Value = host
            .post("/api/trusted_device_list")
            .authorization_bearer(HOST_TOKEN)
            .await
            .json();
        assert_eq!(devices.as_array().unwrap().len(), 1);
        assert_eq!(devices[0]["deviceId"], device.device_id);
        assert_eq!(devices[0]["status"], "trusted");

        // Host + device restart: same host_id, same device_id, device
        // reconnects over HTTP and WS with its stored credential.
        drop(host);
        let host = boot_host(host_dir.path()).await;
        let device = DeviceIdentity::load_or_create(&device_file, "ignored").unwrap();
        assert_eq!(device.device_id, grant.device_id);
        let credential = device.credential_for(&host_id).unwrap().to_string();
        let restarted_host_id = host
            .post("/api/host_identity")
            .authorization_bearer(&credential)
            .await
            .json::<serde_json::Value>()["hostId"]
            .clone();
        assert_eq!(restarted_host_id, host_id.as_str());
        assert_eq!(health(&host, &credential).await, StatusCode::OK);
        assert_ne!(
            ws_status(&host, &credential).await,
            StatusCode::UNAUTHORIZED
        );

        // Revoke: the device is rejected on HTTP and WS; local desktop is not.
        host.post("/api/trusted_device_revoke")
            .authorization_bearer(HOST_TOKEN)
            .json(&json!({ "deviceId": device.device_id }))
            .await;
        assert_eq!(health(&host, &credential).await, StatusCode::UNAUTHORIZED);
        assert_eq!(
            ws_status(&host, &credential).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(health(&host, HOST_TOKEN).await, StatusCode::OK);

        let devices: serde_json::Value = host
            .post("/api/trusted_device_list")
            .authorization_bearer(HOST_TOKEN)
            .await
            .json();
        assert_eq!(devices[0]["status"], "revoked");

        // Revoking an unknown device is a 404.
        let unknown = host
            .post("/api/trusted_device_revoke")
            .authorization_bearer(HOST_TOKEN)
            .json(&json!({ "deviceId": "nope" }))
            .expect_failure()
            .await;
        assert_eq!(unknown.status_code(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pairing_management_requires_authentication() {
        let host_dir = tempfile::tempdir().unwrap();
        let host = boot_host(host_dir.path()).await;
        for path in [
            "/api/host_identity",
            "/api/pairing_invitation_create",
            "/api/trusted_device_list",
            "/api/trusted_device_revoke",
        ] {
            let res = host.post(path).expect_failure().await;
            assert_eq!(res.status_code(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }
}
