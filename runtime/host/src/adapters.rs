//! Managed agent adapters: the exact Claude and Codex adapters this Host
//! runs, installed from the committed lock into `<data dir>/adapters`.
//!
//! - **From the lock only.** Every package is checked against its locked
//!   sha512 and npm registry signature, and Node against its locked SHA-256,
//!   before anything is extracted; nothing runs until all of it matched.
//!   No install scripts run, and nothing is installed outside the data
//!   directory (no `npm -g`, no system paths).
//! - **Never from PATH.** An adapter launches from its managed install and
//!   nowhere else. `ZWIIM_ADAPTER_SOURCE=path` is an explicit development
//!   switch that allows a PATH copy when no managed one is installed.
//! - **Known-good stays.** A new version is activated only after it
//!   answered the ACP `initialize` probe; the version it replaces is kept to
//!   roll back to. Offline, the installed version keeps working; a missing
//!   one reports that setup needs a network connection; retrying is safe.
//! - **Four separate facts.** Installed (a complete tree is active),
//!   healthy (it answered the probe this run), authenticated (vendor
//!   credentials were found; the agent itself is the final judge) and usable
//!   (all three).

pub mod install;
pub mod lock;
pub mod probe;
pub mod verify;

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::acp::error::AcpError;
use crate::models::agent::AgentType;
use install::{Fetch, FetchError, Installer, Outcome, SetupError};
use lock::{Lock, Platform};

/// Set to `path` to let a development Host fall back to an adapter on PATH.
pub const DEV_SOURCE_ENV: &str = "ZWIIM_ADAPTER_SOURCE";
/// Set to `manual` to skip setting adapters up when the Host starts.
pub const SETUP_ENV: &str = "ZWIIM_ADAPTER_SETUP";
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAdapter {
    Claude,
    Codex,
}

impl ManagedAdapter {
    pub const ALL: [ManagedAdapter; 2] = [ManagedAdapter::Claude, ManagedAdapter::Codex];

    /// Its key in the lock and its directory under the adapters root.
    pub fn id(self) -> &'static str {
        match self {
            ManagedAdapter::Claude => "claude",
            ManagedAdapter::Codex => "codex",
        }
    }

    pub fn for_agent(agent: AgentType) -> Option<ManagedAdapter> {
        match agent {
            AgentType::ClaudeCode => Some(ManagedAdapter::Claude),
            AgentType::Codex => Some(ManagedAdapter::Codex),
            _ => None,
        }
    }

    pub fn agent(self) -> AgentType {
        match self {
            ManagedAdapter::Claude => AgentType::ClaudeCode,
            ManagedAdapter::Codex => AgentType::Codex,
        }
    }

    /// The adapter's command name, for the development PATH fallback.
    fn command(self) -> &'static str {
        match self {
            ManagedAdapter::Claude => "claude-agent-acp",
            ManagedAdapter::Codex => "codex-acp",
        }
    }

    /// The version this Host is built for.
    pub fn locked_version(self) -> Option<&'static str> {
        Lock::embedded()
            .adapters
            .get(self.id())
            .map(|a| a.version.as_str())
    }
}

/// Where managed adapters live: beside the Host's database.
pub fn root() -> PathBuf {
    let data = std::env::var_os("ZWIIM_DATA_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|d| d.join("app.zwiim")))
        .unwrap_or_else(|| PathBuf::from("."));
    data.join("adapters")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Managed,
    /// Found on PATH under the development switch.
    DevPath,
}

/// How to start an adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub source: Source,
    pub version: Option<String>,
}

impl Launch {
    pub fn argv(&self) -> Vec<String> {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .collect()
    }
}

/// The managed install, else (only when `dev_path` is given) the adapter on
/// that PATH.
pub fn resolve(
    root: &Path,
    adapter: ManagedAdapter,
    dev_path: Option<&std::ffi::OsStr>,
) -> Option<Launch> {
    if let Some(installed) = install::active(root, adapter.id()) {
        return Some(Launch {
            program: installed.node,
            args: vec![installed.entry.to_string_lossy().into_owned()],
            source: Source::Managed,
            version: Some(installed.version),
        });
    }
    let path = dev_path?;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let program = which::which_in(adapter.command(), Some(path), cwd).ok()?;
    Some(Launch {
        program,
        args: Vec::new(),
        source: Source::DevPath,
        version: None,
    })
}

/// PATH, when the development switch allows it.
fn dev_path() -> Option<OsString> {
    let enabled = std::env::var(DEV_SOURCE_ENV).is_ok_and(|v| v.trim() == "path");
    enabled.then(|| std::env::var_os("PATH")).flatten()
}

