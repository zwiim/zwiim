//! What a session tab's page fetched (design 10.2): a ring of its requests
//! for `browser_network_log`, written out as HAR when asked, and what is
//! still in flight, so an action can wait for the network to go quiet.
//!
//! Cookie, `Authorization` and `Set-Cookie` headers never enter the ring,
//! and neither do request or response bodies.

use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Requests kept per tab.
pub const NETWORK_RING_CAPACITY: usize = 500;
/// Requests a read answers when not told; the newest ones.
pub const DEFAULT_NETWORK_LIMIT: usize = 100;
/// Longest address kept, in characters.
const MAX_URL: usize = 2048;
/// Longest header value kept, in characters, and most headers kept.
const MAX_HEADER: usize = 1024;
const MAX_HEADERS: usize = 64;
/// Headers that carry credentials: never kept.
const SECRET_HEADERS: &[&str] = &[
    "cookie",
    "set-cookie",
    "authorization",
    "proxy-authorization",
];
/// Requests that stay open by design and never make the network busy.
const LONG_LIVED: &[&str] = &["EventSource", "WebSocket"];

/// One request as an agent reads it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEntry {
    /// Position in this tab's requests; a reader passes the last one it saw
    /// as `since`.
    pub seq: u64,
    pub method: String,
    pub url: String,
    /// The engine's resource type: `Document`, `XHR`, `Fetch`, `Script`, …
    #[serde(rename = "type")]
    pub kind: String,
    /// Absent while waiting for an answer, and for a request that failed
    /// without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    /// Unix milliseconds when it was sent.
    pub started: i64,
    /// From sent to done; absent while in flight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Bytes on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Why it failed, in the engine's words (`net::ERR_CONNECTION_REFUSED`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// For HAR only; credentials already removed.
    #[serde(skip)]
    request_headers: Vec<(String, String)>,
    #[serde(skip)]
    response_headers: Vec<(String, String)>,
    /// The engine's clock when it was sent, in seconds.
    #[serde(skip)]
    sent_at: f64,
    #[serde(skip)]
    done: bool,
}

/// What a read asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkQuery {
    /// Only requests after this `seq`. `0` is everything the ring holds.
    #[serde(default)]
    pub since: u64,
    /// Only requests whose address contains this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// Only requests that failed or were answered with a status of 400 or
    /// more.
    #[serde(default)]
    pub failed_only: bool,
    /// At most this many, the newest. Absent is [`DEFAULT_NETWORK_LIMIT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// What a read answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkReadout {
    /// Where the tab was when it was read.
    pub url: String,
    /// Oldest first.
    pub entries: Vec<NetworkEntry>,
    /// Older requests that matched but were left out for `limit`.
    pub omitted: usize,
    /// Requests no longer in the ring.
    pub dropped: u64,
    /// The `since` to pass next time to see only what came after.
    pub next_since: u64,
    /// Where the HAR file was written, when one was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub har: Option<String>,
}

/// One tab's requests.
#[derive(Debug, Default)]
pub struct NetworkRing {
    entries: VecDeque<NetworkEntry>,
    /// The engine's request id of each request not yet done, and its seq.
    open: HashMap<String, u64>,
    last_seq: u64,
    dropped: u64,
}

impl NetworkRing {
    /// Apply one `Network.*` event. `now` is unix milliseconds.
    pub fn on_event(&mut self, method: &str, p: &Value, now: i64) {
        let id = p["requestId"].as_str().unwrap_or_default();
        match method {
            "Network.requestWillBeSent" => {
                // A redirect reuses the id: the hop before it is done.
                if let Some(redirect) = p.get("redirectResponse").filter(|r| r.is_object()) {
                    if let Some(entry) = self.open_entry(id) {
                        entry.answered(redirect);
                    }
                    self.finish(id, p["timestamp"].as_f64(), None, None);
                }
                let request = &p["request"];
                self.last_seq += 1;
                if self.entries.len() >= NETWORK_RING_CAPACITY {
                    if let Some(old) = self.entries.pop_front() {
                        self.open.retain(|_, seq| *seq != old.seq);
                    }
                    self.dropped += 1;
                }
                self.open.insert(id.to_string(), self.last_seq);
                self.entries.push_back(NetworkEntry {
                    seq: self.last_seq,
                    method: request["method"].as_str().unwrap_or("GET").to_string(),
                    url: cut(request["url"].as_str().unwrap_or_default(), MAX_URL),
                    kind: p["type"].as_str().unwrap_or("Other").to_string(),
                    status: None,
                    mime: None,
                    started: p["wallTime"].as_f64().map_or(now, |s| (s * 1000.0) as i64),
                    duration_ms: None,
                    size: None,
                    error: None,
                    request_headers: headers(&request["headers"]),
                    response_headers: Vec::new(),
                    sent_at: p["timestamp"].as_f64().unwrap_or(0.0),
                    done: false,
                });
            }
            "Network.responseReceived" => {
                if let Some(entry) = self.open_entry(id) {
                    entry.answered(&p["response"]);
                }
            }
            "Network.loadingFinished" => {
                let size = p["encodedDataLength"].as_f64().map(|n| n as u64);
                self.finish(id, p["timestamp"].as_f64(), size, None);
            }
            "Network.loadingFailed" => {
                let error = match (p["errorText"].as_str(), p["blockedReason"].as_str()) {
                    (_, Some(blocked)) => format!("blocked: {blocked}"),
                    (Some(text), _) if p["canceled"] == true => format!("canceled: {text}"),
                    (Some(text), _) => text.to_string(),
                    (None, None) => "failed".to_string(),
                };
                self.finish(id, p["timestamp"].as_f64(), None, Some(error));
            }
            _ => {}
        }
    }

