// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: launching one engine session and holding it to its limits
//! (automation spec 5.6, 6.2.1, 7.1.4, 7.1.6, 10.5).
//!
//! [`run_session`] starts a locked-down session, gives it its lane tabs,
//! sends the prompt, and waits for the first of: its result, the end of its
//! turn, its wall clock, its tool-call limit. Then it ends the session. The
//! agent launch itself sits behind [`EngineLauncher`], so the loop is tested
//! without an agent; [`AcpEngineLauncher`] is the Host's.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::automation::item_runner::SessionLimits;
use crate::automation::lockdown::{EngineLaunch, ItemAttempt, SessionKind};
use crate::automation::sessions::{EngineScope, EngineSessions, SessionReport};
use crate::models::agent::AgentType;

/// One session to run.
#[derive(Debug, Clone)]
pub struct SessionRequest {
    pub kind: SessionKind,
    pub agent: AgentType,
    /// The item attempt an item session works (spec 6.6.7).
    pub attempt: Option<ItemAttempt>,
    pub prompt: String,
    /// The lane tabs it may use; `None` for the batch kinds.
    pub scope: Option<EngineScope>,
    pub limits: SessionLimits,
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// It returned its result.
    Reported(SessionReport),
    /// Its turn ended with no result.
    NoResult,
    /// It ran past its wall clock (`timeout`).
    TimedOut,
    /// It made more tool calls than its limit (`agent_limit`).
    ToolLimit,
    /// The agent could not be started or prompted.
    LaunchFailed(String),
}

/// Starts and ends engine agents.
#[async_trait]
pub trait EngineLauncher: Send + Sync {
    /// Start a locked-down agent; its connection id.
    async fn spawn(&self, agent: AgentType, launch: EngineLaunch) -> Result<String, String>;
    /// Send the session its one prompt.
    async fn prompt(&self, connection_id: &str, text: String) -> Result<(), String>;
    /// Resolve once the prompt's turn has ended (or the agent is gone).
    async fn turn_ended(&self, connection_id: &str);
    /// Stop the agent.
    async fn end(&self, connection_id: &str);
}

/// Run one session to its end.
pub async fn run_session(
    launcher: &dyn EngineLauncher,
    sessions: &EngineSessions,
    request: SessionRequest,
) -> SessionEnd {
    let launch = EngineLaunch {
        kind: request.kind,
        attempt: request.attempt,
    };
    let connection_id = match launcher.spawn(request.agent, launch).await {
        Ok(id) => id,
        Err(e) => return SessionEnd::LaunchFailed(e),
    };
    // Tracked before the prompt goes, so its first call is already counted
    // and already scoped.
    let mut watch = sessions.open(&connection_id, request.kind);
    if let Some(scope) = request.scope {
        sessions.set_scope(&connection_id, scope);
    }
    let end = match launcher.prompt(&connection_id, request.prompt).await {
        Err(e) => SessionEnd::LaunchFailed(e),
        Ok(()) => {
            let clock = Duration::from_secs(u64::from(request.limits.minutes) * 60);
            let limit = request.limits.tool_calls;
            tokio::select! {
                // A result that came with the turn's end is still a result.
                biased;
                report = &mut watch.report => match report {
                    Ok(report) => SessionEnd::Reported(report),
                    Err(_) => SessionEnd::NoResult,
                },
                _ = watch.calls.wait_for(|n| *n > limit) => SessionEnd::ToolLimit,
                _ = launcher.turn_ended(&connection_id) => match watch.report.try_recv() {
                    Ok(report) => SessionEnd::Reported(report),
                    Err(_) => SessionEnd::NoResult,
                },
                _ = tokio::time::sleep(clock) => SessionEnd::TimedOut,
            }
        }
    };
    sessions.close(&connection_id);
    launcher.end(&connection_id).await;
    end
}

/// The Host's launcher: the ACP runtime, with every session's conversation
/// in the hidden engine folder (spec 10.5.1).
pub struct AcpEngineLauncher {
    pub state: Arc<crate::app_state::AppState>,
}

