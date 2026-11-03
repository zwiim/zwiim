//! Zwiim-owned: carrying on a saved plan in a new session, with any agent.
//!
//! Each turn's plan is saved in the session's worktree (`session_plans`), so
//! it does not belong to the agent that made it. When a plan was left with
//! steps to do, `session.continue_plan` starts a new session in the same
//! workspace, with the agent the user picks (the same one by default), whose
//! first prompt lists the plan's steps and asks it to finish them. The new
//! session records the plan it continues; the old plan stays as it was.

use std::path::Path;

use crate::acp::manager::ConnectionManager;
use crate::acp::progress::StepStatus;
use crate::acp::types::PromptInputBlock;
use crate::attachments;
use crate::db::entities::conversation;
use crate::db::service::conversation_service;
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::message::MessageTurn;
use crate::protocol::error::ZwiimError;
use crate::protocol::events::relativize;
use crate::session::{resolve, session_lock, start_tracked, AgentLauncher};
use crate::session_options::OptionsRequest;
use crate::session_plans::{self, ContinuedIn, PlanLink, PlanRecord, PlanState};
use crate::session_retry::{carried_options, refuse_turn_in_progress};
use crate::session_summary::{self, SummaryKind};

/// The plan's file name, as the new session's attachment. Users and agents
/// read it as the agent's tasks, so the agent's own plan mode is never implied.
pub(crate) const PLAN_FILE: &str = "tasks.md";

/// The new session and the plan it continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continued {
    pub session_id: String,
    pub plan_id: String,
}

/// Start a new session in session `session_id`'s workspace, with `agent`
/// (the session's own agent when `None`), to finish the session's newest
/// plan. The same agent keeps the session's model, effort and mode; another
/// starts on its defaults. Refused with `turn_in_progress` while a turn
/// runs, `nothing_to_continue` without a plan that can be resumed (see
/// `session_plans::is_resumable`; `history` is the session's turns), and
/// `root_workspace` where no new session can start.
pub async fn continue_plan<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    session_id: &str,
    agent: Option<AgentType>,
    history: &[MessageTurn],
) -> Result<Continued, ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    // With no turn running, a plan still saved as running was cut short.
    refuse_turn_in_progress(connections, &row).await?;
    let workspace_id = crate::workspace::workspace_id_of(&db.conn, row.folder_id)
        .await?
        .ok_or_else(|| ZwiimError::not_found("Session has no workspace"))?;
    let folder = crate::workspace::resolve(&db.conn, &workspace_id).await?;
    let root = Path::new(&folder.path);
    let mut plan = session_plans::latest(root, session_id)
        .await
        .ok_or_else(ZwiimError::nothing_to_continue)?;
    if plan.state == PlanState::Running {
        plan.state = PlanState::Interrupted;
    }
    let unreachable = conversation_service::unreachable_turn(&db.conn, row.id)
        .await
        .map_err(crate::app_error::AppCommandError::from)?
        .is_some();
    let continued = session_plans::continued_in(&db.conn, root, session_id, &plan.plan_id)
        .await
        .is_some();
    if !session_plans::is_resumable(&plan, history, unreachable, continued) {
        return Err(ZwiimError::nothing_to_continue());
    }
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    let own: AgentType = serde_json::from_value(serde_json::Value::String(row.agent_type.clone()))
        .map_err(|_| ZwiimError::internal())?;
    let agent = agent.unwrap_or(own);
    let options = if agent == own {
        carried_options(db, connections, &row, session_id).await
    } else {
        OptionsRequest::default()
    };

    let new_session_id = hand_over(
        db,
        connections,
        launcher,
        &row,
        &workspace_id,
        root,
        &plan,
        history,
        agent,
        &options,
        Vec::new(),
    )
    .await?;
    Ok(Continued {
        session_id: new_session_id,
        plan_id: plan.plan_id,
    })
}

