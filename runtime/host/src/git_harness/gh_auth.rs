//! Signing the Host's `gh` in to GitHub from a Zwiim client, through gh's own
//! device flow: the Host runs `gh auth login --web`, reads the one-time code
//! and the address gh prints, and hands both to the client, which shows the
//! code and opens the address. gh stores its login where it always does; Zwiim
//! never sees or keeps a token.
//!
//! One sign-in runs at a time. Asking again while one runs returns it; it ends
//! when gh exits, after [`SIGN_IN_TIMEOUT`], or when cancelled. What gh prints
//! never reaches a client: a failed attempt is described by a Zwiim sentence,
//! reported once.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{oneshot, watch};

use super::error::{classify_gh, GitErrorCode};
use super::run::{self, GroupKill, GITHUB_READ};

/// How long a sign-in may wait for the user to enter the code.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long `sign_in` waits for gh to print the code.
const CODE_WAIT: Duration = Duration::from_secs(10);

/// The only address a client is asked to open.
const DEVICE_HOST: &str = "github.com";

static CODE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\b[A-Z0-9]{4}-[A-Z0-9]{4}\b").expect("valid pattern"));

const TIMED_OUT: &str = "GitHub sign-in timed out before the code was entered. Try again.";
const DENIED: &str = "GitHub sign-in was denied.";
const EXPIRED: &str = "The GitHub code expired before it was entered. Try again.";
const UNREACHABLE: &str = "Couldn't reach GitHub. Check the connection and try again.";
const FAILED: &str = "gh couldn't sign in to GitHub. Try again.";
const NO_CODE: &str = "gh didn't show a sign-in code. Try again.";
const BAD_URL: &str = "gh asked to open an unexpected address, so sign-in was stopped.";
const NOT_STARTED: &str = "Couldn't start gh to sign in to GitHub.";
const STILL_SIGNED_OUT: &str = "gh finished, but GitHub still shows it signed out. Try again.";

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GithubAuthState {
    SignedOut,
    SigningIn,
    SignedIn,
    /// gh is not installed.
    Unavailable,
}

/// How the Host's `gh` is signed in to GitHub.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GithubAuth {
    pub state: GithubAuthState,
    /// The signed-in account, when `signed_in`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    /// The one-time code to enter, while `signing_in`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Where to enter it, while `signing_in`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Why the last attempt failed, reported once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl GithubAuth {
    fn of(state: GithubAuthState) -> Self {
        Self {
            state,
            login: None,
            code: None,
            url: None,
            error: None,
        }
    }

    fn signed_out(error: Option<String>) -> Self {
        Self {
            error,
            ..Self::of(GithubAuthState::SignedOut)
        }
    }

    fn signed_in(login: String) -> Self {
        Self {
            login: Some(login),
            ..Self::of(GithubAuthState::SignedIn)
        }
    }
}

/// The running sign-in.
struct Job {
    code: Option<String>,
    url: Option<String>,
    /// Dropping it ends the sign-in.
    _cancel: oneshot::Sender<()>,
    /// Changes once the code is known; closes when the sign-in ends.
    prompt: watch::Receiver<bool>,
}

#[derive(Default)]
struct Slot {
    /// Bumped by every start and cancel, so an ended sign-in cannot report
    /// over a newer one.
    generation: u64,
    job: Option<Job>,
    last_error: Option<String>,
}

/// The Host's GitHub sign-in: at most one at a time.
pub struct GithubSignIn {
    slot: Arc<Mutex<Slot>>,
    /// Held while a sign-in starts, so two at once start one.
    starting: tokio::sync::Mutex<()>,
    timeout: Duration,
    code_wait: Duration,
}

impl Default for GithubSignIn {
    fn default() -> Self {
        Self::with_limits(SIGN_IN_TIMEOUT, CODE_WAIT)
    }
}

