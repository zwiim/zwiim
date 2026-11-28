//! The connector benchmark through the agent proxy (design r3 §8.1, the
//! Phase 0 gate for ADR-2): real `zwiim-mcp --connector` processes, spoken
//! to over stdio the way an agent does, against the Host's delegation
//! listener, connectors and fake upstreams.
//!
//! ```text
//! cargo build --release --bin zwiim-mcp
//! cargo test --release -p zwiim-host --lib connectors::bench::proxy -- --ignored --nocapture
//! ```
//!
//! One proxy per connector is the topology (ADR-2, decided on these numbers
//! 2026-11-28); a multiplexed proxy would be built only if this one missed
//! the ADR-17 budget. Memory for 8 warm connectors is held to 30 MiB. As in
//! the Host-only benchmark, each fake answers at once in a child process, so
//! every round trip counts as Zwiim's: an upper bound.
//!
//! The ADR-17 budget and the 30 MiB limit are for a release build, which
//! the run above checks. CI runs the debug build its tests already compiled
//! as a regression check (`ZWIIM_BENCH_CHECK=1`): each p50 and the memory
//! figure may be at most 20 % over `proxy_baseline.json`, which holds the CI
//! runner's own debug numbers. Medians, because a shared runner's tail
//! moves by more than 20 % from run to run. A run prints its numbers in
//! that file's shape (the `BENCH` line); after a change that is meant to
//! cost more, paste the CI run's line into the file.
//!
//! ```text
//! "agent" (this test) ──stdio──▶ zwiim-mcp --connector ──UDS──▶ listener
//!                                  (one per connector)            │
//!                                                     Connectors ─┴─HTTP─▶ fake
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::{footprint, footprint_of, Row, CALLS, CHILD_ENV, CONNECTIONS, MAX, MEMORY_TARGET};
use super::{P95_TARGET, P99_TARGET};
use crate::acp::delegation::listener::tests::{make_browser_listener, StubBrowser};
use crate::acp::delegation::listener::{TokenEntry, TokenRegistry};
use crate::connectors::credentials::MemorySecrets;
use crate::connectors::definition::{UserAuth, UserConnector};
use crate::connectors::egress::{NetworkClass, SystemResolver};
use crate::connectors::grants::{self, Fallback, Scope};
use crate::connectors::runtime::{Connectors, McpFactory};

/// Sessions started per case; each spawns its proxies anew.
const STARTS: usize = 50;

/// How much worse than the baseline a CI run may be (design r3 §8.1).
const TOLERANCE: f64 = 1.2;
/// Below this a change is the runner's noise, not Zwiim's: 20 % of a
/// half-millisecond call is a tenth of a millisecond.
const NOISE_US: u64 = 1_000;

/// What a run measured, in the shape of `proxy_baseline.json`.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct Measured {
    /// `debug` or `release`: only the same build compares.
    profile: String,
    /// p50 per case, in microseconds, keyed like `x8 warm call`.
    p50_us: BTreeMap<String, u64>,
    /// 8 warm connectors, Host growth plus proxies; null where unmeasured.
    memory_kib: Option<u64>,
}

/// Each figure in `run` more than [`TOLERANCE`] over `baseline`. A case
/// the baseline lacks is a miss too, so a new case can't pass unchecked.
fn regressions(run: &Measured, baseline: &Measured) -> Vec<String> {
    if run.profile != baseline.profile {
        return vec![format!(
            "a {} build, against a {} baseline",
            run.profile, baseline.profile
        )];
    }
    let mut misses = Vec::new();
    for (case, &us) in &run.p50_us {
        match baseline.p50_us.get(case) {
            None => misses.push(format!("{case}: no baseline")),
            Some(&base) => {
                let allowed = ((base as f64 * TOLERANCE) as u64).max(base + NOISE_US);
                if us > allowed {
                    misses.push(format!("{case}: p50 {us} µs, baseline {base} µs"));
                }
            }
        }
    }
    match (run.memory_kib, baseline.memory_kib) {
        (Some(kib), Some(base)) if kib as f64 > base as f64 * TOLERANCE => {
            misses.push(format!("memory: {kib} KiB, baseline {base} KiB"));
        }
        (Some(_), None) => misses.push("memory: no baseline".into()),
        _ => {}
    }
    misses
}

/// One proxy, as an agent sees it: a child process speaking MCP over stdio.
struct Proxy {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next: u64,
}

impl Proxy {
    fn spawn(binary: &Path, socket: &Path, token: &str) -> Self {
        let mut child = Command::new(binary)
            .args(["--connector", "--parent-connection-id", "bench"])
            .arg("--socket-path")
            .arg(socket)
            .args(["--token", token])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        Self {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
            next: 0,
        }
    }

