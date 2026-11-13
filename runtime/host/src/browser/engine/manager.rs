//! Keeping one engine running while the browser is in use (design 5.3, 5.5).
//!
//! - Started lazily, by the first caller of [`EngineManager::cdp`].
//! - Restarted on an unexpected exit after 1 s, 2 s, 4 s … at most 30 s. Five
//!   exits within ten minutes turn the browser off with `crash_loop` until
//!   someone asks for it again with [`EngineManager::reset`].
//! - [`EngineManager::stop`] ends it on purpose; nothing restarts it until
//!   the next caller.
//! - [`EngineManager::check`] (after a wake) restarts an engine that no
//!   longer answers.
//!
//! Every start bumps [`EngineState::Running::generation`], so the tab layer
//! knows to restore its tabs into the new engine.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::FutureExt;
use tokio::sync::{mpsc, watch, Mutex};
use tokio::time::Instant;

use super::cdp::Cdp;

const CRASH_WINDOW: Duration = Duration::from_secs(10 * 60);
const CRASH_LIMIT: usize = 5;
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// What clients are told about the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    Stopped,
    Starting,
    Running { version: String, generation: u64 },
    Failed { code: &'static str, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct EngineError {
    /// `download_offline`, `checksum_mismatch`, `disk_full`,
    /// `unsupported_platform`, `crash_loop`, `setup_failed`.
    pub code: &'static str,
    pub message: String,
}

/// A started engine. Dropping `exited` kills it.
pub struct Running {
    pub cdp: Cdp,
    pub version: String,
    /// Resolves when the engine process exits.
    pub exited: BoxFuture<'static, ()>,
}

pub type StartFn = Arc<dyn Fn() -> BoxFuture<'static, Result<Running, EngineError>> + Send + Sync>;

enum Control {
    /// Close it and do not restart.
    Stop,
    /// Kill it and restart, as after a crash.
    Restart,
}

struct Inner {
    cdp: Option<Cdp>,
    control: Option<mpsc::UnboundedSender<Control>>,
    exits: VecDeque<Instant>,
    generation: u64,
}

pub struct EngineManager {
    start: StartFn,
    inner: Mutex<Inner>,
    state: watch::Sender<EngineState>,
}

impl EngineManager {
    pub fn new(start: StartFn) -> Arc<Self> {
        Arc::new(EngineManager {
            start,
            inner: Mutex::new(Inner {
                cdp: None,
                control: None,
                exits: VecDeque::new(),
                generation: 0,
            }),
            state: watch::channel(EngineState::Stopped).0,
        })
    }

    pub fn state(&self) -> watch::Receiver<EngineState> {
        self.state.subscribe()
    }

    /// The running engine's connection, starting it first if needed.
    pub async fn cdp(self: &Arc<Self>) -> Result<Cdp, EngineError> {
        self.connect().await.map(|(cdp, _)| cdp)
    }

    /// Like [`Self::cdp`], with the generation the connection belongs to.
    pub async fn connect(self: &Arc<Self>) -> Result<(Cdp, u64), EngineError> {
        let mut inner = self.inner.lock().await;
        if let Some(cdp) = inner.cdp.as_ref().filter(|c| !c.is_closed()) {
            return Ok((cdp.clone(), inner.generation));
        }
        if let EngineState::Failed {
            code: "crash_loop",
            message,
        } = &*self.state.borrow()
        {
            return Err(EngineError {
                code: "crash_loop",
                message: message.clone(),
            });
        }
        let cdp = self.start_locked(&mut inner).await?;
        Ok((cdp, inner.generation))
    }

    /// The running engine's connection and generation, without starting one.
    pub async fn running(&self) -> Option<(Cdp, u64)> {
        let inner = self.inner.lock().await;
        let cdp = inner.cdp.as_ref().filter(|c| !c.is_closed())?;
        Some((cdp.clone(), inner.generation))
    }

    /// Allow starts again after `crash_loop`.
    pub async fn reset(&self) {
        let mut inner = self.inner.lock().await;
        inner.exits.clear();
        if matches!(*self.state.borrow(), EngineState::Failed { .. }) {
            self.state.send_replace(EngineState::Stopped);
        }
    }

    /// Close the engine. It starts again on the next [`Self::cdp`].
    pub async fn stop(&self) {
        let mut inner = self.inner.lock().await;
        inner.cdp = None;
        if let Some(control) = inner.control.take() {
            let _ = control.send(Control::Stop);
        }
        self.state.send_replace(EngineState::Stopped);
    }

    /// Restart a running engine that does not answer within 3 s.
    pub async fn check(&self) {
        let (cdp, control) = {
            let inner = self.inner.lock().await;
            match (&inner.cdp, &inner.control) {
                (Some(cdp), Some(control)) => (cdp.clone(), control.clone()),
                _ => return,
            }
        };
        let answer = cdp
            .call_with_timeout(
                "Browser.getVersion",
                serde_json::json!({}),
                None,
                HEALTH_TIMEOUT,
            )
            .await;
        if let Err(e) = answer {
            tracing::warn!(error = %e, "browser engine failed its health check; restarting");
            let _ = control.send(Control::Restart);
        }
    }

