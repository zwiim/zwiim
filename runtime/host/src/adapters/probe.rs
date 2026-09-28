//! Health probe: start the adapter and complete the ACP `initialize`
//! handshake. It proves the installed tree loads and speaks ACP on this
//! machine, without a session and without credentials.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// What the adapter said about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeInfo {
    pub protocol_version: u64,
    /// `agentInfo.version`, when the adapter reports it.
    pub agent_version: Option<String>,
}

pub async fn initialize(node: &Path, entry: &Path, limit: Duration) -> Result<ProbeInfo, String> {
    let scratch = tempfile::tempdir().map_err(|e| format!("probe scratch dir: {e}"))?;
    let mut child = crate::process::tokio_command(node)
        .arg(entry)
        .current_dir(scratch.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("could not start the adapter: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");

    let request = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false,
            },
        },
    });
    let handshake = async {
        stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .map_err(|e| format!("could not talk to the adapter: {e}"))?;
        stdin.flush().await.ok();
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| format!("reading the adapter: {e}"))?
        {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if msg["id"] != json!(0) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(format!("the adapter refused initialize: {err}"));
            }
            let result = &msg["result"];
            let protocol_version = result["protocolVersion"]
                .as_u64()
                .ok_or_else(|| format!("unexpected initialize answer: {line}"))?;
            return Ok(ProbeInfo {
                protocol_version,
                agent_version: result["agentInfo"]["version"].as_str().map(String::from),
            });
        }
        Err("the adapter exited before answering".to_string())
    };
    let outcome = match tokio::time::timeout(limit, handshake).await {
        Ok(outcome) => outcome,
        Err(_) => Err(format!(
            "the adapter did not answer within {}s",
            limit.as_secs()
        )),
    };
    let _ = child.start_kill();
    if outcome.is_err() {
        // The adapter's own explanation, when it gave one.
        let mut tail = String::new();
        let _ =
            tokio::time::timeout(Duration::from_secs(1), stderr.read_to_string(&mut tail)).await;
        let tail = tail.trim();
        if !tail.is_empty() {
            let last: String = tail.lines().rev().take(3).collect::<Vec<_>>().join(" | ");
            return outcome.map_err(|e| format!("{e} ({last})"));
        }
    }
    let _ = child.wait().await;
    outcome
}
