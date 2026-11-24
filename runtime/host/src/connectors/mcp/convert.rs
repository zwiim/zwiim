//! rmcp types to Zwiim's. Done through each type's JSON form, which the MCP
//! schema fixes, so an rmcp upgrade that reshapes its Rust types can't change
//! what Zwiim sees or how tools hash.

use std::time::Duration;

use rmcp::model::{CallToolResult, Tool};
use rmcp::service::{ClientInitializeError, ServiceError};
use serde_json::Value;

use crate::connectors::egress::EgressError;
use crate::connectors::upstream::{CallOutput, ConnectorError, ToolDefinition};

pub(super) fn tool(tool: &Tool) -> Result<ToolDefinition, ConnectorError> {
    let value = serde_json::to_value(tool).map_err(|e| ConnectorError::Protocol(e.to_string()))?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let object = |key: &str| value.get(key).cloned();
    Ok(ToolDefinition {
        name: text("name").ok_or_else(|| ConnectorError::Protocol("a tool has no name".into()))?,
        title: text("title"),
        description: text("description"),
        input_schema: object("inputSchema").unwrap_or(Value::Null),
        output_schema: object("outputSchema"),
        annotations: object("annotations"),
    })
}

pub(super) fn ttl(ttl_ms: Option<u64>) -> Option<Duration> {
    ttl_ms.map(Duration::from_millis)
}

pub(super) fn call_output(result: &CallToolResult) -> Result<CallOutput, ConnectorError> {
    let value =
        serde_json::to_value(result).map_err(|e| ConnectorError::Protocol(e.to_string()))?;
    Ok(CallOutput {
        content: value
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        structured_content: value.get("structuredContent").cloned(),
        is_error: value
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

/// The egress refusal behind an rmcp error, if that is what stopped it.
pub(super) fn egress_refusal(error: &(dyn std::error::Error + 'static)) -> Option<EgressError> {
    let mut source = Some(error);
    while let Some(cause) = source {
        if let Some(refusal) = cause.downcast_ref::<EgressError>() {
            return Some(refusal.clone());
        }
        source = cause.source();
    }
    None
}

/// Whether nothing of the request can have left the Host.
pub(super) fn never_sent(error: &EgressError) -> bool {
    matches!(
        error,
        EgressError::NotAllowed { .. }
            | EgressError::RefusedAddress { .. }
            | EgressError::CredentialsNotAllowed { .. }
            | EgressError::CredentialHeader(_)
            | EgressError::InvalidRule { .. }
            | EgressError::Resolve { .. }
    )
}

pub(super) fn service_error(error: ServiceError) -> ConnectorError {
    match error {
        ServiceError::McpError(error) => ConnectorError::Rpc {
            code: i64::from(error.code.0),
            message: error.message.to_string(),
        },
        ServiceError::TransportClosed => ConnectorError::Closed,
        ServiceError::TransportSend(error) => ConnectorError::Transport(
            egress_refusal(error.error.as_ref())
                .map(|refusal| refusal.to_string())
                .unwrap_or_else(|| chain(error.error.as_ref())),
        ),
        other => ConnectorError::Protocol(other.to_string()),
    }
}

pub(super) fn init_error(error: ClientInitializeError) -> ConnectorError {
    match error {
        ClientInitializeError::NoCompatibleProtocolVersion {
            server_supported, ..
        } => ConnectorError::UnsupportedVersion {
            supported: server_supported
                .iter()
                .map(|v| v.as_str().to_string())
                .collect(),
        },
        ClientInitializeError::JsonRpcError(error) => ConnectorError::Rpc {
            code: i64::from(error.code.0),
            message: error.message.to_string(),
        },
        ClientInitializeError::ConnectionClosed(_) => ConnectorError::Closed,
        ClientInitializeError::TransportError { error, .. } => ConnectorError::Transport(
            egress_refusal(error.error.as_ref())
                .map(|refusal| refusal.to_string())
                .unwrap_or_else(|| chain(error.error.as_ref())),
        ),
        // Neither era worked; the legacy attempt says more about why.
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => init_error(*fallback),
        other => ConnectorError::Protocol(other.to_string()),
    }
}

fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message = format!("{message}: {cause}");
        source = cause.source();
    }
    message
}