impl GithubSignIn {
    pub fn with_limits(timeout: Duration, code_wait: Duration) -> Self {
        Self {
            slot: Arc::default(),
            starting: tokio::sync::Mutex::new(()),
            timeout,
            code_wait,
        }
    }

    /// How gh is signed in now. A sign-in in progress reads `signing_in`;
    /// otherwise gh is asked. The last attempt's failure is reported once.
    pub async fn status(&self, gh: Option<&Path>, cwd: &str) -> GithubAuth {
        let Ok(program) = run::find_gh(gh) else {
            return GithubAuth::of(GithubAuthState::Unavailable);
        };
        if let Some(running) = self.running() {
            return running;
        }
        match signed_in_login(&program, cwd).await {
            Login::In(login) => {
                self.slot.lock().unwrap().last_error = None;
                GithubAuth::signed_in(login)
            }
            Login::Out => GithubAuth::signed_out(self.slot.lock().unwrap().last_error.take()),
            Login::Unreachable => {
                let last = self.slot.lock().unwrap().last_error.take();
                GithubAuth::signed_out(Some(last.unwrap_or_else(|| UNREACHABLE.to_string())))
            }
        }
    }

    /// Start signing gh in, and wait briefly for the code to show. Signed in
    /// already reads `signed_in`; a sign-in in progress is returned as is.
    pub async fn sign_in(&self, gh: Option<&Path>, cwd: &str) -> GithubAuth {
        let Ok(program) = run::find_gh(gh) else {
            return GithubAuth::of(GithubAuthState::Unavailable);
        };
        let starting = self.starting.lock().await;
        let (generation, prompt) = match self.running_prompt() {
            Some(running) => running,
            None => {
                if let Login::In(login) = signed_in_login(&program, cwd).await {
                    return GithubAuth::signed_in(login);
                }
                match self.start(&program, cwd).await {
                    Ok(started) => started,
                    Err(error) => return GithubAuth::signed_out(Some(error.to_string())),
                }
            }
        };
        drop(starting);
        if !self.await_code(prompt).await {
            // Only this run: a cancel and a newer sign-in may have come since.
            self.end_run(generation);
            tracing::info!("[github-auth] gh showed no code; sign-in stopped");
            return GithubAuth::signed_out(Some(NO_CODE.to_string()));
        }
        self.status(Some(&program), cwd).await
    }

    /// End the sign-in in progress, if any, and read the state after.
    pub async fn cancel(&self, gh: Option<&Path>, cwd: &str) -> GithubAuth {
        if self.end() {
            tracing::info!("[github-auth] sign-in cancelled");
        }
        self.slot.lock().unwrap().last_error = None;
        self.status(gh, cwd).await
    }

    fn running(&self) -> Option<GithubAuth> {
        let slot = self.slot.lock().unwrap();
        let job = slot.job.as_ref()?;
        Some(GithubAuth {
            code: job.code.clone(),
            url: job.url.clone(),
            ..GithubAuth::of(GithubAuthState::SigningIn)
        })
    }

    fn running_prompt(&self) -> Option<(u64, watch::Receiver<bool>)> {
        let slot = self.slot.lock().unwrap();
        let job = slot.job.as_ref()?;
        Some((slot.generation, job.prompt.clone()))
    }

    /// Whether the code showed within the wait. A sign-in that ended
    /// meanwhile counts as shown: its outcome is the answer.
    async fn await_code(&self, mut prompt: watch::Receiver<bool>) -> bool {
        tokio::time::timeout(self.code_wait, prompt.wait_for(|shown| *shown))
            .await
            .is_ok()
    }

    /// End the running sign-in; whether there was one.
    fn end(&self) -> bool {
        let mut slot = self.slot.lock().unwrap();
        slot.generation += 1;
        slot.job.take().is_some()
    }

    /// End the sign-in of `generation`, unless a newer one has begun.
    fn end_run(&self, generation: u64) -> bool {
        let mut slot = self.slot.lock().unwrap();
        if slot.generation != generation {
            return false;
        }
        slot.generation += 1;
        slot.job.take().is_some()
    }

