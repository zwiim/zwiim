//! Zwiim-owned: retrying a turn the agent couldn't send because it couldn't
//! reach its provider.
//!
//! When a turn ends that way the Host saves the error message and the failed
//! prompt's blocks on the session row (`UnreachableTurn`). `session.retry`
//! sends those blocks again, unchanged, in the same session; any prompt that
//! goes out clears the record. `session.retry_in_new_chat` sends them
//! instead as the first prompt of a new session, with a summary of the old
//! chat attached; the old session keeps its record.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::acp::manager::ConnectionManager;
use crate::acp::types::PromptInputBlock;
use crate::app_error::AppCommandError;
use crate::attachments;
use crate::db::entities::conversation;
use crate::db::service::conversation_service;
use crate::db::AppDatabase;
use crate::models::agent::AgentType;
use crate::models::message::MessageTurn;
use crate::protocol::error::ZwiimError;
use crate::session::{
    connection_for, resolve, send_locked, session_lock, start_tracked, AgentLauncher,
};
use crate::session_options::OptionsRequest;
use crate::session_summary;

/// A turn that failed because the agent couldn't reach its provider: the
/// message the user saw and the prompt's blocks, kept so Retry sends exactly
/// what failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnreachableTurn {
    pub message: String,
    pub blocks: Vec<PromptInputBlock>,
}

/// Session `session_id`'s saved can't-reach turn. `nothing_to_retry` when
/// there is none, or when it has no blocks to send.
pub async fn saved(db: &AppDatabase, session_id: &str) -> Result<UnreachableTurn, ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    saved_for(db, row.id).await
}

async fn saved_for(db: &AppDatabase, conversation_id: i32) -> Result<UnreachableTurn, ZwiimError> {
    conversation_service::unreachable_turn(&db.conn, conversation_id)
        .await
        .map_err(AppCommandError::from)?
        .filter(|turn| !turn.blocks.is_empty())
        .ok_or_else(ZwiimError::nothing_to_retry)
}

/// Send session `session_id`'s saved can't-reach turn again, in the same
/// session, resuming its agent if it is gone. Refused with `turn_in_progress`
/// while a turn runs and `nothing_to_retry` without a saved turn. Sending it
/// clears the record.
pub async fn retry<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    session_id: &str,
) -> Result<(), ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    refuse_turn_in_progress(connections, &row).await?;
    let turn = saved_for(db, row.id).await?;
    send_locked(db, connections, launcher, &row, turn.blocks, None, None).await
}

/// Whether `retry_in_new_chat` would pass its first two refusals now, so a
/// caller can skip gathering the chat's history for a call that is refused.
/// `retry_in_new_chat` still decides under the session lock.
pub async fn ensure_retryable(
    db: &AppDatabase,
    connections: &ConnectionManager,
    session_id: &str,
) -> Result<(), ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    refuse_turn_in_progress(connections, &row).await?;
    saved_for(db, row.id).await.map(|_| ())
}

