//! Zwiim-owned session commands: the slash commands a session's agent offers,
//! in Zwiim vocabulary. A command is run by sending `/name args` as prompt
//! text; the agent interprets it, so nothing here executes anything.
//!
//! A live session's commands reach clients through its snapshot and the
//! `session.commands` event. A new session has no agent yet, so each agent's
//! most recent list is kept per agent and offered through `agent.list`.

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;

use crate::acp::types::{AcpEvent, AvailableCommandInfo};
use crate::app_state::AppState;
use crate::db::service::app_metadata_service;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionCommand {
    /// Without the leading `/`.
    pub name: String,
    pub description: String,
    /// What to type after the name, when the command takes input.
    pub hint: Option<String>,
}

pub fn project(commands: &[AvailableCommandInfo]) -> Vec<SessionCommand> {
    commands
        .iter()
        .map(|c| SessionCommand {
            name: c.name.clone(),
            description: crate::protocol::error::scrub_paths(&c.description),
            hint: c.input_hint.clone().filter(|h| !h.is_empty()),
        })
        .collect()
}

fn agent_key(agent: &str) -> String {
    format!("zwiim.agent_commands:{agent}")
}

/// The commands `agent` offered in its most recent session on this host;
/// `None` before its first.
pub async fn for_new_session(db: &DatabaseConnection, agent: &str) -> Option<Vec<SessionCommand>> {
    let raw = app_metadata_service::get_value(db, &agent_key(agent))
        .await
        .ok()??;
    serde_json::from_str(&raw).ok()
}

/// Keep each agent's latest command list for its next new session.
pub fn spawn_tracker(state: Arc<AppState>) {
    let mut rx = state.acp_event_bus.subscribe();
    tokio::spawn(async move {
        loop {
            let envelope = match rx.recv().await {
                Ok(envelope) => envelope,
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            };
            if let AcpEvent::AvailableCommands { commands } = &envelope.payload {
                // An agent may announce an empty list before its real one.
                if !commands.is_empty() {
                    track(&state, &envelope.connection_id, commands).await;
                }
            }
        }
    });
}

async fn track(state: &AppState, connection_id: &str, commands: &[AvailableCommandInfo]) {
    let Some(session_state) = state.connection_manager.get_state(connection_id).await else {
        return;
    };
    let agent_type = session_state.read().await.agent_type;
    let Some(agent) = crate::session_options::agent_name(agent_type) else {
        return;
    };
    remember(&state.db.conn, &agent, commands).await;
}

pub(crate) async fn remember(db: &DatabaseConnection, agent: &str, commands: &[AvailableCommandInfo]) {
    let Ok(raw) = serde_json::to_string(&project(commands)) else {
        return;
    };
    if let Err(e) = app_metadata_service::upsert_value(db, &agent_key(agent), &raw).await {
        tracing::warn!(agent, error = %e, "[zwiim] failed to persist agent commands");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    fn info(name: &str, description: &str, hint: Option<&str>) -> AvailableCommandInfo {
        AvailableCommandInfo {
            name: name.into(),
            description: description.into(),
            input_hint: hint.map(Into::into),
        }
    }

    #[test]
    fn projects_to_zwiim_fields_without_host_paths() {
        let out = project(&[
            info("review", "Review a PR", Some("pr number")),
            info("deploy", "Runs /Users/a/repo/.claude/commands/deploy.md", Some("")),
        ]);
        assert_eq!(
            out[0],
            SessionCommand {
                name: "review".into(),
                description: "Review a PR".into(),
                hint: Some("pr number".into()),
            }
        );
        assert_eq!(out[1].hint, None);
        assert!(!out[1].description.contains("/Users/a"), "{}", out[1].description);
        let json = serde_json::to_value(&out[0]).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["description", "hint", "name"]);
    }

    #[tokio::test]
    async fn new_sessions_get_the_agents_latest_list() {
        let db = fresh_in_memory_db().await;
        assert_eq!(for_new_session(&db.conn, "claude_code").await, None);
        remember(&db.conn, "claude_code", &[info("compact", "Compact", None)]).await;
        remember(&db.conn, "claude_code", &[info("review", "Review", None)]).await;
        let listed = for_new_session(&db.conn, "claude_code").await.unwrap();
        assert_eq!(listed.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["review"]);
        assert_eq!(for_new_session(&db.conn, "codex").await, None);
    }
}