    async fn start_locked(self: &Arc<Self>, inner: &mut Inner) -> Result<Cdp, EngineError> {
        self.state.send_replace(EngineState::Starting);
        let running = match (self.start)().await {
            Ok(running) => running,
            Err(e) => {
                self.state.send_replace(EngineState::Failed {
                    code: e.code,
                    message: e.message.clone(),
                });
                return Err(e);
            }
        };
        inner.generation += 1;
        let cdp = running.cdp.clone();
        let (control, rx) = mpsc::unbounded_channel();
        inner.cdp = Some(cdp.clone());
        inner.control = Some(control.clone());
        self.state.send_replace(EngineState::Running {
            version: running.version.clone(),
            generation: inner.generation,
        });
        tokio::spawn(self.clone().watch(running, rx, control));
        Ok(cdp)
    }

    /// Wait for the engine to exit or be told to stop, then restart it if
    /// the exit was not asked for.
    fn watch(
        self: Arc<Self>,
        running: Running,
        mut rx: mpsc::UnboundedReceiver<Control>,
        me: mpsc::UnboundedSender<Control>,
    ) -> BoxFuture<'static, ()> {
        async move {
        let Running {
            cdp, mut exited, ..
        } = running;
        let unexpected = tokio::select! {
            _ = &mut exited => true,
            control = rx.recv() => match control {
                Some(Control::Restart) => true,
                Some(Control::Stop) | None => {
                    let close = cdp.call_with_timeout("Browser.close", serde_json::json!({}), None, CLOSE_GRACE);
                    let _ = tokio::time::timeout(CLOSE_GRACE, async { let _ = close.await; (&mut exited).await }).await;
                    false
                }
            },
        };
        // Kills the process if it is still there.
        drop(exited);
        if !unexpected {
            return;
        }

        let mut inner = self.inner.lock().await;
        // A newer engine or a stop has taken over since.
        if !inner.control.as_ref().is_some_and(|c| c.same_channel(&me)) {
            return;
        }
        inner.cdp = None;
        inner.control = None;
        let now = Instant::now();
        inner.exits.push_back(now);
        while inner
            .exits
            .front()
            .is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW)
        {
            inner.exits.pop_front();
        }
        let exits = inner.exits.len();
        if exits >= CRASH_LIMIT {
            tracing::error!(
                exits,
                "browser engine keeps exiting; turning the browser off"
            );
            self.state.send_replace(EngineState::Failed {
                code: "crash_loop",
                message: format!(
                    "The browser engine stopped {exits} times in ten minutes, so it was turned off."
                ),
            });
            return;
        }
        let backoff = Duration::from_secs(1 << (exits - 1)).min(MAX_BACKOFF);
        tracing::warn!(exits, ?backoff, "browser engine exited; restarting");
        self.state.send_replace(EngineState::Starting);
        // A stop during the wait cancels the restart: it clears the state.
        drop(inner);
        tokio::time::sleep(backoff).await;
        let mut inner = self.inner.lock().await;
        if *self.state.borrow() != EngineState::Starting || inner.cdp.is_some() {
            return;
        }
        let _ = self.start_locked(&mut inner).await;
        }
        .boxed()
    }
}

