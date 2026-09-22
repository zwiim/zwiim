use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::{Deserialize, Serialize};

use crate::app_error::AppCommandError;
use crate::app_state::AppState;
use crate::web::{
    do_get_web_server_status, do_probe_web_service_port, do_stop_web_server,
    load_web_service_config, update_web_service_config_core, WebServerInfo, WebServiceConfig,
    WebServicePortProbe,
};

pub async fn get_web_server_status(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<Option<WebServerInfo>>, AppCommandError> {
    Ok(Json(do_get_web_server_status(&state.web_server_state)))
}

pub async fn get_web_service_config(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<WebServiceConfig>, AppCommandError> {
    load_web_service_config(&state.db.conn).await.map(Json)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWebServiceConfigParams {
    pub config: WebServiceConfig,
}

pub async fn update_web_service_config(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<UpdateWebServiceConfigParams>,
) -> Result<Json<WebServiceConfig>, AppCommandError> {
    update_web_service_config_core(&state.db.conn, params.config)
        .await
        .map(Json)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartWebServerParams {
    pub port: Option<u16>,
    pub host: Option<String>,
    pub token: Option<String>,
}

pub async fn start_web_server(
    Extension(state): Extension<Arc<AppState>>,
    Json(_params): Json<StartWebServerParams>,
) -> Result<Json<WebServerInfo>, AppCommandError> {
    // In web mode, the server is already running (this handler itself is served by it).
    // This endpoint is mainly useful in Tauri mode. Return current status as a noop.
    let ws = &state.web_server_state;
    if ws.running.load(std::sync::atomic::Ordering::Relaxed) {
        if let Some(info) = do_get_web_server_status(ws) {
            return Ok(Json(info));
        }
    }
    Err(AppCommandError::new(
        crate::app_error::AppErrorCode::InvalidInput,
        "Cannot start web server from within web mode",
    ))
}

pub async fn stop_web_server(
    Extension(state): Extension<Arc<AppState>>,
) -> Result<Json<()>, AppCommandError> {
    // In web mode the serve task is owned by `zwiim-host`'s main loop,
    // not WebServerState. Calling do_stop_web_server here would not stop
    // the process but WOULD trigger shutdown_signal — killing every live
    // WebSocket including the caller's own session. Reject instead.
    if state.web_server_state.is_externally_managed() {
        return Err(AppCommandError::new(
            crate::app_error::AppErrorCode::InvalidInput,
            "Cannot stop web server from within web mode",
        ));
    }
    do_stop_web_server(&state.web_server_state).await;
    Ok(Json(()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeWebServicePortParams {
    pub port: Option<u16>,
}

pub async fn probe_web_service_port(
    Extension(state): Extension<Arc<AppState>>,
    Json(params): Json<ProbeWebServicePortParams>,
) -> Result<Json<WebServicePortProbe>, AppCommandError> {
    do_probe_web_service_port(&state.db.conn, params.port)
        .await
        .map(Json)
}