    async fn start(
        &self,
        program: &Path,
        cwd: &str,
    ) -> Result<(u64, watch::Receiver<bool>), &'static str> {
        let mut cmd = run::gh(program, cwd);
        cmd.args([
            "auth",
            "login",
            "--web",
            "--hostname",
            DEVICE_HOST,
            "--git-protocol",
            "https",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        let child = crate::process::spawn_retrying_exec_busy(|| cmd.spawn())
            .await
            .map_err(|e| {
                tracing::warn!("[github-auth] gh did not start: {e}");
                NOT_STARTED
            })?;
        let group = GroupKill::new(child.id());
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (prompt_tx, prompt_rx) = watch::channel(false);
        let generation = {
            let mut slot = self.slot.lock().unwrap();
            slot.generation += 1;
            slot.last_error = None;
            slot.job = Some(Job {
                code: None,
                url: None,
                _cancel: cancel_tx,
                prompt: prompt_rx.clone(),
            });
            slot.generation
        };
        tracing::info!("[github-auth] sign-in started");
        let run = Run {
            slot: self.slot.clone(),
            generation,
            program: program.to_path_buf(),
            cwd: cwd.to_string(),
        };
        tokio::spawn(run.drive(child, group, cancel_rx, prompt_tx, self.timeout));
        Ok((generation, prompt_rx))
    }
}

/// How a sign-in ended.
enum End {
    Exited(bool),
    TimedOut,
    Cancelled,
    BadUrl,
}

/// One sign-in, followed in the background.
struct Run {
    slot: Arc<Mutex<Slot>>,
    generation: u64,
    program: PathBuf,
    cwd: String,
}

impl Run {
    async fn drive(
        self,
        mut child: tokio::process::Child,
        group: GroupKill,
        mut cancel: oneshot::Receiver<()>,
        prompt: watch::Sender<bool>,
        timeout: Duration,
    ) {
        let mut stderr = child.stderr.take().map(|s| BufReader::new(s).lines());
        let mut stdout = child.stdout.take().map(|s| BufReader::new(s).lines());
        let mut seen = Seen::default();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let end = loop {
            tokio::select! {
                line = next_line(&mut stderr), if stderr.is_some() => match line {
                    Some(line) => if !self.read(&line, &mut seen, &prompt) { break End::BadUrl },
                    None => stderr = None,
                },
                line = next_line(&mut stdout), if stdout.is_some() => match line {
                    Some(line) => if !self.read(&line, &mut seen, &prompt) { break End::BadUrl },
                    None => stdout = None,
                },
                status = child.wait() => break End::Exited(status.is_ok_and(|s| s.success())),
                _ = &mut deadline => break End::TimedOut,
                _ = &mut cancel => break End::Cancelled,
            }
        };
        match end {
            End::Exited(true) => group.disarm(),
            _ => {
                drop(group);
                let _ = child.start_kill();
            }
        }
        // Whatever gh printed after the code says why it failed.
        if matches!(end, End::Exited(false)) {
            let rest = Duration::from_secs(1);
            while let Ok(Some(line)) = tokio::time::timeout(rest, next_line(&mut stderr)).await {
                seen.keep(line);
            }
            while let Ok(Some(line)) = tokio::time::timeout(rest, next_line(&mut stdout)).await {
                seen.keep(line);
            }
        }
        let _ = child.wait().await;

        let error = match end {
            End::Exited(true) => match signed_in_login(&self.program, &self.cwd).await {
                Login::In(_) => None,
                Login::Out => Some(STILL_SIGNED_OUT),
                Login::Unreachable => Some(UNREACHABLE),
            },
            End::Exited(false) => Some(failure(&seen.tail.join("\n"))),
            End::TimedOut => Some(TIMED_OUT),
            End::BadUrl => Some(BAD_URL),
            End::Cancelled => None,
        };
        let mut slot = self.slot.lock().unwrap();
        if slot.generation != self.generation {
            return;
        }
        slot.job = None;
        slot.last_error = error.map(str::to_string);
        drop(slot);
        match error {
            None => tracing::info!("[github-auth] sign-in finished"),
            Some(why) => tracing::info!("[github-auth] sign-in did not finish: {why}"),
        }
    }

