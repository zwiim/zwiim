//! Connector proxies on an agent launch (connector design r3 §3.3). Each
//! connection the session is offered becomes one `zwiim-mcp --connector`
//! server, named by the connection's slug, with a token bound to (this
//! agent connection, that connection). Built once per launch, so
//! `session/new`, `load` and `resume` all get fresh tokens and the current
//! grants. The tokens go with the agent connection at teardown.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sacp::schema::{McpServer, McpServerStdio};

use crate::acp::connection::DelegationInjection;
use crate::acp::delegation::listener::TokenEntry;
use crate::models::agent::AgentType;

/// Adds a proxy to `servers` for each connection the session is offered,
/// returning their slugs. None before the Host's connections are made, for
/// engine sessions, or without the `zwiim-mcp` binary. A slug already in
/// `servers` (the user's own MCP server of that name) is left out.
pub(crate) async fn inject(
    servers: &mut Vec<McpServer>,
    injection: &DelegationInjection,
    parent_connection_id: &str,
    working_dir: &Path,
    agent_type: AgentType,
    engine: bool,
    locate_binary: impl FnOnce() -> Option<PathBuf>,
) -> Vec<String> {
    let Some(connectors) = injection.connectors.get() else {
        return Vec::new();
    };
    let Some(agent) = crate::acp::connector_cards::agent_name(agent_type) else {
        return Vec::new();
    };
    let offered = connectors.offered(working_dir, &agent, engine).await;
    if offered.is_empty() {
        return Vec::new();
    }
    let Some(binary) = locate_binary() else {
        tracing::warn!(
            "[connectors] zwiim-mcp not found; connection {parent_connection_id} gets no \
             connectors"
        );
        return Vec::new();
    };
    let mut taken: HashSet<String> = servers.iter().filter_map(server_name).collect();
    let mut injected = Vec::new();
    for connection in offered {
        if !taken.insert(connection.slug.clone()) {
            tracing::warn!(
                "[connectors] an MCP server is already named {}; connection {} isn't offered \
                 to {parent_connection_id}",
                connection.slug,
                connection.id
            );
            continue;
        }
        let token = uuid::Uuid::new_v4().to_string();
        injection
            .tokens
            .register(
                token.clone(),
                TokenEntry {
                    parent_connection_id: parent_connection_id.into(),
                    working_dir: working_dir.to_path_buf(),
                    engine: None,
                    connector: Some(connection.id.clone()),
                },
            )
            .await;
        servers.push(McpServer::Stdio(
            McpServerStdio::new(connection.slug.clone(), binary.clone()).args(vec![
                "--connector".to_string(),
                "--parent-connection-id".to_string(),
                parent_connection_id.to_string(),
                "--socket-path".to_string(),
                injection.socket_path.to_string_lossy().to_string(),
                "--token".to_string(),
                token,
                "--parent-pid".to_string(),
                std::process::id().to_string(),
            ]),
        ));
        injected.push(connection.slug);
    }
    injected
}

/// What the agent connection `parent_connection_id` held ends with it: its
/// proxies' tokens and what its user allowed for the session.
pub(crate) async fn end(injection: &DelegationInjection, parent_connection_id: &str) {
    injection
        .tokens
        .revoke_by_parent(parent_connection_id)
        .await;
    if let Some(connectors) = injection.connectors.get() {
        connectors.end_session(parent_connection_id);
    }
}

fn server_name(server: &McpServer) -> Option<String> {
    match server {
        McpServer::Stdio(s) => Some(s.name.clone()),
        McpServer::Http(s) => Some(s.name.clone()),
        McpServer::Sse(s) => Some(s.name.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