impl AcpEngineLauncher {
    /// Where engine sessions run: a Host-owned folder, not a workspace.
    fn working_dir(&self) -> std::path::PathBuf {
        self.state.data_dir.join("automation").join("sessions")
    }
}

#[async_trait]
impl EngineLauncher for AcpEngineLauncher {
    async fn spawn(&self, agent: AgentType, launch: EngineLaunch) -> Result<String, String> {
        let dir = self.working_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        crate::commands::acp::verify_agent_installed(agent)
            .await
            .map_err(|e| e.to_string())?;
        let env = crate::commands::acp::build_session_runtime_env(
            &self.state.db,
            agent,
            None,
            &self.state.data_dir,
        )
        .await
        .map_err(|e| e.to_string())?;
        self.state
            .connection_manager
            .spawn_engine_agent(
                agent,
                dir.to_string_lossy().to_string(),
                env,
                self.state.emitter.clone(),
                launch,
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn prompt(&self, connection_id: &str, text: String) -> Result<(), String> {
        let dir = self.working_dir();
        let folder = crate::db::service::folder_service::engine_folder(
            &self.state.db.conn,
            &dir.to_string_lossy(),
        )
        .await
        .map_err(|e| e.to_string())?;
        self.state
            .connection_manager
            .send_prompt_linked(
                &self.state.db,
                connection_id,
                vec![crate::acp::types::PromptInputBlock::Text { text }],
                Some(folder),
                None,
                None,
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn turn_ended(&self, connection_id: &str) {
        loop {
            match self.state.connection_manager.get_state(connection_id).await {
                Some(state) if state.read().await.turn_in_flight => {}
                _ => return,
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn end(&self, connection_id: &str) {
        let _ = self
            .state
            .connection_manager
            .disconnect(connection_id)
            .await;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::automation::sessions::{ItemReport, ItemReportStatus};
    use crate::browser::store::TabOwner;
    use serde_json::json;
    use std::sync::Mutex;

    /// What the fake agent does once prompted.
    #[derive(Clone)]
    pub(crate) enum Script {
        /// Make `calls` tool calls, then return this result (if any) and end
        /// its turn.
        Act {
            calls: u32,
            result: Option<serde_json::Value>,
        },
        /// Never end its turn.
        Hang,
    }

    /// A launcher whose agents follow a script.
    pub(crate) struct FakeLauncher {
        pub sessions: Arc<EngineSessions>,
        pub scripts: Mutex<Vec<Script>>,
        pub fail_spawn: bool,
        /// Each session's (kind, prompt, scope at prompt time).
        pub prompted: Mutex<Vec<(SessionKind, String, Option<EngineScope>)>>,
        pub ended: Mutex<Vec<String>>,
        kinds: Mutex<std::collections::HashMap<String, SessionKind>>,
        done: Mutex<std::collections::HashSet<String>>,
        next: Mutex<u32>,
    }

    impl FakeLauncher {
        pub(crate) fn new(sessions: Arc<EngineSessions>, scripts: Vec<Script>) -> Self {
            Self {
                sessions,
                scripts: Mutex::new(scripts),
                fail_spawn: false,
                prompted: Default::default(),
                ended: Default::default(),
                kinds: Default::default(),
                done: Default::default(),
                next: Mutex::new(0),
            }
        }
    }

    #[async_trait]
    impl EngineLauncher for FakeLauncher {
        async fn spawn(&self, _agent: AgentType, launch: EngineLaunch) -> Result<String, String> {
            if self.fail_spawn {
                return Err("agent not installed".into());
            }
            let mut next = self.next.lock().unwrap();
            *next += 1;
            let id = format!("conn-{next}");
            self.kinds.lock().unwrap().insert(id.clone(), launch.kind);
            Ok(id)
        }

        async fn prompt(&self, connection_id: &str, text: String) -> Result<(), String> {
            let kind = self.kinds.lock().unwrap()[connection_id];
            self.prompted
                .lock()
                .unwrap()
                .push((kind, text, self.sessions.scope(connection_id)));
            let script = {
                let mut scripts = self.scripts.lock().unwrap();
                if scripts.is_empty() {
                    Script::Act {
                        calls: 0,
                        result: None,
                    }
                } else {
                    scripts.remove(0)
                }
            };
            if let Script::Act { calls, result } = script {
                for _ in 0..calls {
                    self.sessions.tool_called(connection_id);
                }
                if let Some(result) = result {
                    self.sessions.tool_called(connection_id);
                    let _ = self.sessions.report(connection_id, &result);
                }
                self.done.lock().unwrap().insert(connection_id.to_string());
            }
            Ok(())
        }

        async fn turn_ended(&self, connection_id: &str) {
            if self.done.lock().unwrap().contains(connection_id) {
                return;
            }
            std::future::pending::<()>().await
        }

        async fn end(&self, connection_id: &str) {
            self.ended.lock().unwrap().push(connection_id.to_string());
        }
    }

    fn request(kind: SessionKind) -> SessionRequest {
        SessionRequest {
            kind,
            agent: AgentType::ClaudeCode,
            attempt: None,
            prompt: "work the item".into(),
            scope: None,
            limits: SessionLimits {
                minutes: 15,
                tool_calls: 40,
            },
        }
    }

    #[tokio::test]
    async fn a_session_ends_with_its_result_and_is_stopped() {
        let sessions = Arc::new(EngineSessions::default());
        let launcher = FakeLauncher::new(
            sessions.clone(),
            vec![Script::Act {
                calls: 3,
                result: Some(json!({ "status": "skipped", "rule": "no relocation" })),
            }],
        );
        let scope = EngineScope {
            lane: TabOwner::Lane {
                site: "jobs.test".into(),
                account: "main".into(),
            },
            tabs: vec!["t1".into()],
            files: Vec::new(),
        };
        let end = run_session(
            &launcher,
            &sessions,
            SessionRequest {
                scope: Some(scope.clone()),
                ..request(SessionKind::Item)
            },
        )
        .await;
        assert_eq!(
            end,
            SessionEnd::Reported(SessionReport::Item(ItemReport {
                status: ItemReportStatus::Skipped {
                    rule: "no relocation".into()
                },
                tips: vec![],
            }))
        );
        // The scope was in place before the prompt; the session is gone after.
        let prompted = launcher.prompted.lock().unwrap().clone();
        assert_eq!(prompted[0].2, Some(scope));
        assert_eq!(launcher.ended.lock().unwrap().as_slice(), &["conn-1"]);
        assert_eq!(sessions.scope("conn-1"), None);
    }

    #[tokio::test]
    async fn a_turn_without_a_result_and_too_many_calls_end_the_session() {
        let sessions = Arc::new(EngineSessions::default());
        let launcher = FakeLauncher::new(
            sessions.clone(),
            vec![
                Script::Act {
                    calls: 2,
                    result: None,
                },
                Script::Act {
                    calls: 41,
                    result: None,
                },
            ],
        );
        let quiet = run_session(&launcher, &sessions, request(SessionKind::Item)).await;
        assert_eq!(quiet, SessionEnd::NoResult);
        let busy = run_session(&launcher, &sessions, request(SessionKind::Item)).await;
        assert_eq!(busy, SessionEnd::ToolLimit);
        assert_eq!(launcher.ended.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_past_its_clock_times_out() {
        let sessions = Arc::new(EngineSessions::default());
        let launcher = FakeLauncher::new(sessions.clone(), vec![Script::Hang]);
        let end = run_session(&launcher, &sessions, request(SessionKind::Discovery)).await;
        assert_eq!(end, SessionEnd::TimedOut);
        assert_eq!(launcher.ended.lock().unwrap().as_slice(), &["conn-1"]);
    }

    #[tokio::test]
    async fn an_agent_that_cannot_start_runs_nothing() {
        let sessions = Arc::new(EngineSessions::default());
        let mut launcher = FakeLauncher::new(sessions.clone(), vec![]);
        launcher.fail_spawn = true;
        let end = run_session(&launcher, &sessions, request(SessionKind::Item)).await;
        assert_eq!(end, SessionEnd::LaunchFailed("agent not installed".into()));
        assert!(launcher.prompted.lock().unwrap().is_empty());
        assert!(launcher.ended.lock().unwrap().is_empty());
    }
}