    /// Take the code and address from one line of gh's output; false when
    /// the address is not GitHub's.
    fn read(&self, line: &str, seen: &mut Seen, prompt: &watch::Sender<bool>) -> bool {
        seen.keep(line.to_string());
        if seen.code.is_none() {
            seen.code = CODE.find(line).map(|m| m.as_str().to_string());
        }
        if seen.url.is_none() {
            let mut links = line.split_whitespace().filter(|t| {
                let t = t.to_ascii_lowercase();
                t.starts_with("http://") || t.starts_with("https://")
            });
            let mut any = false;
            let found = links.find_map(|token| {
                any = true;
                device_url(token)
            });
            match found {
                Some(url) => seen.url = Some(url),
                // With the code shown, the address is what should follow: an
                // address that is not GitHub's is refused. Before that, gh
                // may be linking elsewhere, and is let go on.
                None if any && seen.code.is_some() => {
                    tracing::warn!("[github-auth] gh printed an address not on GitHub");
                    return false;
                }
                None => {}
            }
        }
        if let (Some(code), Some(url)) = (&seen.code, &seen.url) {
            let mut slot = self.slot.lock().unwrap();
            if slot.generation == self.generation {
                if let Some(job) = slot.job.as_mut().filter(|j| j.code.is_none()) {
                    job.code = Some(code.clone());
                    job.url = Some(url.clone());
                    prompt.send_replace(true);
                }
            }
        }
        true
    }
}

/// How many of gh's last lines are kept to say why it failed.
const TAIL_LINES: usize = 20;

#[derive(Default)]
struct Seen {
    code: Option<String>,
    url: Option<String>,
    /// gh's last lines, to say why it failed.
    tail: Vec<String>,
}

impl Seen {
    fn keep(&mut self, line: String) {
        if self.tail.len() >= TAIL_LINES {
            self.tail.remove(0);
        }
        self.tail.push(line);
    }
}

async fn next_line<R>(lines: &mut Option<tokio::io::Lines<BufReader<R>>>) -> Option<String>
where
    R: tokio::io::AsyncRead + Unpin,
{
    match lines {
        Some(lines) => lines.next_line().await.ok().flatten(),
        None => None,
    }
}

/// `token` when it is an https address on GitHub itself.
fn device_url(token: &str) -> Option<String> {
    let url = url::Url::parse(token).ok()?;
    let ok = url.scheme() == "https"
        && url.host_str() == Some(DEVICE_HOST)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none();
    ok.then(|| url.to_string())
}

/// A Zwiim sentence for gh's failure, never gh's own words.
fn failure(output: &str) -> &'static str {
    let lower = output.to_lowercase();
    if lower.contains("denied") {
        DENIED
    } else if lower.contains("expired") {
        EXPIRED
    } else if classify_gh("auth login", Some(1), output).code == GitErrorCode::Network {
        UNREACHABLE
    } else {
        FAILED
    }
}

#[derive(Deserialize)]
struct AuthStatus {
    #[serde(default)]
    hosts: std::collections::HashMap<String, Vec<HostAccount>>,
}

#[derive(Deserialize)]
struct HostAccount {
    #[serde(default)]
    state: String,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    login: String,
    #[serde(default)]
    error: String,
}

/// What gh says about the github.com account.
#[derive(Debug, PartialEq)]
enum Login {
    /// Signed in as this account.
    In(String),
    /// No account is signed in.
    Out,
    /// gh couldn't reach GitHub to check.
    Unreachable,
}