/// How to launch `agent`, for the agents this module manages (`None` for
/// the rest).
pub fn launch(agent: AgentType) -> Option<Result<Launch, AcpError>> {
    let adapter = ManagedAdapter::for_agent(agent)?;
    Some(
        resolve(&root(), adapter, dev_path().as_deref()).ok_or_else(|| {
            let name = crate::acp::registry::get_agent_meta(agent).name;
            // Keeps the "is not installed" wording the install gate uses.
            AcpError::SdkNotInstalled(format!(
                "{name} is not installed on this host yet. {}",
                match record(adapter).setup {
                    Setup::NeedsNetwork { .. } => "Setting it up needs a network connection.",
                    Setup::Installing => "It is being set up.",
                    _ => "Set it up from the host's agent settings.",
                }
            ))
        }),
    )
}

// ─── runtime state ──────────────────────────────────────────────────────

/// Where setting up an adapter stands in this Host process.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum Setup {
    #[default]
    Idle,
    Installing,
    /// The last attempt could not reach the network.
    NeedsNetwork {
        message: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, Default)]
struct Record {
    setup: Setup,
    /// The active version's probe result this run, by version.
    health: Option<(String, bool)>,
}

static RECORDS: LazyLock<Mutex<HashMap<ManagedAdapter, Record>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static SETUP_LOCKS: LazyLock<HashMap<ManagedAdapter, tokio::sync::Mutex<()>>> =
    LazyLock::new(|| {
        ManagedAdapter::ALL
            .into_iter()
            .map(|a| (a, tokio::sync::Mutex::new(())))
            .collect()
    });

fn record(adapter: ManagedAdapter) -> Record {
    RECORDS
        .lock()
        .unwrap()
        .get(&adapter)
        .cloned()
        .unwrap_or_default()
}

fn update(adapter: ManagedAdapter, f: impl FnOnce(&mut Record)) {
    f(RECORDS.lock().unwrap().entry(adapter).or_default());
}

/// Everything a client is told about a managed adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdapterStatus {
    /// The version that launches, if any.
    pub version: Option<String>,
    /// The version this Host is built for.
    pub locked_version: Option<&'static str>,
    pub installed: bool,
    /// Whether it answered the health probe; `null` until probed this run.
    pub healthy: Option<bool>,
    pub authenticated: bool,
    pub usable: bool,
    pub setup: Setup,
    pub source: Option<Source>,
}

/// The adapter's state now. `env` is the agent's configured environment,
/// which may carry its credentials.
pub fn status(adapter: ManagedAdapter, env: &BTreeMap<String, String>) -> AdapterStatus {
    let launch = resolve(&root(), adapter, dev_path().as_deref());
    let record = record(adapter);
    let healthy = match (&launch, &record.health) {
        (Some(l), Some((version, ok))) if l.version.as_ref() == Some(version) => Some(*ok),
        _ => None,
    };
    let authenticated = authenticated(adapter, env);
    let installed = launch.is_some();
    AdapterStatus {
        version: launch.as_ref().and_then(|l| l.version.clone()),
        locked_version: adapter.locked_version(),
        installed,
        healthy,
        authenticated,
        usable: installed && healthy == Some(true) && authenticated,
        setup: record.setup,
        source: launch.map(|l| l.source),
    }
}

/// Install (or confirm) the locked version and probe it. One run per adapter
/// at a time; a second caller waits and then finds the work done.
pub async fn setup(adapter: ManagedAdapter) -> Result<(), SetupError> {
    setup_with(adapter, &root(), Lock::embedded(), &HttpFetch::new()).await
}

async fn setup_with(
    adapter: ManagedAdapter,
    root: &Path,
    lock: &Lock,
    fetch: &dyn Fetch,
) -> Result<(), SetupError> {
    let _running = SETUP_LOCKS[&adapter].lock().await;
    let Some(platform) = Platform::current() else {
        return Err(SetupError::Unsupported);
    };
    let needs_install = match (
        install::active(root, adapter.id()),
        lock.digest(adapter.id(), platform),
    ) {
        (Some(current), Some(digest)) => current.digest != digest,
        _ => true,
    };
    if needs_install {
        update(adapter, |r| r.setup = Setup::Installing);
    }
    let installer = Installer {
        root,
        lock,
        platform,
        fetch,
        probe_timeout: PROBE_TIMEOUT,
    };
    let result = installer.ensure(adapter.id()).await;
    match &result {
        Ok(Outcome::Current(_)) => {
            tracing::info!("[adapters] {} is current and healthy", adapter.id())
        }
        Ok(Outcome::Installed {
            version, replaced, ..
        }) => tracing::info!(
            "[adapters] {} {version} installed (replaced {})",
            adapter.id(),
            replaced.as_deref().unwrap_or("nothing")
        ),
        Err(e) => tracing::warn!("[adapters] setting up {} failed: {e}", adapter.id()),
    }
    // Health of whatever launches now: the new version, or the one kept.
    let health = match (&result, install::active(root, adapter.id())) {
        (_, None) => None,
        (Ok(_), Some(active)) => Some((active.version, true)),
        (Err(SetupError::Unhealthy(_)), Some(active)) if !needs_install => {
            Some((active.version, false))
        }
        (Err(_), Some(active)) => {
            let ok = probe::initialize(&active.node, &active.entry, PROBE_TIMEOUT)
                .await
                .is_ok();
            Some((active.version, ok))
        }
    };
    update(adapter, |r| {
        r.health = health;
        r.setup = match &result {
            Ok(_) => Setup::Idle,
            Err(SetupError::Offline(m)) => Setup::NeedsNetwork { message: m.clone() },
            Err(e) => Setup::Failed {
                message: e.to_string(),
            },
        };
    });
    result.map(|_| ())
}

