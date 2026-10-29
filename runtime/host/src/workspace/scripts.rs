//! Zwiim-owned: a repository's own scripts, run by the Host around a
//! worktree's life (see `crate::repository_settings`).
//!
//! The setup script runs after a worktree is created, in a terminal titled
//! `Setup` that every client lists with the worktree's others. It never holds
//! up the create or an agent starting there. Once it ends the terminal says
//! how, then stays open as the user's shell, so its output can still be read.
//!
//! The run script (a dev server, say) runs when a client asks, in a terminal
//! titled `Run`; it ends with the script. A client asks again to run setup
//! again. Either replaces the worktree's live terminal of the same title, so
//! there is one of each at most; clients know them by these titles.
//!
//! The archive script runs before a worktree's checkout is removed, without a
//! terminal or input, for at most [`ARCHIVE_LIMIT`]. Whatever it does, the
//! removal goes on; how it ended is reported with the removal.
//!
//! All run in the user's own shell, in the worktree, with [`ScriptEnv`]
//! saying where they are, the worktree's ports and the repository's
//! environment variables; the worktree's terminals and agents get the same
//! ([`workspace_env`]). Scripts only run where that shell is POSIX-like.

use std::collections::HashMap;
use std::path::Path;
#[cfg(unix)]
use std::process::Stdio;
use std::time::Duration;

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};

use crate::db::entities::folder;
use crate::protocol::error::ZwiimError;
use crate::repository_settings::{self, RUN_SCRIPT, SETUP_SCRIPT};
use crate::terminal::manager::{resolve_shell, SpawnOptions, TerminalManager};
use crate::web::event_bridge::EventEmitter;

/// Longest an archive script may run before it is stopped.
pub const ARCHIVE_LIMIT: Duration = Duration::from_secs(5 * 60);

/// Most of an archive script's output reported, its last bytes.
const OUTPUT_TAIL: usize = 4000;

/// Where a script runs, given to it as `ZWIIM_*` variables, and the
/// repository's own variables beneath them.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptEnv {
    /// The worktree's folder name (`pune`).
    pub workspace_name: String,
    pub workspace_path: String,
    /// The repository the worktree was made from.
    pub root_path: String,
    /// The branch the worktree started from, when known.
    pub base_branch: Option<String>,
    /// The first of the worktree's own ports (`super::ports`), when one is free.
    pub port: Option<u16>,
    /// The repository's environment variables (`crate::repository_settings::env`).
    pub repository: HashMap<String, String>,
    /// Whether a plain `git push` sets the branch's upstream
    /// (`crate::settings::push_sets_upstream`).
    pub push_sets_upstream: bool,
}