/// Whether a failed gh run's output says GitHub couldn't be reached.
fn unreachable(exit: Option<i32>, output: &str) -> bool {
    classify_gh("auth status", exit, output).code == GitErrorCode::Network
}

/// The account gh is actively signed in as on github.com, if any.
async fn signed_in_login(program: &Path, cwd: &str) -> Login {
    let mut cmd = run::gh(program, cwd);
    cmd.args(["auth", "status", "--json", "hosts"]);
    let out = match run::run(cmd, GITHUB_READ, "gh auth status").await {
        Ok(out) => out,
        Err(e) => {
            tracing::warn!("[github-auth] gh auth status: {}", e.message);
            return Login::Out;
        }
    };
    if let Ok(status) = serde_json::from_str::<AuthStatus>(out.stdout.trim()) {
        return active_login(status);
    }
    // A gh older than `auth status --json`: ask who the account is.
    if out.stderr.to_lowercase().contains("unknown flag") {
        return match super::github_account(Some(program), cwd).await {
            Ok(account) => Login::In(account.login),
            Err(e) if e.code == GitErrorCode::Network => Login::Unreachable,
            Err(_) => Login::Out,
        };
    }
    if !out.status.success() && unreachable(out.status.code(), &out.stderr) {
        tracing::warn!("[github-auth] gh auth status: GitHub unreachable");
        return Login::Unreachable;
    }
    Login::Out
}

