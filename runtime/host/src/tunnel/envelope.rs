//! Messages inside the tunnel (JSON, one per tunnel message).
//!
//! ```text
//! req       device → host  { t, id, op, body }      POST /zwiim/v1/<op>
//! res       host → device  { t, id, status, body }  its response
//! ws.open   device → host  { t, id }                open a /zwiim/v1/ws socket
//!           host → device  { t, id, status, body? } 101, or the refusal
//! ws.msg    both ways      { t, id, data }          one text frame, verbatim
//! ws.close  both ways      { t, id, code?, reason? }
//! ```
//!
//! `id` correlates a `res` with its `req`, and names an event stream for its
//! `ws.*` messages; the device picks both. Bodies and frames are exactly the
//! `/zwiim/v1` ones.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", deny_unknown_fields)]
pub enum Envelope {
    #[serde(rename = "req")]
    Req {
        id: u64,
        op: String,
        #[serde(default = "empty_body")]
        body: Value,
    },
    #[serde(rename = "res")]
    Res { id: u64, status: u16, body: Value },
    #[serde(rename = "ws.open")]
    WsOpen {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
    },
    #[serde(rename = "ws.msg")]
    WsMsg { id: u64, data: String },
    #[serde(rename = "ws.close")]
    WsClose {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<u16>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

fn empty_body() -> Value {
    Value::Object(Default::default())
}

impl Envelope {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("envelopes serialize")
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn wire_shape() {
        let cases = [
            (
                Envelope::Req { id: 1, op: "workspace.list".into(), body: json!({}) },
                json!({"t": "req", "id": 1, "op": "workspace.list", "body": {}}),
            ),
            (
                Envelope::Res { id: 1, status: 200, body: json!([]) },
                json!({"t": "res", "id": 1, "status": 200, "body": []}),
            ),
            (
                Envelope::WsOpen { id: 2, status: None, body: None },
                json!({"t": "ws.open", "id": 2}),
            ),
            (
                Envelope::WsOpen { id: 2, status: Some(101), body: None },
                json!({"t": "ws.open", "id": 2, "status": 101}),
            ),
            (
                Envelope::WsMsg { id: 2, data: "{\"op\":\"ping\"}".into() },
                json!({"t": "ws.msg", "id": 2, "data": "{\"op\":\"ping\"}"}),
            ),
            (
                Envelope::WsClose { id: 2, code: Some(4403), reason: Some("device_revoked".into()) },
                json!({"t": "ws.close", "id": 2, "code": 4403, "reason": "device_revoked"}),
            ),
            (
                Envelope::WsClose { id: 2, code: None, reason: None },
                json!({"t": "ws.close", "id": 2}),
            ),
        ];
        for (envelope, wire) in cases {
            assert_eq!(serde_json::to_value(&envelope).unwrap(), wire);
            assert_eq!(Envelope::decode(&envelope.encode()), Some(envelope));
        }
    }

    #[test]
    fn a_request_body_defaults_to_an_empty_object() {
        assert_eq!(
            Envelope::decode(br#"{"t":"req","id":7,"op":"host.hello"}"#),
            Some(Envelope::Req { id: 7, op: "host.hello".into(), body: json!({}) })
        );
    }

    #[test]
    fn anything_else_is_malformed() {
        for bytes in [
            &br#"{"t":"req","id":1,"op":"host.hello","path":"/api/x"}"#[..],
            br#"{"t":"http","id":1}"#,
            br#"{"id":1,"op":"host.hello"}"#,
            br#"{"t":"res","id":-1,"status":200,"body":null}"#,
            br#"{"t":"ws.msg","id":1,"data":{"op":"ping"}}"#,
            b"not json",
        ] {
            assert_eq!(Envelope::decode(bytes), None, "{}", String::from_utf8_lossy(bytes));
        }
    }
}