impl ScriptEnv {
    pub fn of(root_path: &str, worktree: &folder::Model) -> Self {
        Self {
            workspace_name: Path::new(&worktree.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            workspace_path: worktree.path.clone(),
            root_path: root_path.to_string(),
            base_branch: worktree.base_branch.clone(),
            port: None,
            repository: HashMap::new(),
            push_sets_upstream: false,
        }
    }

    /// [`Self::of`] with the worktree's ports and the variables of
    /// repository `root`.
    pub async fn load(db: &DatabaseConnection, root: &folder::Model, worktree: &folder::Model) -> Self {
        Self {
            port: super::ports::port_of(db, worktree.id).await,
            repository: repository_settings::env(db, root.id).await,
            push_sets_upstream: crate::settings::push_sets_upstream(db).await,
            ..Self::of(&root.path, worktree)
        }
    }

    /// The repository's variables, then the `ZWIIM_*` ones over them.
    fn vars(&self) -> HashMap<String, String> {
        let mut vars = self.repository.clone();
        vars.extend([
            ("ZWIIM_WORKSPACE_NAME".to_string(), self.workspace_name.clone()),
            ("ZWIIM_WORKSPACE_PATH".to_string(), self.workspace_path.clone()),
            ("ZWIIM_ROOT_PATH".to_string(), self.root_path.clone()),
        ]);
        if let Some(base) = &self.base_branch {
            vars.insert("ZWIIM_BASE_BRANCH".to_string(), base.clone());
        }
        if let Some(port) = self.port {
            vars.insert("ZWIIM_PORT".to_string(), port.to_string());
        }
        if self.push_sets_upstream {
            let inherited = std::env::var("GIT_CONFIG_COUNT").ok();
            add_git_config(&mut vars, inherited.as_deref(), "push.autoSetupRemote", "true");
        }
        vars
    }
}

/// Add `key = value` to the git config `vars` give every git run beneath
/// them, after any entries already given there or (`inherited`, the count)
/// by the Host's own environment. Nothing is written to a config file. When
/// the count already given is not a number, git would refuse it whatever is
/// added, so nothing is.
fn add_git_config(vars: &mut HashMap<String, String>, inherited: Option<&str>, key: &str, value: &str) {
    let given = vars.get("GIT_CONFIG_COUNT").map(String::as_str).or(inherited);
    let Ok(n) = given.map_or(Ok(0), |c| c.trim().parse::<usize>()) else {
        return;
    };
    vars.insert(format!("GIT_CONFIG_KEY_{n}"), key.to_string());
    vars.insert(format!("GIT_CONFIG_VALUE_{n}"), value.to_string());
    vars.insert("GIT_CONFIG_COUNT".to_string(), (n + 1).to_string());
}

/// The variables everything started in `workspace` gets: a worktree's
/// [`ScriptEnv`], a repository folder's own variables.
pub async fn workspace_env(db: &DatabaseConnection, workspace: &folder::Model) -> HashMap<String, String> {
    let Some(parent) = workspace.parent_id else {
        return repository_settings::env(db, workspace.id).await;
    };
    match folder::Entity::find_by_id(parent).one(db).await {
        Ok(Some(root)) => ScriptEnv::load(db, &root, workspace).await.vars(),
        _ => HashMap::new(),
    }
}

/// [`workspace_env`] of the live workspace at `working_dir`, for agents
/// started by path; none elsewhere.
pub async fn workspace_env_at(db: &DatabaseConnection, working_dir: Option<&str>) -> HashMap<String, String> {
    let Some(dir) = working_dir else {
        return HashMap::new();
    };
    let found = folder::Entity::find()
        .filter(folder::Column::Path.eq(dir))
        .filter(folder::Column::DeletedAt.is_null())
        .one(db)
        .await;
    match found {
        Ok(Some(workspace)) => workspace_env(db, &workspace).await,
        _ => HashMap::new(),
    }
}

/// A script that runs in a terminal of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Script {
    Setup,
    Run,
}

impl Script {
    fn key(self) -> &'static str {
        match self {
            Self::Setup => SETUP_SCRIPT,
            Self::Run => RUN_SCRIPT,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Run => "run",
        }
    }

    /// Its terminal's title, by which clients know it.
    pub fn title(self) -> &'static str {
        match self {
            Self::Setup => "Setup",
            Self::Run => "Run",
        }
    }

    #[cfg(unix)]
    fn runner(self) -> &'static str {
        match self {
            Self::Setup => SETUP_RUNNER,
            Self::Run => RUN_RUNNER,
        }
    }
}

/// What the setup terminal runs, in `sh`: the script in the user's shell, a
/// line saying how it ended, then that shell for the user. Ctrl-C stops the
/// script, not this.
#[cfg(unix)]
const SETUP_RUNNER: &str = r#"trap : INT
"$ZWIIM_SHELL" -l -i -c "$ZWIIM_SCRIPT"
zwiim_status=$?
if [ "$zwiim_status" -eq 0 ]; then
  printf '\n\033[2mSetup finished.\033[0m\n'
else
  printf '\n\033[31mSetup failed with exit code %s.\033[0m\n' "$zwiim_status"
fi
unset ZWIIM_SCRIPT ZWIIM_CMD zwiim_status
exec "$ZWIIM_SHELL" -l -i"#;

/// What the run terminal runs, in `sh`: the script in the user's shell and a
/// line saying how it ended. Ctrl-C stops the script, not this.
#[cfg(unix)]
const RUN_RUNNER: &str = r#"trap : INT
"$ZWIIM_SHELL" -l -i -c "$ZWIIM_SCRIPT"
zwiim_status=$?
if [ "$zwiim_status" -eq 0 ]; then
  printf '\n\033[2mRun finished.\033[0m\n'