    async fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
    }

    /// The answer to one request, skipping notifications.
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let line = self
                .stdout
                .next_line()
                .await
                .unwrap()
                .expect("the proxy answers");
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] == id {
                assert!(message.get("error").is_none(), "{method}: {message}");
                return message["result"].clone();
            }
        }
    }

    /// Handshake and tool list, as an agent does when its session starts.
    async fn start(&mut self) {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "bench", "version": "1" },
            }),
        )
        .await;
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        let listed = self.request("tools/list", json!({})).await;
        assert_eq!(listed["tools"][0]["name"], "echo");
    }

    async fn call(&mut self) -> Duration {
        let at = Instant::now();
        let result = self
            .request(
                "tools/call",
                json!({ "name": "echo", "arguments": { "text": "hi" } }),
            )
            .await;
        let took = at.elapsed();
        assert_eq!(result["isError"], false, "{result}");
        took
    }
}

/// `n` proxies spawned at once, each started.
async fn started(binary: &Path, socket: &Path, n: usize) -> Vec<Proxy> {
    let starts = (0..n).map(|i| async move {
        let mut proxy = Proxy::spawn(binary, socket, &format!("proxy-{i}"));
        proxy.start().await;
        proxy
    });
    futures::future::join_all(starts).await
}

/// `zwiim-mcp`, built in the same profile directory as this test binary:
/// one of its parents (`target/<profile>/deps/…`, or deeper where cargo
/// keeps test binaries under `build/`).
fn proxy_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let name = format!("zwiim-mcp{}", std::env::consts::EXE_SUFFIX);
    let binary = exe
        .ancestors()
        .skip(1)
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file());
    assert!(
        binary.is_some(),
        "build it first: cargo build {}--bin zwiim-mcp",
        if cfg!(debug_assertions) {
            ""
        } else {
            "--release "
        }
    );
    binary.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark; run explicitly after building zwiim-mcp"]