    fn open_entry(&mut self, id: &str) -> Option<&mut NetworkEntry> {
        let seq = *self.open.get(id)?;
        self.entries.iter_mut().find(|e| e.seq == seq)
    }

    fn finish(&mut self, id: &str, at: Option<f64>, size: Option<u64>, error: Option<String>) {
        if let Some(entry) = self.open_entry(id) {
            entry.done = true;
            if let Some(at) = at {
                entry.duration_ms = Some(((at - entry.sent_at).max(0.0) * 1000.0) as u64);
            }
            entry.size = size.or(entry.size);
            entry.error = error;
        }
        self.open.remove(id);
    }

    /// Requests still waiting on an answer, leaving out those that stay open
    /// by design.
    pub fn busy(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| !e.done && !LONG_LIVED.contains(&e.kind.as_str()))
            .count()
    }

    /// The seq of the newest request.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// The requests matching `query`, oldest first.
    pub fn read(&self, query: &NetworkQuery, url: &str) -> NetworkReadout {
        let limit = match query.limit {
            Some(0) | None => DEFAULT_NETWORK_LIMIT,
            Some(n) => n,
        };
        let matching: Vec<&NetworkEntry> = self
            .entries
            .iter()
            .filter(|e| e.seq > query.since)
            .filter(|e| query.filter.as_deref().is_none_or(|f| e.url.contains(f)))
            .filter(|e| !query.failed_only || e.failed())
            .collect();
        let omitted = matching.len().saturating_sub(limit);
        NetworkReadout {
            url: url.to_string(),
            entries: matching[omitted..].iter().map(|e| (*e).clone()).collect(),
            omitted,
            dropped: self.dropped,
            next_since: self.last_seq,
            har: None,
        }
    }
}

impl NetworkEntry {
    fn answered(&mut self, response: &Value) {
        self.status = response["status"].as_u64().map(|s| s as u16);
        self.mime = response["mimeType"].as_str().map(str::to_string);
        self.response_headers = headers(&response["headers"]);
    }

    fn failed(&self) -> bool {
        self.error.is_some() || self.status.is_some_and(|s| s >= 400)
    }
}