elif [ "$zwiim_status" -eq 130 ]; then
  printf '\n\033[2mRun stopped.\033[0m\n'
else
  printf '\n\033[31mRun failed with exit code %s.\033[0m\n' "$zwiim_status"
fi"#;

/// Start `script`'s `command` in a new terminal of its title in the worktree,
/// with [`ScriptEnv`]'s variables and `extra_env` over the repository's. The
/// terminal's id.
fn start(
    terminals: &TerminalManager,
    emitter: &EventEmitter,
    script: Script,
    command: &str,
    env: &ScriptEnv,
    extra_env: Option<HashMap<String, String>>,
) -> Result<String, String> {
    #[cfg(unix)]
    {
        let mut vars = env.vars();
        // The Host's own (credentials) win over the repository's; they never
        // name a `ZWIIM_*`.
        vars.extend(extra_env.unwrap_or_default());
        vars.insert("ZWIIM_SHELL".to_string(), resolve_shell());
        vars.insert("ZWIIM_SCRIPT".to_string(), command.to_string());
        terminals
            .spawn_with_id(
                SpawnOptions {
                    terminal_id: uuid::Uuid::new_v4().to_string(),
                    working_dir: env.workspace_path.clone(),
                    owner_window_label: "zwiim".to_string(),
                    shell: Some("/bin/sh".to_string()),
                    initial_command: Some(script.runner().to_string()),
                    extra_env: Some(vars),
                    temp_files: vec![],
                    title: Some(script.title().to_string()),
                },
                emitter.clone(),
            )
            .map_err(|e| e.to_string())
    }
    #[cfg(not(unix))]
    {
        let _ = (terminals, emitter, script, command, env, extra_env, resolve_shell);
        Err("scripts don't run on this platform yet".to_string())
    }
}

/// Start the setup script of the repository of new worktree `workspace_id`,
/// if it has one, with `extra_env` besides [`ScriptEnv`]'s. The `Setup`
/// terminal's id, or `None` when nothing started. Never an error: the
/// worktree is made either way.
pub async fn setup_new_worktree(
    db: &DatabaseConnection,
    terminals: &TerminalManager,
    emitter: &EventEmitter,
    workspace_id: &str,
    extra_env: Option<HashMap<String, String>>,
) -> Option<String> {
    // Held until the terminal is registered, so a removal started meanwhile
    // either sees it or has already removed the worktree.
    let (worktree, _launching) = super::resolve_for_launch(db, workspace_id).await.ok()?;
    let root = folder::Entity::find_by_id(worktree.parent_id?)
        .one(db)
        .await
        .ok()??;
    let command = repository_settings::script(db, root.id, SETUP_SCRIPT).await?;
    let env = ScriptEnv::load(db, &root, &worktree).await;
    match start(terminals, emitter, Script::Setup, &command, &env, extra_env) {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(error = %e, "[workspace] the setup script could not start");
            None
        }
    }
}

/// Run worktree `workspace_id`'s repository `script` now, in place of the
/// worktree's live terminal of its title, with `extra_env` besides
/// [`ScriptEnv`]'s. The new terminal's id.
pub async fn run_script(
    db: &DatabaseConnection,
    terminals: &TerminalManager,
    emitter: &EventEmitter,
    workspace_id: &str,
    script: Script,
    extra_env: Option<HashMap<String, String>>,
) -> Result<String, ZwiimError> {
    // One at a time, so two quick asks leave one terminal, not two.
    static STARTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = STARTING.lock().await;
    // Held until the terminal is registered, as for a new worktree's setup.
    let (worktree, _launching) = super::resolve_for_launch(db, workspace_id).await?;
    let Some(parent) = worktree.parent_id else {
        return Err(ZwiimError::root_workspace());
    };
    let root = folder::Entity::find_by_id(parent)
        .one(db)
        .await
        .ok()
        .flatten()
        .ok_or_else(|| ZwiimError::not_found("Workspace not found"))?;
    let command = repository_settings::script(db, root.id, script.key())
        .await
        .ok_or_else(|| ZwiimError::script_not_set(script.name()))?;
    let env = ScriptEnv::load(db, &root, &worktree).await;
    for t in terminals.list_with_exit_check(Some(emitter)) {
        if t.title == script.title() && t.working_dir == worktree.path {
            // Gone meanwhile is as good.
            let _ = terminals.kill(&t.id);
        }
    }
    start(terminals, emitter, script, &command, &env, extra_env).map_err(|e| {
        tracing::warn!(error = %e, script = script.name(), "[workspace] a script could not start");
        #[cfg(unix)]
        {
            ZwiimError::internal()
        }
        #[cfg(not(unix))]
        {
            ZwiimError::invalid_request("Scripts don't run on this platform yet")
        }
    })
}

