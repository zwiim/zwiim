//! Host-side connector benchmark against the ADR-17 budget (design r3 §8.1):
//! the adapter and egress client, without the agent proxy (Phase 2 adds it).
//!
//! ```text
//! cargo test --release -p zwiim-host --lib connectors::bench -- --ignored --nocapture
//! ```
//!
//! The fake servers run in a child process (this test binary again), so the
//! memory figure is this process alone. Each fake answers at once: there is
//! no network latency to subtract, and no timer whose rounding (about 2 ms
//! for a tokio sleep) would pass for overhead. The whole loopback round trip,
//! the fake's own work included, counts as Zwiim's, so the figures are an
//! upper bound on Host overhead.
//!
//! ```text
//! bench (this process)                         child (same binary)
//!   spawn child ─────────────────────────────▶ fake_servers: 8 fakes per era
//!   read "FAKE <era> <port>" lines ◀────────── print ports, wait for stdin EOF
//!   memory: 8 warm connections, before/after, first
//!   per era, per 1/3/8 connections:
//!     cold: connect + tools/list + one call, repeated
//!     warm: ≥ 1,000 calls over open connections
//!   drop stdin ──────────────────────────────▶ exit
//! ```

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use http::header::AUTHORIZATION;
use serde_json::{json, Map};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use url::Url;

use super::egress::SystemResolver;
use super::egress::{Credential, EgressGuard, EgressHttpClient, EndpointRule, NetworkClass};
use super::fake_mcp::{Era, FakeMcp};
use super::mcp::McpUpstream;
use super::upstream::{DispatchMark, ShutdownReason, Upstream, UpstreamCall};

mod proxy;

const CALLS: usize = 1_000;
const CONNECTIONS: [usize; 3] = [1, 3, 8];
const MAX: usize = 8;
const CHILD_ENV: &str = "ZWIIM_CONNECTOR_BENCH_CHILD";

const P95_TARGET: Duration = Duration::from_millis(100);
const P99_TARGET: Duration = Duration::from_millis(300);
/// 8 warm connectors, Host and proxies together: confirmed as the limit
/// after Phase 0 (2026-11-28). The Host-only run reports it; the proxy run
/// enforces it.
const MEMORY_TARGET: u64 = 30 * 1024 * 1024;

const ERAS: [(&str, Era); 2] = [
    ("modern", Era::Modern),
    ("legacy", Era::Legacy("2025-11-25")),
];

/// The child half: serves [`MAX`] fakes per era until stdin closes.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "run by the connector benchmark"]
async fn fake_servers() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let mut servers = Vec::new();
    for (name, era) in ERAS {
        for _ in 0..MAX {
            let server = FakeMcp::new(era).serve_http().await;
            println!("FAKE {name} {}", server.addr().port());
            servers.push(server);
        }
    }
    println!("FAKES READY");
    let mut rest = Vec::new();
    let _ = tokio::io::stdin().read_to_end(&mut rest).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark; run explicitly with --release"]
async fn connector_overhead() {
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
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
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut ports: Vec<(String, u16)> = Vec::new();
    while let Some(line) = lines.next_line().await.unwrap() {
        if line == "FAKES READY" {
            break;
        }
        if let Some(rest) = line.strip_prefix("FAKE ") {
            let (era, port) = rest.split_once(' ').unwrap();
            ports.push((era.to_string(), port.parse().unwrap()));
        }
    }
    let urls = |era: &str| -> Vec<Url> {
        ports
            .iter()
            .filter(|(name, _)| name == era)
            .map(|(_, port)| Url::parse(&format!("http://127.0.0.1:{port}/mcp")).unwrap())
            .collect()
    };

    // First, before the latency runs have grown the allocator's pools.
    let mut all = urls("modern");
    all.truncate(MAX / 2);
    all.extend(urls("legacy").into_iter().take(MAX / 2));
    match warm_memory(&all).await {
        Some(bytes) => println!(
            "\nmemory: 8 warm connections (4 modern, 4 legacy) add {} KiB to this process (initial target {} MiB{})",
            bytes / 1024,
            MEMORY_TARGET / (1024 * 1024),
            if bytes > MEMORY_TARGET { ", MISSED" } else { "" },
        ),
        None => println!("memory: not measured on this platform"),
    }

    println!(
        "\nconnector overhead, {} build",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "{:<7} {:>5} {:<13} {:>6} {:>9} {:>9} {:>9}",
        "era", "conns", "case", "n", "p50", "p95", "p99"
    );
    let mut misses = Vec::new();
    for (era, _) in ERAS {
        let urls = urls(era);
        assert_eq!(urls.len(), MAX, "{era}: fakes");
        for n in CONNECTIONS {
            let egress = egress_for(&urls[..n]);
            for (case, samples) in [
                ("session start", session_start(&urls[..n], &egress).await),
                ("cold call", cold_calls(&urls[..n], &egress).await),
                ("warm call", warm_calls(&urls[..n], &egress).await),
            ] {
                let row = Row::new(samples);
                println!(
                    "{era:<7} {n:>5} {case:<13} {:>6} {:>9.2?} {:>9.2?} {:>9.2?}",
                    row.n, row.p50, row.p95, row.p99
                );
                if row.p95 > P95_TARGET || row.p99 > P99_TARGET {
                    misses.push(format!(
                        "{era} x{n} {case}: p95 {:?}, p99 {:?}",
                        row.p95, row.p99
                    ));
                }
            }
        }
    }

    drop(child.stdin.take());
    let _ = child.wait().await;
    assert!(
        misses.is_empty(),
        "missed the ADR-17 latency budget: {misses:#?}"
    );
}