/// Start a new session in `workspace_id` with `agent` whose first prompt is
/// `failed` (a message the old session could not send; none when a plan is
/// continued) followed by `plan`'s steps, the old session's last reply and
/// two attachments: `tasks.md` and `previous-session.md` (a record of
/// `history`). The new session's plan link, the old session's forward link
/// and the new session's title are then saved; none of those can fail the
/// call. Returns the new `session_id`.
///
/// The caller holds session `row`'s lock.
pub(crate) async fn hand_over<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    row: &conversation::Model,
    workspace_id: &str,
    root: &Path,
    plan: &PlanRecord,
    history: &[MessageTurn],
    agent: AgentType,
    options: &OptionsRequest,
    failed: Vec<PromptInputBlock>,
) -> Result<String, ZwiimError> {
    let own: AgentType = serde_json::from_value(serde_json::Value::String(row.agent_type.clone()))
        .map_err(|_| ZwiimError::internal())?;
    let session_id = row.session_id.as_deref().ok_or_else(ZwiimError::internal)?;

    let plan_path = store(root, PLAN_FILE, plan_file(plan, root).as_bytes()).await?;
    let previous = session_summary::build(
        &session_summary::turns_from_history(history),
        &own.to_string(),
        &failed,
        SummaryKind::PlanStopped,
    );
    let previous_path = match store(
        root,
        session_summary::PREVIOUS_SESSION_FILE,
        previous.as_bytes(),
    )
    .await
    {
        Ok(path) => path,
        Err(e) => {
            remove_attachments(&[plan_path]).await;
            return Err(e);
        }
    };
    let paths = [plan_path, previous_path];

    let mut text = prompt(plan, root);
    if let Some(reply) = session_summary::last_reply(history) {
        text.push_str("\n\nTheir last reply:\n\n");
        text.push_str(&quote(&session_summary::capped(&reply)));
    }
    text.push_str("\n\n");
    text.push_str(&attachments::note(&paths));
    let mut blocks = failed;
    blocks.push(PromptInputBlock::Text { text });

    let mut dispatched = false;
    let started = start_tracked(
        db,
        connections,
        launcher,
        workspace_id,
        agent,
        blocks,
        None,
        options,
        &mut dispatched,
    )
    .await;
    let new_session_id = match started {
        Ok(id) => id,
        Err(e) => {
            if !dispatched {
                // Nothing will read the files: take them back out (best
                // effort). Once the first turn is dispatched it names them.
                remove_attachments(&paths).await;
            }
            return Err(e);
        }
    };

    let link = PlanLink {
        session_id: session_id.to_string(),
        plan_id: plan.plan_id.clone(),
        agent: Some(own),
        completed: plan
            .steps
            .iter()
            .filter(|step| step.status == StepStatus::Completed)
            .count(),
        total: plan.steps.len(),
        title: row.title.clone(),
    };
    if let Err(e) = session_plans::save_link(root, &new_session_id, &link).await {
        tracing::warn!(
            "[plans] could not record that {new_session_id} continues {}: {e}",
            plan.plan_id
        );
    }
    let forward = ContinuedIn {
        session_id: new_session_id.clone(),
        plan_id: plan.plan_id.clone(),
    };
    if let Err(e) = session_plans::save_continued_in(root, session_id, &forward).await {
        tracing::warn!(
            "[plans] could not record that {new_session_id} took over {}: {e}",
            plan.plan_id
        );
    }
    if let Some(title) = &row.title {
        match resolve(&db.conn, &new_session_id).await {
            Ok(new_row) => {
                if let Err(e) = conversation_service::update_title(
                    &db.conn,
                    new_row.id,
                    format!("{title} (continued)"),
                )
                .await
                {
                    tracing::warn!("[plans] could not title {new_session_id}: {e}");
                }
            }
            Err(e) => tracing::warn!("[plans] could not find {new_session_id} to title it: {e}"),
        }
    }
    if let Err(e) = conversation_service::touch(&db.conn, row.id).await {
        tracing::warn!("[plans] could not bump {session_id}: {e}");
    }
    Ok(new_session_id)
}

/// Save `bytes` as attachment `name` in `root`, and where it is.
async fn store(root: &Path, name: &str, bytes: &[u8]) -> Result<std::path::PathBuf, ZwiimError> {
    let stored = attachments::store(root, name, bytes).await?;
    attachments::resolve(root, &stored.attachment_id)
}