/// Wrap a starter so it can be stored in [`StartFn`].
pub fn start_fn<F, Fut>(f: F) -> StartFn
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Running, EngineError>> + Send + 'static,
{
    Arc::new(move || f().boxed())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    use serde_json::json;
    use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::oneshot;

    use super::*;

    /// Fake engines: each start answers CDP until the test makes it exit.
    #[derive(Default)]
    struct Fakes {
        starts: AtomicUsize,
        exits: StdMutex<Vec<oneshot::Sender<()>>>,
        /// When set, engines stop answering (but stay up).
        hung: std::sync::atomic::AtomicBool,
        fail_start: StdMutex<Option<EngineError>>,
    }

    impl Fakes {
        fn crash_latest(&self) {
            self.exits.lock().unwrap().pop().unwrap().send(()).unwrap();
        }
    }

    fn manager(fakes: Arc<Fakes>) -> Arc<EngineManager> {
        EngineManager::new(start_fn(move || {
            let fakes = fakes.clone();
            async move {
                if let Some(e) = fakes.fail_start.lock().unwrap().clone() {
                    return Err(e);
                }
                let n = fakes.starts.fetch_add(1, Ordering::SeqCst) + 1;
                let (to_engine, from_host) = duplex(1 << 16);
                let (mut to_host, from_engine) = duplex(1 << 16);
                let (exit_tx, exit_rx) = oneshot::channel();
                fakes.exits.lock().unwrap().push(exit_tx);
                let answering = fakes.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(from_host);
                    let mut buf = Vec::new();
                    while lines.read_until(0, &mut buf).await.unwrap_or(0) > 0 {
                        buf.pop();
                        let req: serde_json::Value = serde_json::from_slice(&buf).unwrap();
                        buf.clear();
                        if answering.hung.load(Ordering::SeqCst) {
                            continue;
                        }
                        let mut reply = serde_json::to_vec(
                            &json!({"id": req["id"], "result": {"product": format!("Fake/{n}")}}),
                        )
                        .unwrap();
                        reply.push(0);
                        if to_host.write_all(&reply).await.is_err() {
                            break;
                        }
                    }
                });
                Ok(Running {
                    cdp: Cdp::new(from_engine, to_engine),
                    version: format!("v{n}"),
                    exited: async move {
                        let _ = exit_rx.await;
                    }
                    .boxed(),
                })
            }
        }))
    }

    async fn running(m: &EngineManager) -> u64 {
        let mut rx = m.state();
        let state = rx
            .wait_for(|s| matches!(s, EngineState::Running { .. }))
            .await
            .unwrap()
            .clone();
        match state {
            EngineState::Running { generation, .. } => generation,
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn starts_lazily_and_once() {
        let fakes = Arc::new(Fakes::default());
        let m = manager(fakes.clone());
        assert_eq!(*m.state().borrow(), EngineState::Stopped);
        assert_eq!(fakes.starts.load(Ordering::SeqCst), 0);

        let (a, b) = tokio::join!(m.cdp(), m.cdp());
        a.unwrap();
        b.unwrap();
        assert_eq!(fakes.starts.load(Ordering::SeqCst), 1);
        assert_eq!(
            *m.state().borrow(),
            EngineState::Running {
                version: "v1".into(),
                generation: 1
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_crash_restarts_with_a_new_generation() {
        let fakes = Arc::new(Fakes::default());
        let m = manager(fakes.clone());
        m.cdp().await.unwrap();

        fakes.crash_latest();
        let mut rx = m.state();
        rx.wait_for(|s| *s == EngineState::Starting).await.unwrap();
        assert_eq!(running(&m).await, 2);
        assert_eq!(fakes.starts.load(Ordering::SeqCst), 2);
        let cdp = m.cdp().await.unwrap();
        let v = cdp
            .call("Browser.getVersion", json!({}), None)
            .await
            .unwrap();
        assert_eq!(v["product"], "Fake/2");
    }

    #[tokio::test(start_paused = true)]
    async fn five_crashes_in_ten_minutes_turn_it_off_until_reset() {
        let fakes = Arc::new(Fakes::default());
        let m = manager(fakes.clone());
        m.cdp().await.unwrap();
        for generation in 2..=5 {
            fakes.crash_latest();
            let mut rx = m.state();
            rx.wait_for(|s| *s == EngineState::Starting).await.unwrap();
            assert_eq!(running(&m).await, generation);
        }
        fakes.crash_latest();
        let mut rx = m.state();
        rx.wait_for(|s| matches!(s, EngineState::Failed { .. }))
            .await
            .unwrap();
        assert_eq!(m.cdp().await.unwrap_err().code, "crash_loop");
        assert_eq!(fakes.starts.load(Ordering::SeqCst), 5);

        m.reset().await;
        m.cdp().await.unwrap();
        assert_eq!(fakes.starts.load(Ordering::SeqCst), 6);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stop_does_not_restart() {
        let fakes = Arc::new(Fakes::default());
        let m = manager(fakes.clone());
        m.cdp().await.unwrap();
        m.stop().await;
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(*m.state().borrow(), EngineState::Stopped);
        assert_eq!(fakes.starts.load(Ordering::SeqCst), 1);

        m.cdp().await.unwrap();
        assert_eq!(fakes.starts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn an_engine_that_stops_answering_is_restarted() {
        let fakes = Arc::new(Fakes::default());
        let m = manager(fakes.clone());
        m.cdp().await.unwrap();
        m.check().await;
        assert_eq!(fakes.starts.load(Ordering::SeqCst), 1);

        fakes.hung.store(true, Ordering::SeqCst);
        m.check().await;
        fakes.hung.store(false, Ordering::SeqCst);
        let mut rx = m.state();
        rx.wait_for(|s| matches!(s, EngineState::Running { generation: 2, .. }))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_setup_failure_is_reported_and_retried_on_the_next_call() {
        let fakes = Arc::new(Fakes::default());
        *fakes.fail_start.lock().unwrap() = Some(EngineError {
            code: "download_offline",
            message: "offline".into(),
        });
        let m = manager(fakes.clone());
        assert_eq!(m.cdp().await.unwrap_err().code, "download_offline");
        assert!(matches!(
            *m.state().borrow(),
            EngineState::Failed {
                code: "download_offline",
                ..
            }
        ));

        *fakes.fail_start.lock().unwrap() = None;
        m.cdp().await.unwrap();
    }
}
