//! Signing the Host's agents in to their providers: the status clients see,
//! account sign-in through the vendor's own CLI, API keys kept in the Mac's
//! Keychain, and sign-out.
//!
//! The Host is the sole owner of provider secrets. A key arrives once, from
//! the local Desktop, and leaves only for the agent's own environment at
//! launch. Clients get status alone: connected or not, how, and the account's
//! email and plan — never a key, a vendor file path or an adapter detail.
//!
//! An account sign-in always wins over a saved key: the key is handed to an
//! agent only while the agent has no account sign-in of its own.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use zeroize::Zeroizing;

use crate::adapters::{self, ManagedAdapter};
use crate::models::agent::AgentType;

const KEYCHAIN_SERVICE: &str = "zwiim";
/// The key file of the `file` store (`ZWIIM_HOST_KEY_STORE=file`).
const KEY_FILE: &str = "agent-keys.json";
/// A browser sign-in the user walked away from ends here.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SIGN_OUT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_KEY_LEN: usize = 512;
/// The Codex launcher that keeps an API key sign-in in memory (see
/// [`keep_codex_key_in_memory`]); a file beside the Codex installs.
const CODEX_IN_MEMORY_LAUNCHER: &str = "codex-in-memory.sh";

/// The variable each agent reads its API key from.
fn key_var(adapter: ManagedAdapter) -> &'static str {
    match adapter {
        ManagedAdapter::Claude => "ANTHROPIC_API_KEY",
        ManagedAdapter::Codex => "CODEX_API_KEY",
    }
}

/// What a valid key starts with.
fn key_prefix(adapter: ManagedAdapter) -> &'static str {
    match adapter {
        ManagedAdapter::Claude => "sk-ant-",
        ManagedAdapter::Codex => "sk-",
    }
}