/// Take attachments back out (best effort).
async fn remove_attachments(paths: &[std::path::PathBuf]) {
    for dir in paths.iter().filter_map(|path| path.parent()) {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}

/// `text` as a Markdown quote.
fn quote(text: &str) -> String {
    text.lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `tasks.md`: the plan's steps and where it stopped, as the new session's
/// agent reads them. Step titles are workspace-relative, like the live
/// `progress` event's.
fn plan_file(plan: &PlanRecord, root: &Path) -> String {
    let stopped_at = plan
        .steps
        .iter()
        .position(|step| step.status != StepStatus::Completed)
        .map_or(plan.steps.len(), |i| i + 1);
    format!(
        "Tasks from {}, stopped at task {stopped_at}\n\n{}\n",
        plan.agent,
        step_lines(plan, root.to_str()).join("\n"),
    )
}

/// One line per step of `plan`: `N. [done|in progress|to do] title`, with
/// paths under `root` made relative to it.
pub(crate) fn step_lines(plan: &PlanRecord, root: Option<&str>) -> Vec<String> {
    plan.steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let mark = match step.status {
                StepStatus::Completed => "done",
                StepStatus::InProgress => "in progress",
                StepStatus::Pending => "to do",
            };
            // One line per step.
            let title = relativize(&step.title, root);
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("{}. [{mark}] {title}", i + 1)
        })
        .collect()
}

