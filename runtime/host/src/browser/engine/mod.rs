//! The browser engine: a `chrome-headless-shell` child process the Host
//! downloads, starts and talks CDP to (design 5). Clients never see it; they
//! see the tabs `BrowserService` reports.
//!
//! - `install` — the pinned download, its hash check and unpacking
//! - `launch`  — starting it with the CDP pipe on fd 3 and 4
//! - `cdp`     — the CDP connection: calls, replies and events
//! - `manager` — one engine kept running: lazy start, restarts, `crash_loop`

pub mod cdp;
#[cfg(test)]
pub mod fake;
pub mod install;
#[cfg(unix)]
pub mod launch;
pub mod manager;

use manager::EngineError;

/// How long a fresh engine has to answer before its start counts as failed.
#[cfg(unix)]
const FIRST_ANSWER: std::time::Duration = std::time::Duration::from_secs(10);

impl From<install::SetupError> for EngineError {
    fn from(e: install::SetupError) -> Self {
        EngineError {
            code: e.code(),
            message: e.to_string(),
        }
    }
}

/// Where the browser keeps its files under the Host's data directory:
/// `engine/<version>/`, `profile/`, `engine.log` and `personal.snapshot`.
pub fn browser_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("browser")
}

/// No engine for this platform (design 11.5).
#[cfg(not(unix))]
pub fn host_start(_root: std::path::PathBuf) -> manager::StartFn {
    manager::start_fn(|| async { Err(install::SetupError::Unsupported.into()) })
}

/// The Host's engine starter: install the pinned engine under `root` if
/// needed, launch it, and wait for its first answer. Older versions are
/// removed only after the new one has answered.
#[cfg(unix)]
pub fn host_start(root: std::path::PathBuf) -> manager::StartFn {
    use futures::FutureExt;

    manager::start_fn(move || {
        let root = root.clone();
        async move {
            let pin = install::pin().ok_or(install::SetupError::Unsupported)?;
            let fetch = crate::adapters::HttpFetch::new();
            let installed = install::Installer {
                root: &root,
                pin,
                fetch: &fetch,
                downloads: Some(&crate::downloads::global()),
            }
            .ensure()
            .await?;
            let failed = |message: String| EngineError {
                code: "setup_failed",
                message,
            };
            let engine = launch::spawn(&launch::LaunchOptions {
                binary: installed.binary,
                profile_dir: launch::profile_dir(&root),
                renderer_limit: None,
                log_file: root.parent().map(|p| p.join("engine.log")),
            })
            .map_err(|e| failed(format!("the browser engine could not start: {e}")))?;
            engine
                .cdp
                .call_with_timeout(
                    "Browser.getVersion",
                    serde_json::json!({}),
                    None,
                    FIRST_ANSWER,
                )
                .await
                .map_err(|e| failed(format!("the browser engine did not answer: {e}")))?;
            install::prune(&root, &installed.version);
            let mut child = engine.child;
            Ok(manager::Running {
                cdp: engine.cdp,
                version: installed.version,
                exited: async move {
                    let _ = child.wait().await;
                }
                .boxed(),
            })
        }
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::manager::{EngineManager, EngineState};
    use super::*;

    /// Downloads the pinned engine (about 100 MB) into a temporary directory,
    /// starts it, kills it and checks it comes back.
    #[tokio::test]
    #[ignore = "downloads the engine; set ZWIIM_TEST_ENGINE_DOWNLOAD=1"]
    async fn the_host_installs_starts_and_restarts_the_real_engine() {
        assert!(std::env::var_os("ZWIIM_TEST_ENGINE_DOWNLOAD").is_some());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("browser").join("engine");
        let m = EngineManager::new(host_start(root.clone()));

        let cdp = m.cdp().await.unwrap();
        let version = cdp
            .call("Browser.getVersion", serde_json::json!({}), None)
            .await
            .unwrap();
        assert!(version["product"]
            .as_str()
            .unwrap()
            .ends_with(install::VERSION));
        assert!(install::installed(&root, &install::pin().unwrap()).is_some());

        // The engine exits on its own; the manager starts a new one.
        let _ = cdp.call("Browser.close", serde_json::json!({}), None).await;
        let mut state = m.state();
        state
            .wait_for(|s| matches!(s, EngineState::Running { generation: 2, .. }))
            .await
            .unwrap();
        m.stop().await;
    }
}
