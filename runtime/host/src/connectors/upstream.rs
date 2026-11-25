//! The Zwiim contract for one live MCP endpoint (later also OpenAPI).
//!
//! The rest of the connector platform talks to services only through
//! [`Upstream`] and the plain types here. Protocol libraries stay behind it:
//! rmcp types never cross this boundary, so an SDK upgrade can't change what
//! the executor, policy or audit code sees, and tool hashes are computed on
//! these types rather than on the SDK's.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

/// A tool as a service offers it, after the adapter dropped anything the
/// protocol carries that Zwiim doesn't forward. Serialized with MCP's field
/// names for the Host-to-proxy frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// The server's own hints (`readOnlyHint`, `destructiveHint`, …). Shown to
    /// the user and forwarded to agents; never used to grant anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<Value>,
}

impl ToolDefinition {
    /// `sha256:<hex>` over every field, so a changed description, schema or
    /// hint is a changed tool. Object keys are sorted, so the hash doesn't
    /// depend on the order a server sent them in.
    pub fn hash(&self) -> String {
        let mut fields = Map::new();
        fields.insert("name".into(), Value::String(self.name.clone()));
        let optional = [
            ("title", self.title.clone().map(Value::String)),
            ("description", self.description.clone().map(Value::String)),
            ("inputSchema", Some(self.input_schema.clone())),
            ("outputSchema", self.output_schema.clone()),
            ("annotations", self.annotations.clone()),
        ];
        for (key, value) in optional {
            if let Some(value) = value {
                fields.insert(key.into(), value);
            }
        }
        let mut canonical = String::new();
        write_sorted(&Value::Object(fields), &mut canonical);
        format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
    }
}

/// Compact JSON with object keys sorted at every level.
fn write_sorted(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_sorted(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_sorted(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// A service's complete tool list.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolListing {
    pub tools: Vec<ToolDefinition>,
    /// How long the server says the list stays fresh (2026-07-28 `ttlMs`).
    pub ttl: Option<Duration>,
}

/// One tool call to send.
#[derive(Clone, Debug, PartialEq)]
pub struct UpstreamCall {
    pub tool: String,
    pub arguments: Map<String, Value>,
}

/// What a service answered to a call that completed. Serialized with MCP's
/// field names for the Host-to-proxy frames.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallOutput {
    /// MCP content blocks as sent, passed on to the agent.
    pub content: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    /// The service ran the tool and reports that it failed (`isError`).
    #[serde(default)]
    pub is_error: bool,
}

/// Set by the adapter just before a call's request bytes leave the Host.
/// The executor reads it after a timeout or cancel to tell "never sent"
/// from "may have run".
#[derive(Debug, Default)]
pub struct DispatchMark(AtomicBool);

impl DispatchMark {
    pub fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn dispatched(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Why a listing, probe or connection attempt failed. Nothing ran on the
/// service.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConnectorError {
    #[error("could not reach the service: {0}")]
    Transport(String),
    #[error(
        "the service doesn't support a protocol version Zwiim speaks (it supports {supported:?})"
    )]
    UnsupportedVersion { supported: Vec<String> },
    #[error("the service sent something Zwiim couldn't use: {0}")]
    Protocol(String),
    #[error("the service answered with an error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("the connection to the service is closed")]
    Closed,
}

/// Why a call has no [`CallOutput`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CallFailure {
    /// Nothing left the Host; safe to report as not run.
    #[error("not sent: {0}")]
    NotSent(ConnectorError),
    /// The service answered with a JSON-RPC error: it got the call and refused or failed it.
    #[error("the service answered with an error {code}: {message}")]
    Rejected { code: i64, message: String },
    /// The request left the Host and no answer came back. The call may have
    /// run; it is never retried.
    #[error("outcome unknown: {0:?}")]
    OutcomeUnknown(UnknownCause),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownCause {
    Timeout,
    CancelledAfterDispatch,
    ConnectionLost,
    HostShutdown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownReason {
    SessionEnded,
    Idle,
    Revoked,
    HostShutdown,
}

/// A live MCP endpoint. Implemented once per protocol kind, never per
/// service. Callers bound every method with their own timeout and may drop
/// the future at any point; implementations must not retry on their own.
#[async_trait]
pub trait Upstream: Send + Sync {
    async fn list_tools(&self) -> Result<ToolListing, ConnectorError>;

    /// Sends one call. Marks `dispatch` immediately before the request is
    /// handed to the transport.
    async fn call(
        &self,
        call: UpstreamCall,
        dispatch: &DispatchMark,
    ) -> Result<CallOutput, CallFailure>;

    /// Liveness: `server/discover` (modern) or `ping` (legacy).
    async fn probe(&self) -> Result<(), ConnectorError>;

    /// Bumps each time the service reports that its tool list changed.
    fn tool_changes(&self) -> watch::Receiver<u64>;

    /// Orderly close: cancel in-flight requests, close streams, end the
    /// legacy session where the transport has one. Idempotent.
    async fn shutdown(&self, reason: ShutdownReason) -> Result<(), ConnectorError>;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ToolDefinition;

    fn tool() -> ToolDefinition {
        ToolDefinition {
            name: "create_issue".into(),
            title: Some("Create issue".into()),
            description: Some("Creates an issue".into()),
            input_schema: json!({ "type": "object", "properties": { "title": { "type": "string" } } }),
            output_schema: None,
            annotations: Some(json!({ "readOnlyHint": false })),
        }
    }

    /// Holds even if `serde_json`'s `preserve_order` feature gets switched on
    /// by another crate, which would keep keys in arrival order.
    #[test]
    fn hash_ignores_key_order() {
        let a = tool();
        let mut b = tool();
        b.input_schema =
            serde_json::from_str(r#"{"properties":{"title":{"type":"string"}},"type":"object"}"#)
                .unwrap();
        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn hash_changes_with_any_field() {
        let base = tool().hash();
        assert!(base.starts_with("sha256:") && base.len() == 7 + 64);
        let changes: [fn(&mut ToolDefinition); 6] = [
            |t| t.name.push('2'),
            |t| t.title = None,
            |t| t.description = Some("Deletes the repository".into()),
            |t| t.input_schema["properties"]["body"] = json!({ "type": "string" }),
            |t| t.output_schema = Some(json!({ "type": "object" })),
            |t| t.annotations = Some(json!({ "readOnlyHint": true })),
        ];
        for change in changes {
            let mut changed = tool();
            change(&mut changed);
            assert_ne!(changed.hash(), base, "{changed:?}");
        }
    }

    #[test]
    fn missing_and_null_optional_fields_differ() {
        let mut absent = tool();
        absent.output_schema = None;
        let mut null = tool();
        null.output_schema = Some(serde_json::Value::Null);
        assert_ne!(absent.hash(), null.hash());
    }
}