/// The headers of a CDP `Headers` object, without credentials.
fn headers(raw: &Value) -> Vec<(String, String)> {
    let Some(map) = raw.as_object() else {
        return Vec::new();
    };
    map.iter()
        .filter(|(name, _)| !SECRET_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
        .take(MAX_HEADERS)
        .map(|(name, value)| {
            (
                name.clone(),
                cut(value.as_str().unwrap_or_default(), MAX_HEADER),
            )
        })
        .collect()
}

fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// `entries` as a HAR 1.2 document.
pub fn har(entries: &[NetworkEntry]) -> Value {
    let pairs = |headers: &[(String, String)]| -> Vec<Value> {
        headers
            .iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect()
    };
    let entries: Vec<Value> = entries
        .iter()
        .map(|e| {
            let started = chrono::DateTime::from_timestamp_millis(e.started)
                .unwrap_or_default()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
            let time = e.duration_ms.unwrap_or(0);
            let mut entry = json!({
                "startedDateTime": started,
                "time": time,
                "request": {
                    "method": e.method,
                    "url": e.url,
                    "httpVersion": "",
                    "headers": pairs(&e.request_headers),
                    "queryString": [],
                    "cookies": [],
                    "headersSize": -1,
                    "bodySize": -1,
                },
                "response": {
                    "status": e.status.unwrap_or(0),
                    "statusText": "",
                    "httpVersion": "",
                    "headers": pairs(&e.response_headers),
                    "cookies": [],
                    "content": {
                        "size": e.size.unwrap_or(0),
                        "mimeType": e.mime.clone().unwrap_or_default(),
                    },
                    "redirectURL": "",
                    "headersSize": -1,
                    "bodySize": e.size.map_or(-1, |s| s as i64),
                },
                "cache": {},
                "timings": { "send": 0, "wait": time, "receive": 0 },
                "_resourceType": e.kind,
            });
            if let Some(error) = &e.error {
                entry["_error"] = json!(error);
            }
            entry
        })
        .collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "Zwiim", "version": env!("CARGO_PKG_VERSION") },
            "pages": [],
            "entries": entries,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(ring: &mut NetworkRing, id: &str, url: &str, kind: &str) {
        ring.on_event(
            "Network.requestWillBeSent",
            &json!({
                "requestId": id, "type": kind, "timestamp": 10.0, "wallTime": 1_700_000_000.0,
                "request": { "method": "POST", "url": url, "headers": {
                    "Authorization": "Bearer secret", "Cookie": "sid=1", "Accept": "*/*"
                } }
            }),
            0,
        );
    }

    #[test]
    fn requests_are_followed_to_their_end_without_their_secrets() {
        let mut ring = NetworkRing::default();
        sent(&mut ring, "1", "http://localhost:3000/api/orders", "Fetch");
        sent(&mut ring, "2", "http://localhost:3000/missing.js", "Script");
        sent(
            &mut ring,
            "3",
            "http://localhost:3000/events",
            "EventSource",
        );
        assert_eq!(ring.busy(), 2);
        ring.on_event(
            "Network.responseReceived",
            &json!({ "requestId": "1", "response": { "status": 201, "mimeType": "application/json",
                     "headers": { "Set-Cookie": "sid=2", "Content-Type": "application/json" } } }),
            0,
        );
        ring.on_event(
            "Network.loadingFinished",
            &json!({ "requestId": "1", "timestamp": 10.25, "encodedDataLength": 512 }),
            0,
        );
        ring.on_event(
            "Network.loadingFailed",
            &json!({ "requestId": "2", "timestamp": 10.5, "errorText": "net::ERR_CONNECTION_REFUSED" }),
            0,
        );
        assert_eq!(ring.busy(), 0, "an event stream does not keep it busy");

        let all = ring.read(&NetworkQuery::default(), "http://localhost:3000/");
        assert_eq!(all.entries.len(), 3);
        let order = &all.entries[0];
        assert_eq!(
            (order.status, order.size, order.duration_ms),
            (Some(201), Some(512), Some(250))
        );
        assert_eq!(order.started, 1_700_000_000_000);
        let failed = ring.read(
            &NetworkQuery {
                failed_only: true,
                ..Default::default()
            },
            "",
        );
        assert_eq!(failed.entries.len(), 1);
        assert_eq!(
            failed.entries[0].error.as_deref(),
            Some("net::ERR_CONNECTION_REFUSED")
        );
        let filtered = ring.read(
            &NetworkQuery {
                filter: Some("/api/".into()),
                ..Default::default()
            },
            "",
        );
        assert_eq!(filtered.entries.len(), 1);
        let later = ring.read(
            &NetworkQuery {
                since: all.next_since,
                ..Default::default()
            },
            "",
        );
        assert!(later.entries.is_empty());

        let text = har(&all.entries).to_string();
        assert!(text.contains("\"Accept\""), "{text}");
        for secret in ["secret", "sid=", "Authorization", "Cookie"] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        // The agent's view carries no headers at all.
        let view = serde_json::to_string(&all).unwrap();
        assert!(!view.contains("Accept"), "{view}");
    }

    #[test]
    fn a_redirect_ends_one_hop_and_starts_the_next() {
        let mut ring = NetworkRing::default();
        sent(&mut ring, "1", "http://localhost:3000/old", "Document");
        ring.on_event(
            "Network.requestWillBeSent",
            &json!({ "requestId": "1", "type": "Document", "timestamp": 10.1,
                     "redirectResponse": { "status": 302, "headers": {} },
                     "request": { "method": "GET", "url": "http://localhost:3000/new", "headers": {} } }),
            0,
        );
        let read = ring.read(&NetworkQuery::default(), "");
        assert_eq!(read.entries.len(), 2);
        assert_eq!(read.entries[0].status, Some(302));
        assert_eq!(ring.busy(), 1);
    }

    #[test]
    fn the_ring_keeps_the_newest_and_a_read_the_latest() {
        let mut ring = NetworkRing::default();
        for n in 0..(NETWORK_RING_CAPACITY + 10) {
            sent(&mut ring, &n.to_string(), "http://localhost/x", "Fetch");
        }
        let read = ring.read(&NetworkQuery::default(), "");
        assert_eq!(read.dropped, 10);
        assert_eq!(read.entries.len(), DEFAULT_NETWORK_LIMIT);
        assert_eq!(read.omitted, NETWORK_RING_CAPACITY - DEFAULT_NETWORK_LIMIT);
        assert_eq!(read.entries.last().unwrap().seq, ring.last_seq());
        assert!(ring.open.len() <= NETWORK_RING_CAPACITY);
    }
}