/// Start a new chat in session `session_id`'s workspace, with its agent and
/// its model, effort and mode, whose first prompt is the saved can't-reach
/// turn plus a summary of `history` (the old session's chat) as an
/// attachment. Refused as `retry` is, and with `root_workspace` where no new
/// session can start. The old session, and its saved turn, stay as they are.
/// Returns the new `session_id`.
pub async fn retry_in_new_chat<L: AgentLauncher>(
    db: &AppDatabase,
    connections: &ConnectionManager,
    launcher: &L,
    session_id: &str,
    history: &[MessageTurn],
) -> Result<String, ZwiimError> {
    let row = resolve(&db.conn, session_id).await?;
    let lock = session_lock(session_id);
    let _guard = lock.lock().await;
    refuse_turn_in_progress(connections, &row).await?;
    let turn = saved_for(db, row.id).await?;
    let workspace_id = crate::workspace::workspace_id_of(&db.conn, row.folder_id)
        .await?
        .ok_or_else(|| ZwiimError::not_found("Session has no workspace"))?;
    let folder = crate::workspace::resolve(&db.conn, &workspace_id).await?;
    if folder.parent_id.is_none() {
        return Err(ZwiimError::root_workspace());
    }
    let agent_type: AgentType =
        serde_json::from_value(serde_json::Value::String(row.agent_type.clone()))
            .map_err(|_| ZwiimError::internal())?;
    let current = match crate::session_options::read(&db.conn, connections, &row, session_id).await
    {
        Some(current) => current,
        None => {
            tracing::warn!(
                session_id,
                "[zwiim] session options unknown, so the new chat starts on the agent's defaults"
            );
            Default::default()
        }
    };
    let options = OptionsRequest {
        model: current.model.map(|m| m.current),
        effort: current.effort.map(|e| e.current),
        mode: current.mode.map(|m| m.current),
    };

    let summary = session_summary::build(
        &session_summary::turns_from_history(history),
        &agent_type.to_string(),
        &turn.blocks,
    );
    let root = Path::new(&folder.path);
    let stored = attachments::store(root, session_summary::SUMMARY_FILE, summary.as_bytes()).await?;
    let path = attachments::resolve(root, &stored.attachment_id)?;
    let mut blocks = turn.blocks;
    blocks.push(PromptInputBlock::Text {
        text: attachments::note(std::slice::from_ref(&path)),
    });
    let mut dispatched = false;
    let started = start_tracked(
        db,
        connections,
        launcher,
        &workspace_id,
        agent_type,
        blocks,
        None,
        &options,
        &mut dispatched,
    )
    .await;
    if started.is_err() && !dispatched {
        // Nothing will read the summary: take it back out (best effort). Once
        // the first turn is dispatched it names the file, so it stays.
        if let Some(dir) = path.parent() {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
    }
    started
}

/// `turn_in_progress` while session `row` has a turn running.
async fn refuse_turn_in_progress(
    connections: &ConnectionManager,
    row: &conversation::Model,
) -> Result<(), ZwiimError> {
    if let Some(connection_id) = connection_for(connections, row).await {
        if let Some(state) = connections.get_state(&connection_id).await {
            if state.read().await.turn_in_flight {
                return Err(ZwiimError::turn_in_progress());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::message::{ContentBlock, MessageTurn, TurnRole};
    use crate::models::AgentType;
    use crate::session::orchestration_tests::{finish_turn, settle, text, workspace, FakeLauncher};
    use crate::session::{send, start};
    use std::sync::atomic::Ordering;

    fn image() -> PromptInputBlock {
        PromptInputBlock::Image {
            data: "aGk=".into(),
            mime_type: "image/png".into(),
            uri: None,
        }
    }

    /// Start a session whose first turn has ended, on `fake-conn-1`.
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
            AgentType::Codex,
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

    async fn save(db: &AppDatabase, sid: &str, blocks: Vec<PromptInputBlock>) {
        let row = resolve(&db.conn, sid).await.unwrap();
        conversation_service::set_unreachable_turn(
            &db.conn,
            row.id,
            Some(&UnreachableTurn {
                message: "Can't reach the agent's server.".into(),
                blocks,
            }),
        )
        .await
        .unwrap();
    }

    async fn record(db: &AppDatabase, sid: &str) -> Option<UnreachableTurn> {
        let row = resolve(&db.conn, sid).await.unwrap();
        conversation_service::unreachable_turn(&db.conn, row.id)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn retry_resends_saved_blocks_and_clears_record() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        let blocks = vec![PromptInputBlock::Text { text: "hi".into() }, image()];
        save(&db, &sid, blocks.clone()).await;

        retry(&db, &mgr, &launcher, &sid).await.unwrap();
        settle().await;

        let sent = launcher.prompt_blocks.lock().unwrap().clone();
        assert_eq!(sent.len(), 2, "the first prompt and the retry");
        assert_eq!(sent[1], ("fake-conn-1".to_string(), blocks));
        assert!(record(&db, &sid).await.is_none());
    }

    #[tokio::test]
    async fn retry_refuses_nothing_to_retry() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;

        let err = retry(&db, &mgr, &launcher, &sid).await.unwrap_err();
        assert_eq!(err.code, "nothing_to_retry");
        assert_eq!(err.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["hi"]);
    }

    /// A saved turn with no blocks has nothing to send, so it is nothing to
    /// retry rather than an empty prompt.
    #[tokio::test]
    async fn retry_refuses_empty_saved_blocks() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        save(&db, &sid, Vec::new()).await;

        let err = retry(&db, &mgr, &launcher, &sid).await.unwrap_err();
        assert_eq!(err.code, "nothing_to_retry");
        assert_eq!(saved(&db, &sid).await.unwrap_err().code, "nothing_to_retry");
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["hi"]);
    }

    #[tokio::test]
    async fn retry_refuses_turn_in_progress() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let wid = workspace(&db).await;
        let sid = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::Codex,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        save(&db, &sid, text("again")).await;

        let err = retry(&db, &mgr, &launcher, &sid).await.unwrap_err();
        assert_eq!(err.code, "turn_in_progress");
        assert!(
            record(&db, &sid).await.is_some(),
            "a refused retry keeps it"
        );
        assert_eq!(launcher.prompts_for("fake-conn-1"), ["hi"]);
    }

    /// The record lives on the session row, so a retry after the agent is gone
    /// resumes the agent session under the same session_id.
    #[tokio::test]
    async fn saved_record_survives_without_live_agent() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        save(&db, &sid, text("again")).await;
        mgr.disconnect("fake-conn-1").await.unwrap();
        let external_id = resolve(&db.conn, &sid).await.unwrap().external_id;

        assert_eq!(saved(&db, &sid).await.unwrap().blocks, text("again"));
        retry(&db, &mgr, &launcher, &sid).await.unwrap();
        settle().await;

        assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
        assert_eq!(launcher.resumed.lock().unwrap()[1], external_id);
        assert_eq!(launcher.prompts_for("fake-conn-2"), ["again"]);
        let row = resolve(&db.conn, &sid).await.unwrap();
        assert_eq!(row.session_id.as_deref(), Some(sid.as_str()));
        assert!(record(&db, &sid).await.is_none());
    }

    #[tokio::test]
    async fn send_clears_saved_record() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let sid = idle_session(&db, &mgr, &launcher).await;
        save(&db, &sid, text("hi")).await;

        send(&db, &mgr, &launcher, &sid, text("something else"), None)
            .await
            .unwrap();
        settle().await;

        assert!(record(&db, &sid).await.is_none());
        assert_eq!(
            launcher.prompts_for("fake-conn-1"),
            ["hi", "something else"]
        );
    }

    // ── retry in a new chat ─────────────────────────────────────────────

    fn transcript(pairs: &[(TurnRole, &str)]) -> Vec<MessageTurn> {
        pairs
            .iter()
            .enumerate()
            .map(|(i, (role, t))| MessageTurn {
                id: format!("turn-{i}"),
                role: role.clone(),
                blocks: vec![ContentBlock::Text { text: t.to_string() }],
                timestamp: chrono::Utc::now(),
                usage: None,
                duration_ms: None,
                model: None,
                completed_at: None,
                agent_message_id: None,
            })
            .collect()
    }

    fn old_history() -> Vec<MessageTurn> {
        transcript(&[
            (TurnRole::User, "hi"),
            (TurnRole::Assistant, "hello there"),
            (TurnRole::User, "fix the bug"),
        ])
    }

    /// Claude's options with a chosen model, effort and mode, as its agent
    /// reports them.
    fn claude_options() -> Vec<crate::acp::types::SessionConfigOptionInfo> {
        use crate::acp::types::SessionConfigKindInfo;
        let mut options = crate::session_options::tests::claude("plan");
        for option in &mut options {
            if let SessionConfigKindInfo::Select(s) = &mut option.kind {
                match option.id.as_str() {
                    "model" => s.current_value = "haiku".into(),
                    "effort" => s.current_value = "low".into(),
                    _ => {}
                }
            }
        }
        options
    }

    async fn workspace_of(db: &AppDatabase, sid: &str) -> Option<String> {
        let row = resolve(&db.conn, sid).await.unwrap();
        crate::workspace::workspace_id_of(&db.conn, row.folder_id)
            .await
            .unwrap()
    }

    /// The model, effort and mode in effect for `sid`.
    async fn currents(
        db: &AppDatabase,
        mgr: &ConnectionManager,
        sid: &str,
    ) -> (Option<String>, Option<String>, Option<crate::session_options::Mode>) {
        let row = resolve(&db.conn, sid).await.unwrap();
        let options = crate::session_options::read(&db.conn, mgr, &row, sid)
            .await
            .expect("options known");
        (
            options.model.map(|m| m.current),
            options.effort.map(|e| e.current),
            options.mode.map(|m| m.current),
        )
    }

    #[tokio::test]
    async fn retry_in_new_chat_starts_same_workspace_agent_and_options() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        crate::session_options::remember_agent_report(
            &db.conn,
            "claude_code",
            &crate::session_options::tests::claude("default"),
        )
        .await;
        let wid = workspace(&db).await;
        let old = start(
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
        finish_turn(&mgr, "fake-conn-1").await;
        mgr.get_state("fake-conn-1")
            .await
            .unwrap()
            .write()
            .await
            .config_options = Some(claude_options());
        let blocks = vec![PromptInputBlock::Text { text: "fix the bug".into() }, image()];
        save(&db, &old, blocks.clone()).await;
        let old_options = currents(&db, &mgr, &old).await;
        assert_eq!(
            old_options,
            (
                Some("haiku".into()),
                Some("low".into()),
                Some(crate::session_options::Mode::Plan)
            )
        );

        let new = retry_in_new_chat(&db, &mgr, &launcher, &old, &old_history())
            .await
            .unwrap();
        settle().await;

        assert_ne!(new, old);
        assert_eq!(workspace_of(&db, &new).await, Some(wid.clone()));
        let new_row = resolve(&db.conn, &new).await.unwrap();
        assert_eq!(new_row.agent_type, resolve(&db.conn, &old).await.unwrap().agent_type);
        assert_eq!(currents(&db, &mgr, &new).await, old_options);

        let sent = launcher.prompt_blocks.lock().unwrap().clone();
        let (conn, first) = sent.last().unwrap().clone();
        assert_eq!(conn, "fake-conn-2");
        assert_eq!(first.len(), blocks.len() + 1);
        assert_eq!(first[..blocks.len()], blocks[..]);
        let PromptInputBlock::Text { text: note } = &first[blocks.len()] else {
            panic!("note block: {:?}", first[blocks.len()]);
        };
        let paths: Vec<&str> = note
            .lines()
            .filter_map(|l| l.strip_prefix("- "))
            .collect();
        assert_eq!(paths.len(), 1, "{note}");
        assert!(paths[0].ends_with("/earlier-chat-summary.md"), "{note}");
        let summary = std::fs::read_to_string(paths[0]).unwrap();
        assert!(summary.starts_with("# Earlier chat"), "{summary}");
        assert_eq!(summary.matches("fix the bug").count(), 1, "{summary}");
        assert!(summary.contains("## Claude Code"), "{summary}");

        assert!(record(&db, &old).await.is_some(), "the old session keeps its card");
    }

    #[tokio::test]
    async fn retry_in_new_chat_title_from_failed_message() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let old = idle_session(&db, &mgr, &launcher).await;
        save(&db, &old, text("please fix the login page")).await;

        let new = retry_in_new_chat(&db, &mgr, &launcher, &old, &old_history())
            .await
            .unwrap();
        settle().await;

        let row = resolve(&db.conn, &new).await.unwrap();
        assert_eq!(row.title.as_deref(), Some("please fix the login page"));
    }

    #[tokio::test]
    async fn retry_in_new_chat_refuses_root_workspace() {
        use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel};
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let old = idle_session(&db, &mgr, &launcher).await;
        save(&db, &old, text("again")).await;
        let folder_id = resolve(&db.conn, &old).await.unwrap().folder_id;
        let mut folder = crate::db::entities::folder::Entity::find_by_id(folder_id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
            .into_active_model();
        folder.parent_id = Set(None);
        folder.update(&db.conn).await.unwrap();

        let err = retry_in_new_chat(&db, &mgr, &launcher, &old, &old_history())
            .await
            .unwrap_err();
        assert_eq!(err.code, "root_workspace");
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
        let workspace_dir = crate::session::workspace_dir(&db.conn, &old).await.unwrap();
        assert!(
            !std::path::Path::new(&workspace_dir).join(".zwiim/attachments").exists(),
            "no summary written"
        );
    }

    /// An agent that cannot be launched.
    struct NoAgent;

    impl AgentLauncher for NoAgent {
        async fn launch(
            &self,
            _agent_type: AgentType,
            _working_dir: String,
            _folder_id: i32,
            _resume_external_id: Option<String>,
            _preferred: std::collections::BTreeMap<String, String>,
        ) -> Result<String, ZwiimError> {
            Err(ZwiimError::agent_not_installed())
        }
    }

    #[tokio::test]
    async fn retry_in_new_chat_failed_start_removes_the_summary() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let old = idle_session(&db, &mgr, &launcher).await;
        save(&db, &old, text("again")).await;

        let err = retry_in_new_chat(&db, &mgr, &NoAgent, &old, &old_history())
            .await
            .unwrap_err();
        assert_eq!(err.code, "agent_not_installed");
        let workspace_dir = crate::session::workspace_dir(&db.conn, &old).await.unwrap();
        let base = std::path::Path::new(&workspace_dir).join(".zwiim/attachments");
        let left: Vec<_> = std::fs::read_dir(&base)
            .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "summary left behind: {left:?}");
        assert!(record(&db, &old).await.is_some());
    }

    #[tokio::test]
    async fn retry_in_new_chat_refuses_nothing_to_retry() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let old = idle_session(&db, &mgr, &launcher).await;

        let err = retry_in_new_chat(&db, &mgr, &launcher, &old, &old_history())
            .await
            .unwrap_err();
        assert_eq!(err.code, "nothing_to_retry");
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_in_new_chat_refuses_turn_in_progress() {
        let db = fresh_db().await;
        let mgr = ConnectionManager::new();
        let launcher = FakeLauncher::new(mgr.clone_ref());
        let wid = workspace(&db).await;
        let old = start(
            &db,
            &mgr,
            &launcher,
            &wid,
            AgentType::Codex,
            text("hi"),
            None,
            &Default::default(),
        )
        .await
        .unwrap();
        settle().await;
        save(&db, &old, text("again")).await;

        let err = retry_in_new_chat(&db, &mgr, &launcher, &old, &old_history())
            .await
            .unwrap_err();
        assert_eq!(err.code, "turn_in_progress");
        assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
        assert!(record(&db, &old).await.is_some());
    }

    async fn fresh_db() -> AppDatabase {
        crate::db::test_helpers::fresh_in_memory_db().await
    }
}