/// The new session's first prompt: `plan`'s steps as its agent left them,
/// with step titles relative to the workspace `root`.
fn prompt(plan: &PlanRecord, root: &Path) -> String {
    format!(
        "Continue the tasks that {agent} started in this workspace and did not finish. \
         The tasks, as {agent} left them:\n\n{steps}\n\n\
         The changes from the finished tasks are already in the workspace. Check how far \
         the task in progress got, then carry out the tasks not done. Keep your task list \
         up to date as you go.",
        agent = plan.agent,
        steps = step_lines(plan, root.to_str()).join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::progress::ProgressStep;
    use crate::session::orchestration_tests::{finish_turn, settle, text, workspace, FakeLauncher};
    use crate::session::start;
    use crate::session_plans::PlanState;

    async fn fresh_db() -> AppDatabase {
        crate::db::test_helpers::fresh_in_memory_db().await
    }

    /// A Claude Code session whose first turn has ended, on `fake-conn-1`.
    async fn idle_session(
        db: &AppDatabase,
        mgr: &ConnectionManager,
        launcher: &FakeLauncher,
    ) -> String {
        let wid = workspace(db).await;
        let sid = start(
            db,
            mgr,
            launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        finish_turn(mgr, "fake-conn-1").await;
        sid
    }

    fn plan(sid: &str, statuses: &[StepStatus]) -> PlanRecord {
        PlanRecord {
            v: 1,
            plan_id: uuid::Uuid::new_v4().to_string(),
            session_id: sid.to_string(),
            agent: AgentType::ClaudeCode,
            revision: 2,
            state: PlanState::Interrupted,
            steps: statuses
                .iter()
                .enumerate()
                .map(|(i, status)| ProgressStep {
                    title: format!("step {}", i + 1),
                    status: *status,
                })
                .collect(),
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    async fn root(db: &AppDatabase, sid: &str) -> std::path::PathBuf {
        crate::session::workspace_dir(&db.conn, sid)
            .await
            .unwrap()
            .into()
    }

    #[tokio::test]
    async fn another_agent_continues_the_plan_in_a_new_session() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        let root = root(&db, &sid).await;
        let saved = plan(
            &sid,
            &[
                StepStatus::Completed,
                StepStatus::InProgress,
                StepStatus::Pending,
            ],
        );
        session_plans::save(&root, &saved).await.unwrap();

        let continued = continue_plan(&db, &mgr, &launcher, &sid, Some(AgentType::Codex), &[])
            .await
            .unwrap();
        settle().await;

        assert_eq!(continued.plan_id, saved.plan_id);
        assert_ne!(continued.session_id, sid);
        let new_row = resolve(&db.conn, &continued.session_id).await.unwrap();
        assert_eq!(new_row.agent_type, "codex");
        assert_eq!(
            crate::session::workspace_dir(&db.conn, &continued.session_id).await,
            Some(root.to_string_lossy().into_owned()),
            "same workspace"
        );
        let sent = launcher.prompts_for("fake-conn-2");
        assert_eq!(sent.len(), 1);
        assert!(sent[0].starts_with("Continue the tasks that Claude Code started"));
        assert!(sent[0].contains("1. [done] step 1\n2. [in progress] step 2\n3. [to do] step 3"));
        assert_eq!(
            session_plans::link(&root, &continued.session_id).await,
            Some(PlanLink {
                session_id: sid.clone(),
                plan_id: saved.plan_id.clone(),
                agent: Some(AgentType::ClaudeCode),
                completed: 1,
                total: 3,
                title: Some("hi".into()),
            })
        );
        // The old plan is untouched.
        assert_eq!(session_plans::latest(&root, &sid).await, Some(saved));
    }

    #[tokio::test]
    async fn defaults_to_the_sessions_own_agent() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        let root = root(&db, &sid).await;
        session_plans::save(&root, &plan(&sid, &[StepStatus::Pending]))
            .await
            .unwrap();

        let continued = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap();
        let new_row = resolve(&db.conn, &continued.session_id).await.unwrap();
        assert_eq!(new_row.agent_type, "claude_code");
    }

    #[tokio::test]
    async fn refuses_without_an_unfinished_plan() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;

        let err = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap_err();
        assert_eq!(err.code, "nothing_to_continue");

        let root = root(&db, &sid).await;
        session_plans::save(&root, &plan(&sid, &[StepStatus::Completed]))
            .await
            .unwrap();
        let err = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap_err();
        assert_eq!(err.code, "nothing_to_continue");
        assert_eq!(err.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            launcher.launches.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn refuses_while_a_turn_runs() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let wid = workspace(&db).await;
        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::ClaudeCode,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        let root = root(&db, &sid).await;
        let mut running = plan(&sid, &[StepStatus::InProgress]);
        running.state = PlanState::Running;
        session_plans::save(&root, &running).await.unwrap();

        let err = continue_plan(&db, &mgr, &launcher, &sid, Some(AgentType::Codex), &[])
            .await
            .unwrap_err();
        assert_eq!(err.code, "turn_in_progress");
        assert_eq!(
            launcher.launches.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn continue_refuses_plan_with_later_user_turn() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        let root = root(&db, &sid).await;
        session_plans::save(&root, &plan(&sid, &[StepStatus::Pending]))
            .await
            .unwrap();

        // The user wrote after the plan stopped: the chat moved on.
        let history = crate::session::orchestration_tests::transcript(&["go on"]);
        let err = continue_plan(&db, &mgr, &launcher, &sid, None, &history)
            .await
            .unwrap_err();
        assert_eq!(err.code, "nothing_to_continue");
        assert_eq!(
            launcher.launches.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn continue_refuses_unreachable_session() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        let root = root(&db, &sid).await;
        session_plans::save(&root, &plan(&sid, &[StepStatus::Pending]))
            .await
            .unwrap();
        let row = resolve(&db.conn, &sid).await.unwrap();
        crate::db::service::conversation_service::set_unreachable_turn(
            &db.conn,
            row.id,
            Some(&crate::session_retry::UnreachableTurn {
                message: "Can't reach the agent's server.".into(),
                blocks: Vec::new(),
            }),
        )
        .await
        .unwrap();

        let err = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap_err();
        assert_eq!(err.code, "nothing_to_continue");
    }

    #[test]
    fn prompt_keeps_each_step_on_one_line() {
        let mut record = plan("s", &[StepStatus::Pending]);
        record.steps[0].title = "fix\nthe   parser".to_string();
        assert!(prompt(&record, Path::new("/w")).contains("1. [to do] fix the parser\n"));
    }

    #[test]
    fn prompt_shows_step_paths_relative_to_the_workspace() {
        let mut record = plan("s", &[StepStatus::Pending]);
        record.steps[0].title = "edit /w/tree/src/a.rs".to_string();
        let text = prompt(&record, Path::new("/w/tree"));
        assert!(text.contains("1. [to do] edit src/a.rs\n"), "{text}");
        assert!(!text.contains("/w/tree"), "{text}");
    }

    /// The old chat: one user message, answered by `reply`. Both turns are
    /// older than any test plan, so the plan can still be continued.
    fn old_chat(reply: &str) -> Vec<MessageTurn> {
        use crate::models::message::{ContentBlock, TurnRole};
        let turn = |id: &str, role: TurnRole, text: &str| MessageTurn {
            id: id.into(),
            role,
            blocks: vec![ContentBlock::Text { text: text.into() }],
            timestamp: chrono::DateTime::from_timestamp_millis(1).unwrap(),
            usage: None,
            duration_ms: None,
            model: None,
            completed_at: None,
            agent_message_id: None,
        };
        vec![
            turn("t0", TurnRole::User, "fix the leak"),
            turn("t1", TurnRole::Assistant, reply),
        ]
    }

    /// The files named by the attachment note that ends `prompt`.
    fn attached(prompt: &str) -> Vec<std::path::PathBuf> {
        let note = prompt
            .split(crate::attachments::NOTE_OPEN)
            .nth(1)
            .expect("attachment note");
        note.lines()
            .filter_map(|l| l.strip_prefix("- "))
            .map(std::path::PathBuf::from)
            .collect()
    }

    /// An idle session holding an interrupted plan of `steps`.
    async fn stopped(
        steps: &[(&str, StepStatus)],
    ) -> (AppDatabase, ConnectionManager, FakeLauncher, String, PlanRecord) {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        let mut saved = plan(&sid, &[]);
        saved.steps = steps
            .iter()
            .map(|(title, status)| ProgressStep {
                title: title.to_string(),
                status: *status,
            })
            .collect();
        session_plans::save(&root(&db, &sid).await, &saved)
            .await
            .unwrap();
        (db, mgr, launcher, sid, saved)
    }

    const THREE: [(&str, StepStatus); 3] = [
        ("Find the drop", StepStatus::Completed),
        ("Re-arm", StepStatus::InProgress),
        ("Test", StepStatus::Pending),
    ];

    #[tokio::test]
    async fn continue_attaches_plan_and_previous_session() {
        let (db, mgr, launcher, sid, saved) = stopped(&THREE).await;
        let history = old_chat("Re-arming now.\nNot done yet.");
        let continued = continue_plan(&db, &mgr, &launcher, &sid, None, &history)
            .await
            .unwrap();
        settle().await;

        let sent = launcher.prompts_for("fake-conn-2");
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0].starts_with(&prompt(&saved, &root(&db, &sid).await)),
            "{}",
            sent[0]
        );
        assert!(
            sent[0].contains("\n\nTheir last reply:\n\n> Re-arming now.\n> Not done yet.\n\n"),
            "{}",
            sent[0]
        );
        let files = attached(&sent[0]);
        assert_eq!(files.len(), 2, "{}", sent[0]);
        assert!(files[0].ends_with("tasks.md"));
        assert!(files[1].ends_with("previous-session.md"));
        assert!(sent[0].ends_with(&crate::attachments::note(&files)), "{}", sent[0]);
        let previous = std::fs::read_to_string(&files[1]).unwrap();
        assert!(previous.contains("fix the leak"), "{previous}");
        assert!(previous.contains("The tasks are in tasks.md."), "{previous}");
        assert_ne!(continued.session_id, sid);
    }

    #[tokio::test]
    async fn plan_file_lists_steps() {
        let (db, mgr, launcher, sid, _) = stopped(&THREE).await;
        continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap();
        settle().await;
        let files = attached(&launcher.prompts_for("fake-conn-2")[0]);
        assert_eq!(
            std::fs::read_to_string(&files[0]).unwrap(),
            "Tasks from Claude Code, stopped at task 2\n\n1. [done] Find the drop\n2. [in progress] Re-arm\n3. [to do] Test\n"
        );
    }

    #[tokio::test]
    async fn plan_file_relativizes_step_titles() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        let dir = root(&db, &sid).await;
        let mut saved = plan(&sid, &[StepStatus::Pending]);
        saved.steps[0].title = format!("Edit {}/src/a.ts", dir.display());
        session_plans::save(&dir, &saved).await.unwrap();

        continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap();
        settle().await;
        let files = attached(&launcher.prompts_for("fake-conn-2")[0]);
        let text = std::fs::read_to_string(&files[0]).unwrap();
        assert!(text.contains("1. [to do] Edit src/a.ts\n"), "{text}");
        assert!(!text.contains(dir.to_str().unwrap()), "{text}");
    }

    #[tokio::test]
    async fn prompt_caps_last_reply() {
        let (db, mgr, launcher, sid, _) = stopped(&THREE).await;
        let history = old_chat(&"x".repeat(30_000));
        continue_plan(&db, &mgr, &launcher, &sid, None, &history)
            .await
            .unwrap();
        settle().await;
        let sent = &launcher.prompts_for("fake-conn-2")[0];
        assert!(sent.contains("… [cut]"), "cut mark");
        let longest = sent
            .split(|c| c != 'x')
            .map(|run| run.chars().count())
            .max()
            .unwrap();
        assert_eq!(longest, 10_000);
    }

    #[tokio::test]
    async fn continue_links_both_sessions() {
        let (db, mgr, launcher, sid, saved) = stopped(&THREE).await;
        let old = resolve(&db.conn, &sid).await.unwrap();
        conversation_service::update_title(&db.conn, old.id, "Fix the leak".into())
            .await
            .unwrap();
        let continued = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap();

        let dir = root(&db, &sid).await;
        assert_eq!(
            session_plans::link(&dir, &continued.session_id).await,
            Some(PlanLink {
                session_id: sid.clone(),
                plan_id: saved.plan_id.clone(),
                agent: Some(AgentType::ClaudeCode),
                completed: 1,
                total: 3,
                title: Some("Fix the leak".into()),
            })
        );
        assert_eq!(
            session_plans::continued_in(&db.conn, &dir, &sid, &saved.plan_id).await,
            Some(continued.session_id)
        );
    }

    #[tokio::test]
    async fn continue_titles_new_session() {
        let (db, mgr, launcher, sid, _) = stopped(&THREE).await;
        let old = resolve(&db.conn, &sid).await.unwrap();
        conversation_service::update_title(&db.conn, old.id, "Fix the leak".into())
            .await
            .unwrap();
        let continued = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap();
        let new_row = resolve(&db.conn, &continued.session_id).await.unwrap();
        assert_eq!(new_row.title.as_deref(), Some("Fix the leak (continued)"));
        assert!(new_row.title_locked);
    }

    #[tokio::test]
    async fn continue_bumps_the_old_session() {
        let (db, mgr, launcher, sid, _) = stopped(&THREE).await;
        let before = resolve(&db.conn, &sid).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap();
        let after = resolve(&db.conn, &sid).await.unwrap();
        assert!(after.updated_at > before.updated_at);
        assert_eq!(after.title, before.title);
        assert_eq!(after.status, before.status);
    }

    #[tokio::test]
    async fn second_continue_is_nothing_to_continue() {
        let (db, mgr, launcher, sid, _) = stopped(&THREE).await;
        let (a, b) = tokio::join!(
            continue_plan(&db, &mgr, &launcher, &sid, None, &[]),
            continue_plan(&db, &mgr, &launcher, &sid, None, &[]),
        );
        let (ok, err) = match (a, b) {
            (Ok(ok), Err(err)) | (Err(err), Ok(ok)) => (ok, err),
            other => panic!("expected exactly one Ok: {other:?}"),
        };
        assert_eq!(err.code, "nothing_to_continue");
        assert_ne!(ok.session_id, sid);
        // One session was started besides the original.
        assert_eq!(
            launcher.launches.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
    }

    #[tokio::test]
    async fn continue_after_deleting_new_session_works_again() {
        let (db, mgr, launcher, sid, _) = stopped(&THREE).await;
        let first = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap();
        let err = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap_err();
        assert_eq!(err.code, "nothing_to_continue");

        let new_row = resolve(&db.conn, &first.session_id).await.unwrap();
        conversation_service::soft_delete(&db.conn, new_row.id)
            .await
            .unwrap();
        let again = continue_plan(&db, &mgr, &launcher, &sid, None, &[])
            .await
            .unwrap();
        assert_ne!(again.session_id, first.session_id);
    }
}