async fn connector_proxy_overhead() {
    let binary = proxy_binary();
    let mut fakes = Command::new(std::env::current_exe().unwrap())
        .args([
            "connectors::bench::fake_servers",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(fakes.stdout.take().unwrap()).lines();
    let mut urls = Vec::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        if line == "FAKES READY" {
            break;
        }
        if let Some(port) = line.strip_prefix("FAKE modern ") {
            urls.push(format!("http://127.0.0.1:{port}/mcp"));
        }
    }
    assert_eq!(urls.len(), MAX, "fakes");
    let before = footprint();

    // The Host side: MAX connectors every session may run, one token each.
    let db = crate::db::test_helpers::fresh_in_memory_db().await;
    let connectors = Arc::new(Connectors::new(
        db.conn.clone(),
        Arc::new(McpFactory::new(Arc::new(SystemResolver))),
        Arc::new(MemorySecrets::default()),
    ));
    let tokens = Arc::new(TokenRegistry::default());
    for (i, url) in urls.iter().enumerate() {
        let user = UserConnector {
            name: format!("Fake {i}"),
            url: url.clone(),
            auth: UserAuth::Bearer,
            network: NetworkClass::Local,
        };
        let created = connectors.create(user, "Work", Some("k3y")).await.unwrap();
        created.validation.unwrap();
        let grant = grants::set_grant(
            &db.conn,
            &created.id,
            Scope::default(),
            Fallback::Ask,
            Fallback::Ask,
        )
        .await
        .unwrap();
        grants::allow_all(&db.conn, grant.id).await.unwrap();
        let entry = TokenEntry {
            parent_connection_id: "bench".into(),
            working_dir: PathBuf::from("/tmp"),
            engine: None,
            connector: Some(created.id),
        };
        tokens.register(format!("proxy-{i}"), entry).await;
    }
    let listener = make_browser_listener(tokens, Arc::new(StubBrowser::default()));
    listener.set_connectors(connectors);
    let dir = tempfile::Builder::new()
        .prefix("zcb")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("d.sock");
    let serving = tokio::spawn(listener.run(socket.clone()));
    while !socket.exists() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!("\nconnector overhead through the proxy, {profile} build");
    println!(
        "{:>5} {:<13} {:>6} {:>9} {:>9} {:>9}",
        "conns", "case", "n", "p50", "p95", "p99"
    );
    // The first launch of a new build waits on macOS's check of the binary
    // (about half a second, once per install or update); every later one
    // is a few milliseconds. Shown, not counted.
    let at = Instant::now();
    drop(started(&binary, &socket, 1).await);
    println!(
        "{:>5} {:<13} {:>6} {:>9.2?}",
        1,
        "first launch",
        1,
        at.elapsed()
    );
    let mut misses = Vec::new();
    let mut measured = Measured {
        profile: profile.into(),
        ..Measured::default()
    };
    let mut warm_eight = Vec::new();
    for n in CONNECTIONS {
        let mut starts = Vec::new();
        let mut firsts = Vec::new();
        for _ in 0..STARTS {
            let at = Instant::now();
            let mut proxies = started(&binary, &socket, n).await;
            starts.push(at.elapsed());
            for proxy in &mut proxies {
                firsts.push(proxy.call().await);
            }
        }
        let mut proxies = started(&binary, &socket, n).await;
        let mut warm = Vec::with_capacity(CALLS);
        for i in 0..CALLS {
            warm.push(proxies[i % n].call().await);
        }
        for (case, samples) in [
            ("session start", starts),
            ("first call", firsts),
            ("warm call", warm),
        ] {
            let row = Row::new(samples);
            println!(
                "{n:>5} {case:<13} {:>6} {:>9.2?} {:>9.2?} {:>9.2?}",
                row.n, row.p50, row.p95, row.p99
            );
            let slow = row.p95 > P95_TARGET || row.p99 > P99_TARGET;
            if slow && profile == "release" {
                misses.push(format!("x{n} {case}: p95 {:?}, p99 {:?}", row.p95, row.p99));
            }
            measured
                .p50_us
                .insert(format!("x{n} {case}"), row.p50.as_micros() as u64);
        }
        if n == MAX {
            warm_eight = proxies;
        }
    }

    // Read while the eight are still open and warm.
    let host = before.zip(footprint()).map(|(b, a)| a.saturating_sub(b));
    let proxies: Option<u64> = warm_eight
        .iter()
        .map(|p| p.child.id().and_then(footprint_of))
        .sum();
    match (host, proxies) {
        (Some(host), Some(proxies)) => {
            let total = host + proxies;
            let over = total > MEMORY_TARGET && profile == "release";
            println!(
                "\nmemory, {MAX} warm connectors: Host +{} KiB, {MAX} proxies {} KiB, \
                 together {} KiB (limit {} MiB for a release build{})",
                host / 1024,
                proxies / 1024,
                total / 1024,
                MEMORY_TARGET / (1024 * 1024),
                if over { ", MISSED" } else { "" },
            );
            if over {
                misses.push(format!("memory: {} KiB", total / 1024));
            }
            measured.memory_kib = Some(total / 1024);
        }
        _ => println!("\nmemory: not measured on this platform"),
    }

    println!("\nBENCH {}", serde_json::to_string(&measured).unwrap());

    drop(warm_eight);
    serving.abort();
    drop(fakes.stdin.take());
    let _ = fakes.wait().await;
    assert!(
        misses.is_empty(),
        "missed the ADR-17 budget or the 30 MiB limit: {misses:#?}"
    );
    if std::env::var_os("ZWIIM_BENCH_CHECK").is_some() {
        let baseline: Measured = serde_json::from_str(include_str!("proxy_baseline.json")).unwrap();
        let worse = regressions(&measured, &baseline);
        assert!(
            worse.is_empty(),
            "over proxy_baseline.json by more than 20 %: {worse:#?}"
        );
    }
}

#[test]
fn a_run_may_be_a_fifth_worse_than_the_baseline_and_no_more() {
    let run = |p50: &[(&str, u64)], memory_kib| Measured {
        profile: "debug".into(),
        p50_us: p50.iter().map(|&(k, v)| (k.to_string(), v)).collect(),
        memory_kib,
    };
    let baseline = run(
        &[("x8 session start", 10_000), ("x8 warm call", 500)],
        Some(30_000),
    );

    let within = run(
        &[("x8 session start", 12_000), ("x8 warm call", 1_500)],
        Some(36_000),
    );
    assert!(regressions(&within, &baseline).is_empty());

    let worse = run(
        &[("x8 session start", 12_001), ("x8 warm call", 1_501)],
        Some(36_001),
    );
    assert_eq!(
        regressions(&worse, &baseline),
        [
            "x8 session start: p50 12001 µs, baseline 10000 µs",
            "x8 warm call: p50 1501 µs, baseline 500 µs",
            "memory: 36001 KiB, baseline 30000 KiB",
        ]
    );

    let new_case = run(&[("x16 warm call", 500)], None);
    assert_eq!(
        regressions(&new_case, &baseline),
        ["x16 warm call: no baseline"]
    );

    let release = Measured {
        profile: "release".into(),
        ..run(&[("x8 warm call", 100)], Some(1_000))
    };
    assert_eq!(
        regressions(&release, &baseline),
        ["a release build, against a debug baseline"]
    );
}