// ─── status ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// The vendor account (Claude subscription, ChatGPT).
    Account,
    ApiKey,
    /// A cloud provider (Bedrock, Vertex, Foundry) set up in the agent's env.
    Cloud,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignIn {
    #[default]
    Idle,
    /// The vendor's sign-in page is open in the browser.
    Running,
    /// The last sign-in did not finish (cancelled in the browser, timed out).
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentAuthStatus {
    pub agent: AgentType,
    pub connected: bool,
    pub method: Option<Method>,
    /// Whether an API key is saved on this Host for the agent. Unused while
    /// the agent is signed in to an account.
    pub saved_key: bool,
    pub sign_in: SignIn,
    pub email: Option<String>,
    pub plan: Option<String>,
}

/// `agent`'s sign-in on this Host; `env` is the agent's configured launch
/// environment (with any saved key applied). `None` for agents the Host does
/// not manage.
pub fn status(agent: AgentType, env: &BTreeMap<String, String>) -> Option<AgentAuthStatus> {
    let adapter = ManagedAdapter::for_agent(agent)?;
    let method = match adapter {
        ManagedAdapter::Claude => claude_method(
            env,
            &adapters::inherited_env,
            &adapters::claude_config_dir(),
            &adapters::claude_keychain_item,
        ),
        ManagedAdapter::Codex => codex_method(
            env,
            &adapters::inherited_env,
            &crate::commands::acp::codex_home_dir(),
        ),
    };
    let (email, plan) = match method {
        Some(Method::Account) => account_details(adapter),
        _ => (None, None),
    };
    Some(AgentAuthStatus {
        agent,
        connected: method.is_some(),
        method,
        saved_key: saved_key(adapter).is_some(),
        sign_in: sign_in_state(adapter),
        email,
        plan,
    })
}

/// Claude Code's own precedence: a cloud provider, then a key or token in its
/// environment, then the claude.ai login.
fn claude_method(
    env: &BTreeMap<String, String>,
    inherited: &dyn Fn(&str) -> Option<String>,
    config_dir: &Path,
    keychain_item: &dyn Fn() -> bool,
) -> Option<Method> {
    let set = |k: &str| adapters::credential_set(env, inherited, k);
    if [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ]
    .into_iter()
    .any(set)
    {
        Some(Method::Cloud)
    } else if ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        .into_iter()
        .any(set)
    {
        Some(Method::ApiKey)
    } else if set("CLAUDE_CODE_OAUTH_TOKEN") || claude_account(config_dir, keychain_item) {
        Some(Method::Account)
    } else {
        None
    }
}

fn claude_account(config_dir: &Path, keychain_item: &dyn Fn() -> bool) -> bool {
    config_dir.join(".credentials.json").is_file() || keychain_item()
}

/// A ChatGPT login wins over a key: Zwiim signs Codex in with a key only when
/// it reports no other sign-in.
fn codex_method(
    env: &BTreeMap<String, String>,
    inherited: &dyn Fn(&str) -> Option<String>,
    codex_home: &Path,
) -> Option<Method> {
    match codex_login(codex_home) {
        CodexLogin::ChatGpt => Some(Method::Account),
        _ if adapters::codex_api_key_present(env, inherited) => Some(Method::ApiKey),
        CodexLogin::ApiKey => Some(Method::ApiKey),
        CodexLogin::None => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexLogin {
    None,
    ChatGpt,
    /// A key Codex saved in `auth.json` itself.
    ApiKey,
}

/// What Codex's own `auth.json` holds. Reads the mode only.
fn codex_login(codex_home: &Path) -> CodexLogin {
    let Some(auth) = read_json(&codex_home.join("auth.json")) else {
        return CodexLogin::None;
    };
    match auth.get("auth_mode").and_then(|m| m.as_str()) {
        Some("apikey") => CodexLogin::ApiKey,
        Some(_) => CodexLogin::ChatGpt,
        // Older files carry no mode: tokens mean a ChatGPT login.
        None if auth.get("tokens").is_some_and(|t| !t.is_null()) => CodexLogin::ChatGpt,
        None => CodexLogin::ApiKey,
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// The signed-in account's email and plan: from the vendor's own account
/// record where it keeps one, else from what the agent last reported here.
fn account_details(adapter: ManagedAdapter) -> (Option<String>, Option<String>) {
    let reported = REPORTED
        .lock()
        .unwrap()
        .get(&adapter)
        .filter(|r| r.kind == "account")
        .cloned()
        .unwrap_or_default();
    let (email, plan) = match adapter {
        ManagedAdapter::Claude => (claude_account_email(&adapters::claude_config_dir()), None),
        ManagedAdapter::Codex => codex_account(&crate::commands::acp::codex_home_dir()),
    };
    (email.or(reported.email), plan.or(reported.label))
}

/// Claude Code's account record (`.claude.json`) names the signed-in email.
fn claude_account_email(config_dir: &Path) -> Option<String> {
    let path = match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|s| !s.is_empty()) {
        Some(_) => config_dir.join(".claude.json"),
        None => dirs::home_dir()?.join(".claude.json"),
    };
    non_empty(
        read_json(&path)?
            .get("oauthAccount")?
            .get("emailAddress")?
            .as_str(),
    )
}

/// The email and plan claims of a ChatGPT login's ID token. The token is
/// decoded, not verified: it is Codex's own file, read for display only.
fn codex_account(codex_home: &Path) -> (Option<String>, Option<String>) {
    use base64::Engine;
    let claims = read_json(&codex_home.join("auth.json"))
        .and_then(|auth| {
            let token = auth.get("tokens")?.get("id_token")?.as_str()?.to_string();
            let payload = token.split('.').nth(1)?.to_string();
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(payload.trim_end_matches('='))
                .ok()
        })
        .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok());
    let Some(claims) = claims else {
        return (None, None);
    };
    let email = non_empty(claims.get("email").and_then(|e| e.as_str()));
    let plan = claims
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(|p| p.as_str())
        .and_then(|p| non_empty(Some(p)))
        .map(|p| format!("ChatGPT {}", title_case(&p)));
    (email, plan)
}

fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// What a running agent last said about its sign-in.
#[derive(Debug, Clone, Default)]
struct Reported {
    kind: String,
    label: Option<String>,
    email: Option<String>,
}

static REPORTED: LazyLock<Mutex<HashMap<ManagedAdapter, Reported>>> =
    LazyLock::new(Default::default);

/// Keep the sign-in `agent` reported (its `authStatus`): the plan's name and
/// the account's email show in the status.
pub fn record_reported(agent: AgentType, auth_status: &serde_json::Value) {
    let Some(adapter) = ManagedAdapter::for_agent(agent) else {
        return;
    };
    let text = |v: Option<&serde_json::Value>| non_empty(v.and_then(|v| v.as_str()));
    let reported = Reported {
        kind: text(auth_status.get("kind")).unwrap_or_default(),
        label: text(auth_status.get("label")),
        email: text(auth_status.get("account").and_then(|a| a.get("email"))),
    };
    REPORTED.lock().unwrap().insert(adapter, reported);
}

/// After any change to a sign-in: nothing cached may describe the old one.
fn forget_cached(adapter: ManagedAdapter) {
    REPORTED.lock().unwrap().remove(&adapter);
    if adapter == ManagedAdapter::Claude {
        adapters::forget_claude_keychain_item();
    }
}

// ─── saved keys ─────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("{0}")]
    InvalidKey(&'static str),
    #[error("the agent is not set up on this host")]
    NotInstalled,
    #[error("secure store: {0}")]
    Store(String),
    #[error("sign-out did not finish: {0}")]
    SignOut(String),
}

/// Where saved keys are kept.
#[derive(Debug, Clone)]
enum Store {
    /// The macOS Keychain, one item per agent.
    Keychain,
    /// `<data_dir>/agent-keys.json`, mode `0600` (`ZWIIM_HOST_KEY_STORE=file`:
    /// tests, and hosts without a keychain).
    File(PathBuf),
    /// This process only: a Host that never called [`init`] (library tests).
    Memory,
}

static STORE: OnceLock<Store> = OnceLock::new();

type Keys = HashMap<ManagedAdapter, Zeroizing<String>>;

/// The saved keys, held in memory so a launch never waits on the Keychain.
#[cfg(not(test))]
fn with_keys<T>(f: impl FnOnce(&mut Keys) -> T) -> T {
    static KEYS: LazyLock<Mutex<Keys>> = LazyLock::new(Default::default);
    f(&mut KEYS.lock().unwrap())
}

/// Per test thread, so a test's saved key never reaches another test's agent.
#[cfg(test)]
fn with_keys<T>(f: impl FnOnce(&mut Keys) -> T) -> T {
    thread_local! {
        static KEYS: std::cell::RefCell<Keys> = Default::default();
    }
    KEYS.with(|keys| f(&mut keys.borrow_mut()))
}

fn store() -> Store {
    STORE.get().cloned().unwrap_or(Store::Memory)
}

/// Choose the store `ZWIIM_HOST_KEY_STORE` selects (like the Host's own keys)
/// and load the saved keys, so a launch never waits on the Keychain.
pub async fn init(data_dir: &Path) {
    let selected = match std::env::var(crate::identity::host_keys::KEY_STORE_ENV)
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("file") => Store::File(data_dir.join(KEY_FILE)),
        _ => Store::Keychain,
    };
    let store = STORE.get_or_init(|| selected).clone();
    let loaded = tokio::task::spawn_blocking(move || {
        ManagedAdapter::ALL
            .into_iter()
            .map(|a| (a, load_key(&store, a)))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    for (adapter, key) in loaded {
        match key {
            Ok(Some(key)) => {
                with_keys(|keys| keys.insert(adapter, key));
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("[agent-auth] saved {} key unavailable: {e}", adapter.id()),
        }
    }
}

fn saved_key(adapter: ManagedAdapter) -> Option<Zeroizing<String>> {
    with_keys(|keys| keys.get(&adapter).cloned())
}

fn keychain_account(adapter: ManagedAdapter) -> String {
    format!("agent-api-key:{}", adapter.id())
}

fn keychain_entry(adapter: ManagedAdapter) -> Result<keyring::Entry, AuthError> {
    keyring::Entry::new(KEYCHAIN_SERVICE, &keychain_account(adapter))
        .map_err(|e| AuthError::Store(e.to_string()))
}

fn load_key(
    store: &Store,
    adapter: ManagedAdapter,
) -> Result<Option<Zeroizing<String>>, AuthError> {
    let key = match store {
        Store::Keychain => match keychain_entry(adapter)?.get_password() {
            Ok(key) => Some(Zeroizing::new(key)),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => return Err(AuthError::Store(e.to_string())),
        },
        Store::File(path) => read_key_file(path)?
            .remove(adapter.id())
            .map(Zeroizing::new),
        Store::Memory => None,
    };
    Ok(key.filter(|k| !k.trim().is_empty()))
}

/// Keep (`Some`) or delete (`None`) `adapter`'s key in `store`.
fn write_key(store: &Store, adapter: ManagedAdapter, key: Option<&str>) -> Result<(), AuthError> {
    match store {
        Store::Keychain => {
            let entry = keychain_entry(adapter)?;
            match key {
                Some(key) => entry.set_password(key),
                None => match entry.delete_credential() {
                    Err(keyring::Error::NoEntry) => Ok(()),
                    other => other,
                },
            }
            .map_err(|e| AuthError::Store(e.to_string()))
        }
        Store::File(path) => {
            // One read-modify-write at a time, so two saves never drop a key.
            static FILE_LOCK: Mutex<()> = Mutex::new(());
            let _held = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut keys = read_key_file(path)?;
            match key {
                Some(key) => keys.insert(adapter.id().to_string(), key.to_string()),
                None => keys.remove(adapter.id()),
            };
            let raw = Zeroizing::new(serde_json::to_vec(&keys).unwrap_or_default());
            keys.values_mut().for_each(zeroize::Zeroize::zeroize);
            // Write-then-rename so a crash never leaves a truncated file.
            let tmp = unique_tmp(path);
            crate::identity::device::write_private(&tmp, &raw)
                .and_then(|()| std::fs::rename(&tmp, path))
                .map_err(|e| AuthError::Store(e.to_string()))
        }
        Store::Memory => Ok(()),
    }
}

fn read_key_file(path: &Path) -> Result<BTreeMap<String, String>, AuthError> {
    match std::fs::read(path) {
        Ok(raw) => {
            let raw = Zeroizing::new(raw);
            serde_json::from_slice(&raw)
                .map_err(|_| AuthError::Store("the saved key file is unreadable".into()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(AuthError::Store(e.to_string())),
    }
}

async fn persist(adapter: ManagedAdapter, key: Option<Zeroizing<String>>) -> Result<(), AuthError> {
    let store = store();
    let to_store = key.clone();
    tokio::task::spawn_blocking(move || {
        write_key(&store, adapter, to_store.as_deref().map(|k| k.as_str()))
    })
    .await
    .map_err(|e| AuthError::Store(e.to_string()))??;
    with_keys(|keys| match key {
        Some(key) => keys.insert(adapter, key),
        None => keys.remove(&adapter),
    });
    Ok(())
}

/// A key as typed, checked: no surrounding space, one line, the provider's
/// shape.
fn checked_key(adapter: ManagedAdapter, key: &str) -> Result<Zeroizing<String>, AuthError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AuthError::InvalidKey("Enter an API key."));
    }
    if key.len() > MAX_KEY_LEN || key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AuthError::InvalidKey("That is not an API key."));
    }
    if !key.starts_with(key_prefix(adapter)) {
        return Err(AuthError::InvalidKey(match adapter {
            ManagedAdapter::Claude => "Anthropic API keys start with sk-ant-.",
            ManagedAdapter::Codex => "OpenAI API keys start with sk-.",
        }));
    }
    Ok(Zeroizing::new(key.to_string()))
}

/// Save `key` for `agent`, replacing any saved before. A key Codex once wrote
/// to its own `auth.json` goes: the Host keeps the key from now on.
pub async fn save_key(adapter: ManagedAdapter, key: &str) -> Result<(), AuthError> {
    let key = checked_key(adapter, key)?;
    persist(adapter, Some(key)).await?;
    if adapter == ManagedAdapter::Codex {
        remove_codex_saved_key_file(&crate::commands::acp::codex_home_dir());
    }
    forget_cached(adapter);
    tracing::info!("[agent-auth] {} API key saved", adapter.id());
    Ok(())
}

pub async fn remove_key(adapter: ManagedAdapter) -> Result<(), AuthError> {
    persist(adapter, None).await?;
    forget_cached(adapter);
    tracing::info!("[agent-auth] {} API key removed", adapter.id());
    Ok(())
}

/// Codex's `auth.json` when it holds a plaintext key rather than a ChatGPT
/// login.
fn remove_codex_saved_key_file(codex_home: &Path) {
    if codex_login(codex_home) == CodexLogin::ApiKey {
        let _ = std::fs::remove_file(codex_home.join("auth.json"));
    }
}

/// Give `agent` its saved key at launch — only when it has no account sign-in
/// and no key of its own in `env` or the Host's environment.
pub fn apply_saved_key(agent: AgentType, env: &mut BTreeMap<String, String>) {
    let Some(adapter) = ManagedAdapter::for_agent(agent) else {
        return;
    };
    let Some(key) = saved_key(adapter) else {
        return;
    };
    let signed_in = match adapter {
        ManagedAdapter::Claude => adapters::claude_signed_in(
            env,
            &adapters::inherited_env,
            &adapters::claude_config_dir(),
            &adapters::claude_keychain_item,
        ),
        ManagedAdapter::Codex => {
            adapters::codex_api_key_present(env, &adapters::inherited_env)
                || codex_login(&crate::commands::acp::codex_home_dir()) == CodexLogin::ChatGpt
        }
    };
    hand_over(adapter, &key, signed_in, env);
}

fn hand_over(
    adapter: ManagedAdapter,
    key: &str,
    signed_in: bool,
    env: &mut BTreeMap<String, String>,
) {
    if signed_in {
        return;
    }
    env.insert(key_var(adapter).to_string(), key.to_string());
    if adapter == ManagedAdapter::Claude {
        // Claude Code keeps Anthropic credentials out of what it runs (its
        // shell, hooks, MCP servers), so a command never prints the key.
        env.insert("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB".into(), "1".into());
    }
}

// ─── Codex: a key sign-in stays in memory ───────────────────────────────

/// Codex saves a key it is signed in with to `auth.json` in plaintext. When
/// the Host signs it in with a key, point codex-acp (`CODEX_PATH`) at a
/// launcher that runs this install's Codex with in-memory credentials and
/// without the key in its environment: codex-acp hands the key over the
/// protocol, and anything Codex runs (its shells, their snapshots in
/// `shell_snapshots/`, the agent's commands) would otherwise see it. A
/// ChatGPT login is left alone. When the launcher cannot be used — a Codex of
/// the user's own (`CODEX_PATH`), a development adapter, a failed write — the
/// Host's saved key is withheld rather than written to disk; a key of the
/// user's own is left as it was.
pub fn keep_codex_key_in_memory(env: &mut BTreeMap<String, String>) {
    let inherited = adapters::inherited_env;
    if !adapters::codex_api_key_present(env, &inherited)
        || codex_login(&crate::commands::acp::codex_home_dir()) == CodexLogin::ChatGpt
    {
        return;
    }
    let launcher = if adapters::credential_set(env, &inherited, "CODEX_PATH") {
        Err("Codex is set by CODEX_PATH".to_string())
    } else {
        adapters::install::active(&adapters::root(), ManagedAdapter::Codex.id())
            .ok_or_else(|| "no installed Codex".to_string())
            .and_then(|installed| {
                write_codex_launcher(&adapters::root(), &installed.node, &codex_cli(&installed))
                    .map_err(|e| e.to_string())
            })
    };
    match launcher {
        Ok(launcher) => {
            env.insert("CODEX_PATH".into(), launcher.to_string_lossy().into_owned());
        }
        Err(why) => withhold_saved_codex_key(env, &why),
    }
}

/// Take the Host's saved key back out of `env`: without the launcher Codex
/// would keep it in its `auth.json`.
fn withhold_saved_codex_key(env: &mut BTreeMap<String, String>, why: &str) {
    let var = key_var(ManagedAdapter::Codex);
    let Some(saved) = saved_key(ManagedAdapter::Codex) else {
        return;
    };
    if env.get(var).map(String::as_str) == Some(saved.as_str()) {
        if let Some(mut handed) = env.remove(var) {
            zeroize::Zeroize::zeroize(&mut handed);
        }
        tracing::warn!("[agent-auth] Codex not given the saved API key ({why})");
    }
}

/// The Codex CLI bundled with a codex-acp install.
fn codex_cli(installed: &adapters::install::Installed) -> PathBuf {
    adapters::root()
        .join(ManagedAdapter::Codex.id())
        .join(&installed.version)
        .join("node_modules/@openai/codex/bin/codex.js")
}

fn write_codex_launcher(root: &Path, node: &Path, codex: &Path) -> std::io::Result<PathBuf> {
    let path = root
        .join(ManagedAdapter::Codex.id())
        .join(CODEX_IN_MEMORY_LAUNCHER);
    let script = codex_launcher_script(node, codex);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(script.as_str()) {
        std::fs::create_dir_all(path.parent().unwrap_or(root))?;
        let tmp = unique_tmp(&path);
        std::fs::write(&tmp, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::rename(&tmp, &path)?;
    }
    Ok(path)
}

/// A temporary file beside `path` no other write uses at the same time.
fn unique_tmp(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

fn codex_launcher_script(node: &Path, codex: &Path) -> String {
    format!(
        "#!/bin/sh\n# Written by Zwiim: Codex with in-memory credentials.\nunset CODEX_API_KEY OPENAI_API_KEY\nexec {} {} -c 'cli_auth_credentials_store=\"ephemeral\"' \"$@\"\n",
        shell_quote(&node.to_string_lossy()),
        shell_quote(&codex.to_string_lossy()),
    )
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

// ─── account sign-in and sign-out ───────────────────────────────────────

#[derive(Default)]
struct SignInSlot {
    state: SignIn,
    /// Bumped by every start, so a finished older sign-in cannot report over
    /// a newer one.
    generation: u64,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
}

static SIGN_INS: LazyLock<Mutex<HashMap<ManagedAdapter, SignInSlot>>> =
    LazyLock::new(Default::default);

fn sign_in_state(adapter: ManagedAdapter) -> SignIn {
    SIGN_INS
        .lock()
        .unwrap()
        .get(&adapter)
        .map(|s| s.state)
        .unwrap_or_default()
}

/// The vendor CLI of this Host's install, run with `args`.
fn vendor_command(
    adapter: ManagedAdapter,
    args: &[&str],
) -> Result<tokio::process::Command, AuthError> {
    let installed = adapters::install::active(&adapters::root(), adapter.id())
        .ok_or(AuthError::NotInstalled)?;
    let mut cmd = crate::process::tokio_command(&installed.node);
    match adapter {
        // The Claude adapter runs Claude Code's own CLI under `--cli`.
        ManagedAdapter::Claude => {
            cmd.arg(&installed.entry).arg("--cli");
            // A key in the environment would sign the CLI in with it.
            cmd.env_remove("ANTHROPIC_API_KEY")
                .env_remove("ANTHROPIC_AUTH_TOKEN");
        }
        ManagedAdapter::Codex => {
            cmd.arg(codex_cli(&installed));
        }
    }
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    Ok(cmd)
}

fn sign_in_args(adapter: ManagedAdapter) -> &'static [&'static str] {
    match adapter {
        ManagedAdapter::Claude => &["auth", "login", "--claudeai"],
        ManagedAdapter::Codex => &["login"],
    }
}

fn sign_out_args(adapter: ManagedAdapter) -> &'static [&'static str] {
    match adapter {
        ManagedAdapter::Claude => &["auth", "logout"],
        ManagedAdapter::Codex => &["logout"],
    }
}

/// Open the vendor's sign-in page in the browser of the Mac running the Host
/// and wait for it in the background. Starting again restarts it.
pub fn start_sign_in(adapter: ManagedAdapter) -> Result<(), AuthError> {
    let mut cmd = vendor_command(adapter, sign_in_args(adapter))?;
    let child = cmd.spawn().map_err(|e| AuthError::Store(e.to_string()))?;
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    let generation = {
        let mut slots = SIGN_INS.lock().unwrap();
        let slot = slots.entry(adapter).or_default();
        // Dropping the sender ends the earlier sign-in.
        slot.cancel = Some(cancel_tx);
        slot.generation += 1;
        slot.state = SignIn::Running;
        slot.generation
    };
    tracing::info!("[agent-auth] {} account sign-in started", adapter.id());
    tokio::spawn(async move {
        let finished = wait_or_end(child, cancel_rx, SIGN_IN_TIMEOUT).await;
        let mut slots = SIGN_INS.lock().unwrap();
        let slot = slots.entry(adapter).or_default();
        if slot.generation != generation {
            return;
        }
        slot.cancel = None;
        slot.state = match finished {
            Some(true) => SignIn::Idle,
            Some(false) => SignIn::Failed,
            None => SignIn::Idle,
        };
        drop(slots);
        forget_cached(adapter);
        match finished {
            Some(true) => tracing::info!("[agent-auth] {} account sign-in finished", adapter.id()),
            Some(false) => tracing::info!(
                "[agent-auth] {} account sign-in did not finish",
                adapter.id()
            ),
            None => tracing::info!("[agent-auth] {} account sign-in stopped", adapter.id()),
        }
    });
    Ok(())
}

/// End a sign-in in progress, if any.
fn stop_sign_in(adapter: ManagedAdapter) {
    if let Some(slot) = SIGN_INS.lock().unwrap().get_mut(&adapter) {
        slot.generation += 1;
        slot.cancel = None;
        slot.state = SignIn::Idle;
    }
}

/// `Some(success)` once `child` exits (a timeout is a failure); `None` when
/// cancelled. The child's whole tree is ended either way.
async fn wait_or_end(
    mut child: tokio::process::Child,
    cancel: tokio::sync::oneshot::Receiver<()>,
    timeout: Duration,
) -> Option<bool> {
    let outcome = tokio::select! {
        status = child.wait() => return Some(status.is_ok_and(|s| s.success())),
        _ = tokio::time::sleep(timeout) => Some(false),
        _ = cancel => None,
    };
    if let Some(pid) = child.id() {
        // The vendor CLIs run their own native binary as a child.
        let config = kill_tree::Config {
            signal: "SIGKILL".to_string(),
            ..Default::default()
        };
        let _ = kill_tree::tokio::kill_tree_with_config(pid, &config).await;
    }
    let _ = child.kill().await;
    outcome
}

/// Sign `adapter` out on this Host: the saved key is deleted and the vendor
/// CLI signs its account out. Running sessions of the agent are ended by the
/// caller so none keeps using the credential.
pub async fn sign_out(adapter: ManagedAdapter) -> Result<(), AuthError> {
    stop_sign_in(adapter);
    let removed = persist(adapter, None).await;
    let logged_out = vendor_sign_out(adapter).await;
    forget_cached(adapter);
    removed?;
    logged_out?;
    tracing::info!("[agent-auth] {} signed out", adapter.id());
    Ok(())
}

async fn vendor_sign_out(adapter: ManagedAdapter) -> Result<(), AuthError> {
    // Signed in with a key only: Claude Code holds no login for its CLI to
    // clear, and the key is already gone.
    if matches!(adapter, ManagedAdapter::Claude) && !claude_account_saved() {
        return Ok(());
    }
    let result = match vendor_command(adapter, sign_out_args(adapter)) {
        Ok(mut cmd) => match cmd.spawn() {
            Ok(child) => {
                let (_keep, never) = tokio::sync::oneshot::channel();
                match wait_or_end(child, never, SIGN_OUT_TIMEOUT).await {
                    Some(true) => Ok(()),
                    _ => Err(AuthError::SignOut("the sign-out command failed".into())),
                }
            }
            Err(e) => Err(AuthError::SignOut(e.to_string())),
        },
        Err(e) => Err(e),
    };
    match adapter {
        // Codex's login is its `auth.json`: without the CLI, removing the
        // file is the sign-out.
        ManagedAdapter::Codex => {
            let auth = crate::commands::acp::codex_home_dir().join("auth.json");
            match std::fs::remove_file(&auth) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(AuthError::SignOut(e.to_string())),
            }
        }
        // Claude Code's login lives in its own Keychain item: only its CLI
        // clears that.
        ManagedAdapter::Claude => result,
    }
}

/// Claude Code's own login: its credentials file, or its Keychain item on a
/// Mac — read afresh, not from the cache.
fn claude_account_saved() -> bool {
    adapters::forget_claude_keychain_item();
    adapters::claude_config_dir()
        .join(".credentials.json")
        .is_file()
        || adapters::claude_keychain_item()
}

#[cfg(test)]
mod tests;