/// How an archive script ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ScriptRun {
    /// Its exit code; `None` when it was stopped, killed or never started.
    pub exit_code: Option<i32>,
    /// Stopped after [`ARCHIVE_LIMIT`].
    pub timed_out: bool,
    /// The end of what it printed, or why it could not start.
    pub output: String,
}

impl ScriptRun {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// Run archive script `script` in the worktree and wait for it, at most
/// `limit`; then its whole process group is stopped.
pub async fn run_archive(script: &str, env: &ScriptEnv, limit: Duration) -> ScriptRun {
    #[cfg(unix)]
    {
        run_unix(script, env, limit).await
    }
    #[cfg(not(unix))]
    {
        let _ = (script, env, limit);
        ScriptRun {
            exit_code: None,
            timed_out: false,
            output: "Archive scripts don't run on this platform yet.".to_string(),
        }
    }
}

#[cfg(unix)]
async fn run_unix(script: &str, env: &ScriptEnv, limit: Duration) -> ScriptRun {
    use tokio::io::AsyncReadExt;

    // Its output and errors as one stream, in the order printed.
    let mut cmd = crate::process::tokio_command("/bin/sh");
    cmd.args(["-c", r#"exec "$ZWIIM_SHELL" -l -c "$ZWIIM_SCRIPT" 2>&1"#])
        .current_dir(&env.workspace_path)
        .envs(env.vars())
        .env("ZWIIM_SHELL", resolve_shell())
        .env("ZWIIM_SCRIPT", script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return ScriptRun {
                exit_code: None,
                timed_out: false,
                output: format!("The archive script could not start: {e}"),
            }
        }
    };
    let group = child.id().and_then(|p| i32::try_from(p).ok());
    let mut stdout = child.stdout.take();
    let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let reader = {
        let tail = tail.clone();
        tokio::spawn(async move {
            let Some(stdout) = stdout.as_mut() else { return };
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = stdout.read(&mut buf).await {
                let mut tail = tail.lock().unwrap_or_else(|e| e.into_inner());
                tail.extend_from_slice(&buf[..n]);
                let over = tail.len().saturating_sub(OUTPUT_TAIL);
                tail.drain(..over);
            }
        })
    };

    let waited = tokio::time::timeout(limit, child.wait()).await;
    let timed_out = waited.is_err();
    if timed_out {
        if let Some(pgid) = group {
            // SAFETY: plain syscall on the group this function created; ESRCH
            // when it is already gone is fine.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
        let _ = child.wait().await;
    }
    // Something the script left running may hold the output open: read what
    // came, not what may yet.
    let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
    let output = {
        let tail = tail.lock().unwrap_or_else(|e| e.into_inner());
        String::from_utf8_lossy(&tail).trim_end().to_string()
    };
    ScriptRun {
        exit_code: waited.ok().and_then(|s| s.ok()).and_then(|s| s.code()),
        timed_out,
        output,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn env(dir: &Path) -> ScriptEnv {
        ScriptEnv {
            workspace_name: "pune".to_string(),
            workspace_path: dir.to_string_lossy().into_owned(),
            root_path: "/repo".to_string(),
            base_branch: Some("main".to_string()),
            port: Some(40_000),
            repository: HashMap::from([
                ("API_URL".to_string(), "http://localhost:4000".to_string()),
                ("ZWIIM_ROOT_PATH".to_string(), "/not-the-repo".to_string()),
            ]),
            push_sets_upstream: false,
        }
    }

    #[test]
    fn git_config_goes_after_what_is_already_given() {
        let mut vars = HashMap::new();
        add_git_config(&mut vars, None, "push.autoSetupRemote", "true");
        assert_eq!(vars["GIT_CONFIG_COUNT"], "1");
        assert_eq!(vars["GIT_CONFIG_KEY_0"], "push.autoSetupRemote");
        assert_eq!(vars["GIT_CONFIG_VALUE_0"], "true");

        // The Host's own environment gave two; the repository's count wins over it.
        let mut vars = HashMap::new();
        add_git_config(&mut vars, Some("2"), "push.autoSetupRemote", "true");
        assert_eq!(vars["GIT_CONFIG_COUNT"], "3");
        assert_eq!(vars["GIT_CONFIG_KEY_2"], "push.autoSetupRemote");
        let mut vars = HashMap::from([("GIT_CONFIG_COUNT".to_string(), "1".to_string())]);
        add_git_config(&mut vars, Some("2"), "push.autoSetupRemote", "true");
        assert_eq!(vars["GIT_CONFIG_COUNT"], "2");
        assert_eq!(vars["GIT_CONFIG_KEY_1"], "push.autoSetupRemote");

        let mut vars = HashMap::new();
        add_git_config(&mut vars, Some("lots"), "push.autoSetupRemote", "true");
        assert!(vars.is_empty());
    }

    #[tokio::test]
    async fn git_run_in_a_worktree_sees_push_set_upstream() {
        let dir = tempfile::tempdir().unwrap();
        let script = "git config --get push.autoSetupRemote";
        // Away from this machine's own git config, which may set it already.
        let mut off = env(dir.path());
        off.repository.extend([
            ("GIT_CONFIG_GLOBAL".to_string(), "/dev/null".to_string()),
            ("GIT_CONFIG_NOSYSTEM".to_string(), "1".to_string()),
        ]);
        let on = ScriptEnv { push_sets_upstream: true, ..off.clone() };
        let ran = run_archive(script, &on, ARCHIVE_LIMIT).await;
        assert_eq!(ran.output, "true");
        let ran = run_archive(script, &off, ARCHIVE_LIMIT).await;
        assert_eq!(ran.output, "");
    }

    #[tokio::test]
    async fn archive_runs_in_the_worktree_and_reports_how_it_ended() {
        let dir = tempfile::tempdir().unwrap();
        let ran = run_archive(
            "pwd; echo \"$ZWIIM_WORKSPACE_NAME $ZWIIM_ROOT_PATH $ZWIIM_BASE_BRANCH $ZWIIM_PORT $API_URL\"; echo oops >&2; exit 3",
            &env(dir.path()),
            ARCHIVE_LIMIT,
        )
        .await;
        assert_eq!(ran.exit_code, Some(3));
        assert!(!ran.timed_out && !ran.succeeded());
        let canonical = dir.path().canonicalize().unwrap();
        assert!(ran.output.contains(&*canonical.to_string_lossy()), "{}", ran.output);
        assert!(ran.output.contains("pune /repo main 40000 http://localhost:4000\n"), "{}", ran.output);
        assert!(ran.output.ends_with("oops"), "{}", ran.output);
    }

    #[tokio::test]
    async fn archive_is_stopped_at_its_limit_with_what_it_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let ran = run_archive(
            "sleep 30 & echo started; sleep 30",
            &env(dir.path()),
            Duration::from_millis(1500),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(ran.timed_out);
        assert_eq!(ran.exit_code, None);
        assert_eq!(ran.output, "started");
    }

    #[tokio::test]
    async fn archive_reads_no_input_and_keeps_only_the_end_of_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let ran = run_archive(
            "read line && echo \"read $line\"; i=0; while [ $i -lt 2000 ]; do echo line$i; i=$((i+1)); done",
            &env(dir.path()),
            ARCHIVE_LIMIT,
        )
        .await;
        assert!(ran.succeeded());
        assert!(!ran.output.contains("read "));
        assert!(ran.output.len() <= OUTPUT_TAIL);
        assert!(ran.output.ends_with("line1999"));
    }
}