/// Set up every managed adapter, one after another, in the background.
/// `ZWIIM_ADAPTER_SETUP=manual` leaves it to `agent.setup` (tests, and hosts
/// that must not download at start).
pub fn provision_at_start() {
    if dev_path().is_some() {
        tracing::warn!(
            "[adapters] {DEV_SOURCE_ENV}=path: adapters may launch from PATH (development only)"
        );
    }
    if std::env::var(SETUP_ENV).is_ok_and(|v| v.trim() == "manual") {
        tracing::info!("[adapters] {SETUP_ENV}=manual: not setting up adapters at start");
        return;
    }
    tokio::spawn(async {
        for adapter in ManagedAdapter::ALL {
            let _ = setup(adapter).await;
        }
    });
}

/// Start [`setup`] unless one is already running. Returns immediately.
pub fn setup_in_background(adapter: ManagedAdapter) {
    if SETUP_LOCKS[&adapter].try_lock().is_err() {
        return;
    }
    tokio::spawn(async move {
        let _ = setup(adapter).await;
    });
}

// ─── credentials ────────────────────────────────────────────────────────

/// Whether vendor credentials are present, without reading them: an
/// environment variable the agent honors, the vendor's credentials file, or
/// (Claude on macOS) its keychain item.
fn authenticated(adapter: ManagedAdapter, env: &BTreeMap<String, String>) -> bool {
    let set = |key: &str| {
        env.get(key).is_some_and(|v| !v.trim().is_empty())
            || std::env::var(key).is_ok_and(|v| !v.trim().is_empty())
    };
    match adapter {
        ManagedAdapter::Claude => {
            const KEYS: &[&str] = &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
            ];
            KEYS.iter().any(|k| set(k))
                || claude_config_dir().join(".credentials.json").is_file()
                || claude_keychain_item()
        }
        ManagedAdapter::Codex => {
            set("OPENAI_API_KEY")
                || set("CODEX_API_KEY")
                || crate::commands::acp::codex_home_dir()
                    .join("auth.json")
                    .is_file()
        }
    }
}

fn claude_config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"))
}

/// Claude Code keeps its login in the macOS keychain. Asks only whether the
/// item exists (never for its secret); cached briefly.
#[cfg(target_os = "macos")]
fn claude_keychain_item() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    if let Some((at, found)) = *CACHE.lock().unwrap() {
        if at.elapsed() < Duration::from_secs(30) {
            return found;
        }
    }
    let found = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", "Claude Code-credentials"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    *CACHE.lock().unwrap() = Some((Instant::now(), found));
    found
}

#[cfg(not(target_os = "macos"))]
fn claude_keychain_item() -> bool {
    false
}

// ─── downloads ──────────────────────────────────────────────────────────

pub struct HttpFetch {
    client: reqwest::Client,
}

impl HttpFetch {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .user_agent(concat!("zwiim-host/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("an HTTP client");
        HttpFetch { client }
    }
}

impl Default for HttpFetch {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Fetch for HttpFetch {
    async fn fetch(&self, url: &str, dest: &Path) -> Result<(), FetchError> {
        use futures::StreamExt;
        use tokio::io::AsyncWriteExt;
        let offline = |e: reqwest::Error| {
            if e.is_connect() || e.is_timeout() || e.is_request() {
                FetchError::Offline(e.to_string())
            } else {
                FetchError::Failed(e.to_string())
            }
        };
        let res = self.client.get(url).send().await.map_err(offline)?;
        if !res.status().is_success() {
            return Err(FetchError::Failed(format!("{url}: HTTP {}", res.status())));
        }
        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| FetchError::Failed(e.to_string()))?;
        let mut body = res.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(offline)?;
            file.write_all(&chunk)
                .await
                .map_err(|e| FetchError::Failed(e.to_string()))?;
        }
        file.sync_all()
            .await
            .map_err(|e| FetchError::Failed(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