fn active_login(status: AuthStatus) -> Login {
    let accounts = status
        .hosts
        .get(DEVICE_HOST)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if let Some(account) = accounts
        .iter()
        .find(|a| a.active && a.state == "success" && !a.login.is_empty())
    {
        return Login::In(account.login.clone());
    }
    // gh couldn't check the token: say so rather than "signed out".
    if accounts
        .iter()
        .any(|a| a.state == "timeout" || (a.state == "error" && unreachable(Some(1), &a.error)))
    {
        return Login::Unreachable;
    }
    Login::Out
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::git_harness::fake_gh;

    const SIGNED_IN: &str = r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"octo","tokenSource":"keyring"}]}}"#;
    const SIGNED_OUT: &str = r#"{"hosts":{}}"#;

    /// gh's two lines, as it prints them on stderr.
    fn prompt_lines(url: &str) -> String {
        format!(
            "echo '' >&2; echo '! First copy your one-time code: 36BE-F43F' >&2; echo 'Open this URL to continue in your web browser: {url}' >&2"
        )
    }

    /// A fake gh signed in once `dir/signed-in` exists, whose login runs
    /// `login`.
    fn gh(dir: &Path, login: &str) -> (PathBuf, PathBuf) {
        let flag = dir.join("signed-in");
        fake_gh(
            dir,
            &format!(
                "\"auth status --json hosts\") if [ -f '{flag}' ]; then echo '{SIGNED_IN}'; else echo '{SIGNED_OUT}'; fi ;;\n\"auth login --web --hostname github.com --git-protocol https\") echo \"config=${{GH_CONFIG_DIR-unset}}\" >> '{dir}/env'; {login} ;;",
                flag = flag.display(),
                dir = dir.display(),
            ),
        )
    }

    fn quick() -> GithubSignIn {
        GithubSignIn::with_limits(Duration::from_secs(30), Duration::from_secs(5))
    }

    fn cwd(dir: &Path) -> String {
        dir.to_string_lossy().into_owned()
    }

    /// Poll `status` until it leaves `signing_in`.
    async fn settled(auth: &GithubSignIn, gh: &Path, dir: &Path) -> GithubAuth {
        for _ in 0..200 {
            let now = auth.status(Some(gh), &cwd(dir)).await;
            if now.state != GithubAuthState::SigningIn {
                return now;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("sign-in never settled");
    }

    fn logins(calls: &Path) -> usize {
        std::fs::read_to_string(calls)
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("auth login"))
            .count()
    }

    #[tokio::test]
    async fn the_code_and_address_come_from_gh_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "{}; exec sleep 30",
            prompt_lines("https://github.com/login/device")
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = quick();
        let started = auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(started.state, GithubAuthState::SigningIn);
        assert_eq!(started.code.as_deref(), Some("36BE-F43F"));
        assert_eq!(
            started.url.as_deref(),
            Some("https://github.com/login/device")
        );
        assert_eq!(started.error, None);
        let polled = auth.status(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(polled, started);
        auth.cancel(Some(&gh), &cwd(dir.path())).await;
    }

    #[tokio::test]
    async fn gh_finishing_reads_signed_in_with_the_login() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "{}; sleep 0.3; touch '{}'; exit 0",
            prompt_lines("https://github.com/login/device"),
            dir.path().join("signed-in").display()
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = quick();
        let started = auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(started.state, GithubAuthState::SigningIn);
        let done = settled(&auth, &gh, dir.path()).await;
        assert_eq!(done, GithubAuth::signed_in("octo".into()));
        // Signed in already: no second login.
        let again = auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(again, GithubAuth::signed_in("octo".into()));
    }

    #[tokio::test]
    async fn gh_failing_reads_signed_out_with_a_zwiim_sentence_once() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "{}; sleep 0.2; echo 'error: some raw gh failure /secret/path' >&2; exit 1",
            prompt_lines("https://github.com/login/device")
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = quick();
        auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        let failed = settled(&auth, &gh, dir.path()).await;
        assert_eq!(failed.state, GithubAuthState::SignedOut);
        assert_eq!(failed.error.as_deref(), Some(FAILED));
        // Reported once.
        let after = auth.status(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(after, GithubAuth::signed_out(None));
    }

    #[tokio::test]
    async fn a_denied_sign_in_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "{}; sleep 0.2; echo 'failed to authenticate via web browser: access_denied' >&2; exit 1",
            prompt_lines("https://github.com/login/device")
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = quick();
        let first = auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(first.state, GithubAuthState::SigningIn);
        let failed = settled(&auth, &gh, dir.path()).await;
        assert_eq!(failed.error.as_deref(), Some(DENIED));
    }

    #[tokio::test]
    async fn cancel_kills_gh() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let login = format!(
            "{}; echo $$ > '{}'; exec sleep 30",
            prompt_lines("https://github.com/login/device"),
            pidfile.display()
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = quick();
        assert_eq!(
            auth.sign_in(Some(&gh), &cwd(dir.path())).await.state,
            GithubAuthState::SigningIn
        );
        let pid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let cancelled = auth.cancel(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(cancelled, GithubAuth::signed_out(None));
        for _ in 0..200 {
            // SAFETY: probing a pid this test started.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("gh outlived the cancel");
    }

    #[tokio::test]
    async fn a_second_sign_in_returns_the_same_one() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "{}; exec sleep 30",
            prompt_lines("https://github.com/login/device")
        );
        let (gh, calls) = gh(dir.path(), &login);
        let auth = quick();
        let here = cwd(dir.path());
        let (a, b) = tokio::join!(
            auth.sign_in(Some(&gh), &here),
            auth.sign_in(Some(&gh), &here)
        );
        assert_eq!(a.state, GithubAuthState::SigningIn);
        assert_eq!(a, b);
        let c = auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(c, a);
        assert_eq!(logins(&calls), 1);
        auth.cancel(Some(&gh), &cwd(dir.path())).await;
    }

    #[tokio::test]
    async fn an_address_off_github_is_refused() {
        for url in [
            "https://evil.example/login/device",
            "http://github.com/login/device",
            "https://github.com.evil.example/login/device",
            "https://user@github.com/login/device",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let login = format!("{}; exec sleep 30", prompt_lines(url));
            let (gh, _) = gh(dir.path(), &login);
            let auth = quick();
            let refused = auth.sign_in(Some(&gh), &cwd(dir.path())).await;
            assert_eq!(refused.state, GithubAuthState::SignedOut, "{url}");
            assert_eq!(refused.error.as_deref(), Some(BAD_URL), "{url}");
            assert_eq!(refused.url, None, "{url}");
        }
    }

    #[tokio::test]
    async fn the_device_address_is_picked_among_other_links() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "echo 'Docs: https://cli.github.com/manual/gh_auth_login' >&2; {}; echo 'More at https://example.org/help' >&2; exec sleep 30",
            prompt_lines("https://example.org/first https://github.com/login/device")
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = quick();
        let started = auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(started.state, GithubAuthState::SigningIn);
        assert_eq!(started.code.as_deref(), Some("36BE-F43F"));
        assert_eq!(
            started.url.as_deref(),
            Some("https://github.com/login/device")
        );
        assert_eq!(started.error, None);
        auth.cancel(Some(&gh), &cwd(dir.path())).await;
    }

    #[tokio::test]
    async fn a_cancelled_sign_ins_cleanup_leaves_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "{}; exec sleep 30",
            prompt_lines("https://github.com/login/device")
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = quick();
        let here = cwd(dir.path());
        auth.sign_in(Some(&gh), &here).await;
        let (old, _) = auth.running_prompt().unwrap();
        auth.cancel(Some(&gh), &here).await;
        let newer = auth.sign_in(Some(&gh), &here).await;
        assert_eq!(newer.state, GithubAuthState::SigningIn);
        assert!(!auth.end_run(old));
        assert_eq!(auth.status(Some(&gh), &here).await, newer);
        auth.cancel(Some(&gh), &here).await;
    }

    #[test]
    fn only_gh_s_last_lines_are_kept() {
        let mut seen = Seen::default();
        for n in 0..TAIL_LINES + 5 {
            seen.keep(format!("line {n}"));
        }
        assert_eq!(seen.tail.len(), TAIL_LINES);
        assert_eq!(seen.tail[0], "line 5");
    }

    #[tokio::test]
    async fn gh_showing_no_code_stops_the_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let (gh, _) = gh(dir.path(), "exec sleep 30");
        let auth = GithubSignIn::with_limits(Duration::from_secs(30), Duration::from_millis(300));
        let stopped = auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        assert_eq!(stopped, GithubAuth::signed_out(Some(NO_CODE.into())));
        assert_eq!(
            auth.status(Some(&gh), &cwd(dir.path())).await,
            GithubAuth::signed_out(None)
        );
    }

    #[tokio::test]
    async fn a_sign_in_left_waiting_times_out() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "{}; exec sleep 30",
            prompt_lines("https://github.com/login/device")
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = GithubSignIn::with_limits(Duration::from_millis(500), Duration::from_secs(5));
        auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        let ended = settled(&auth, &gh, dir.path()).await;
        assert_eq!(ended, GithubAuth::signed_out(Some(TIMED_OUT.into())));
    }

    #[tokio::test]
    async fn missing_gh_reads_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-gh");
        let auth = quick();
        let unavailable = GithubAuth::of(GithubAuthState::Unavailable);
        assert_eq!(auth.status(Some(&missing), "/").await, unavailable);
        assert_eq!(auth.sign_in(Some(&missing), "/").await, unavailable);
        assert_eq!(auth.cancel(Some(&missing), "/").await, unavailable);
    }

    #[tokio::test]
    async fn signed_in_reads_the_active_github_account() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("signed-in"), "").unwrap();
        let (gh, calls) = gh(dir.path(), "exit 1");
        let auth = quick();
        assert_eq!(
            auth.sign_in(Some(&gh), &cwd(dir.path())).await,
            GithubAuth::signed_in("octo".into())
        );
        assert_eq!(logins(&calls), 0);
    }

    #[test]
    fn only_a_successful_active_github_account_counts() {
        let parse = |s: &str| active_login(serde_json::from_str(s).unwrap());
        assert_eq!(parse(SIGNED_IN), Login::In("octo".into()));
        assert_eq!(parse(SIGNED_OUT), Login::Out);
        assert_eq!(
            parse(r#"{"hosts":{"github.com":[{"state":"error","active":true,"login":"octo"}]}}"#),
            Login::Out
        );
        assert_eq!(
            parse(
                r#"{"hosts":{"github.com":[{"state":"success","active":false,"login":"old"},{"state":"success","active":true,"login":"new"}]}}"#
            ),
            Login::In("new".into())
        );
        assert_eq!(
            parse(
                r#"{"hosts":{"ghe.example":[{"state":"success","active":true,"login":"octo"}]}}"#
            ),
            Login::Out
        );
    }

    #[test]
    fn an_unchecked_github_account_reads_unreachable() {
        let parse = |s: &str| active_login(serde_json::from_str(s).unwrap());
        assert_eq!(
            parse(r#"{"hosts":{"github.com":[{"state":"timeout","active":true,"login":"octo"}]}}"#),
            Login::Unreachable
        );
        assert_eq!(
            parse(
                r#"{"hosts":{"github.com":[{"state":"error","error":"Get \"https://api.github.com/\": dial tcp: lookup api.github.com: no such host","active":true,"login":"octo"}]}}"#
            ),
            Login::Unreachable
        );
        assert_eq!(
            parse(
                r#"{"hosts":{"github.com":[{"state":"error","error":"HTTP 401: Bad credentials","active":true,"login":"octo"}]}}"#
            ),
            Login::Out
        );
    }

    #[tokio::test]
    async fn an_unreachable_github_reads_signed_out_saying_so() {
        let dir = tempfile::tempdir().unwrap();
        let (gh, _) = fake_gh(
            dir.path(),
            "\"auth status --json hosts\") echo 'error connecting to api.github.com' >&2; exit 1 ;;",
        );
        let auth = quick();
        assert_eq!(
            auth.status(Some(&gh), &cwd(dir.path())).await,
            GithubAuth::signed_out(Some(UNREACHABLE.to_string()))
        );
    }

    #[tokio::test]
    async fn no_account_reads_plain_signed_out() {
        let dir = tempfile::tempdir().unwrap();
        let (gh, _) = fake_gh(
            dir.path(),
            "\"auth status --json hosts\") echo 'You are not logged into any GitHub hosts. To log in, run: gh auth login' >&2; exit 1 ;;",
        );
        let auth = quick();
        assert_eq!(
            auth.status(Some(&gh), &cwd(dir.path())).await,
            GithubAuth::signed_out(None)
        );
    }

    #[test]
    fn failures_are_zwiim_sentences() {
        assert_eq!(failure("error: access_denied"), DENIED);
        assert_eq!(failure("the device code expired"), EXPIRED);
        assert_eq!(
            failure("dial tcp: lookup github.com: could not resolve host"),
            UNREACHABLE
        );
        assert_eq!(failure("something odd"), FAILED);
    }

    #[tokio::test]
    async fn gh_config_dir_is_never_set() {
        let dir = tempfile::tempdir().unwrap();
        let login = format!(
            "{}; exec sleep 30",
            prompt_lines("https://github.com/login/device")
        );
        let (gh, _) = gh(dir.path(), &login);
        let auth = quick();
        auth.sign_in(Some(&gh), &cwd(dir.path())).await;
        auth.cancel(Some(&gh), &cwd(dir.path())).await;
        let seen = std::fs::read_to_string(dir.path().join("env")).unwrap();
        let inherited = std::env::var("GH_CONFIG_DIR").unwrap_or_else(|_| "unset".into());
        assert_eq!(seen.trim(), format!("config={inherited}"));
        let source = include_str!("gh_auth.rs");
        let code = source.split("#[cfg(all(test, unix))]").next().unwrap();
        assert!(!code.contains("GH_CONFIG_DIR"));
    }
}