fn egress_for(urls: &[Url]) -> Arc<EgressHttpClient> {
    let rules = urls
        .iter()
        .map(|url| {
            let origin = url.origin().ascii_serialization();
            EndpointRule::new(&origin, true, NetworkClass::Local).unwrap()
        })
        .collect();
    let guard = EgressGuard::new(rules, Arc::new(SystemResolver));
    Arc::new(EgressHttpClient::new(Arc::new(guard)).unwrap())
}

async fn open(url: &Url, egress: &Arc<EgressHttpClient>) -> McpUpstream {
    let credential = Credential::new(AUTHORIZATION, "Bearer bench").unwrap();
    McpUpstream::connect_http(url, egress.clone(), Some(credential))
        .await
        .unwrap()
}

fn echo() -> UpstreamCall {
    let mut arguments = Map::new();
    arguments.insert("text".into(), json!("bench"));
    UpstreamCall {
        tool: "echo".into(),
        arguments,
    }
}

/// One timed call.
async fn timed_call(upstream: &McpUpstream) -> Duration {
    let started = Instant::now();
    upstream
        .call(echo(), &DispatchMark::default())
        .await
        .unwrap();
    started.elapsed()
}

/// Every connection opened and listed at once, as a session start does: the
/// session waits for the slowest.
async fn session_start(urls: &[Url], egress: &Arc<EgressHttpClient>) -> Vec<Duration> {
    let rounds = CALLS / 10;
    let mut samples = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let started = Instant::now();
        let upstreams = futures::future::join_all(urls.iter().map(|url| async move {
            let upstream = open(url, egress).await;
            upstream.list_tools().await.unwrap();
            upstream
        }))
        .await;
        samples.push(started.elapsed());
        close(upstreams).await;
    }
    samples
}

/// The first call on a fresh connection, [`CALLS`] in all.
async fn cold_calls(urls: &[Url], egress: &Arc<EgressHttpClient>) -> Vec<Duration> {
    let mut samples = Vec::with_capacity(CALLS);
    while samples.len() < CALLS {
        let upstreams = futures::future::join_all(urls.iter().map(|url| open(url, egress))).await;
        let round = futures::future::join_all(upstreams.iter().map(timed_call)).await;
        samples.extend(round);
        close(upstreams).await;
    }
    samples
}

/// [`CALLS`] calls spread over open connections, all connections busy at once.
async fn warm_calls(urls: &[Url], egress: &Arc<EgressHttpClient>) -> Vec<Duration> {
    let upstreams = futures::future::join_all(urls.iter().map(|url| open(url, egress))).await;
    for upstream in &upstreams {
        timed_call(upstream).await;
    }
    let per_connection = CALLS.div_ceil(urls.len());
    let runs = upstreams.iter().map(|upstream| async move {
        let mut samples = Vec::with_capacity(per_connection);
        for _ in 0..per_connection {
            samples.push(timed_call(upstream).await);
        }
        samples
    });
    let samples = futures::future::join_all(runs).await.concat();
    close(upstreams).await;
    samples
}

/// What 8 warm connections (connected, listed, called) add to this process.
/// Freed memory the allocator keeps counts too, so this reads high.
async fn warm_memory(urls: &[Url]) -> Option<u64> {
    let before = footprint()?;
    let egress = egress_for(urls);
    let upstreams = futures::future::join_all(urls.iter().map(|url| open(url, &egress))).await;
    for upstream in &upstreams {
        upstream.list_tools().await.unwrap();
        for _ in 0..10 {
            timed_call(upstream).await;
        }
    }
    let after = footprint()?;
    close(upstreams).await;
    Some(after.saturating_sub(before))
}

async fn close(upstreams: Vec<McpUpstream>) {
    for upstream in upstreams {
        upstream
            .shutdown(ShutdownReason::SessionEnded)
            .await
            .unwrap();
    }
}

struct Row {
    n: usize,
    p50: Duration,
    p95: Duration,
    p99: Duration,
}

impl Row {
    fn new(mut samples: Vec<Duration>) -> Self {
        samples.sort();
        let at = |p: f64| samples[((samples.len() as f64 * p).ceil() as usize).saturating_sub(1)];
        Self {
            n: samples.len(),
            p50: at(0.50),
            p95: at(0.95),
            p99: at(0.99),
        }
    }
}

/// This process's memory.
fn footprint() -> Option<u64> {
    footprint_of(std::process::id())
}

/// Physical footprint, which doesn't count shared pages twice.
#[cfg(target_os = "macos")]
fn footprint_of(pid: u32) -> Option<u64> {
    // SAFETY: `info` is a valid, writable rusage_info_v2 for the call.
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    let status = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            libc::RUSAGE_INFO_V2,
            (&mut info as *mut libc::rusage_info_v2).cast(),
        )
    };
    (status == 0).then_some(info.ri_phys_footprint)
}

/// Proportional set size.
#[cfg(target_os = "linux")]
fn footprint_of(pid: u32) -> Option<u64> {
    let rollup = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    let kib = rollup
        .lines()
        .find_map(|line| line.strip_prefix("Pss:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(kib * 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn footprint_of(_pid: u32) -> Option<u64> {
    None
}
