//! What agent sessions are shown of the Host's connectors (connector design
//! r3 §3.3, §8.2): which connectors a new session gets a proxy for, and
//! which tools each proxy lists. A proxy lists the tools the user turned on
//! that the session's grant doesn't deny; every call is still decided on
//! its own.

use std::path::Path;

use super::{CallSession, Connectors};
use crate::connectors::grants;
use crate::connectors::policy::{self, Decision, Facts, ToolState};
use crate::connectors::store::{self, ConnectorStatus, ListedTool};
use crate::connectors::upstream::ToolDefinition;

/// A connector a new session gets a proxy for.
#[derive(Clone, Debug)]
pub struct Offered {
    pub connector: store::Connector,
    /// About what its shown tools take of the agent's context.
    pub cost: u32,
}

impl Connectors {
    /// The connectors a new agent session in `working_dir` run by `agent`
    /// gets a proxy for: those that would show it at least one tool.
    /// Engine sessions get none. A store failure offers nothing.
    pub async fn offered(&self, working_dir: &Path, agent: &str, engine: bool) -> Vec<Offered> {
        if engine {
            return Vec::new();
        }
        let offered = async {
            let workspace_id = crate::workspace::root_workspace_id_for_dir(
                &self.db,
                &working_dir.to_string_lossy(),
            )
            .await
            .map_err(|e| e.to_string())?;
            let mut offered = Vec::new();
            for connector in store::connectors(&self.db).await.map_err(internal)? {
                let shown = self
                    .shown(&connector, workspace_id.as_deref(), agent, None)
                    .await?;
                if !shown.is_empty() {
                    let cost = shown.iter().map(|t| t.estimate).sum();
                    offered.push(Offered { connector, cost });
                }
            }
            Ok::<_, String>(offered)
        };
        offered.await.unwrap_or_else(|e| {
            tracing::warn!("[connectors] couldn't decide what to offer {agent}: {e}");
            Vec::new()
        })
    }

    /// The tools `session`'s proxy for connector `id` lists, and the
    /// revision they are at.
    pub async fn exposed_tools(
        &self,
        session: &CallSession,
        id: &str,
    ) -> Result<(u64, Vec<ToolDefinition>), String> {
        // Read first: a change after this is a later revision.
        let revision = self.runtime(id).revision();
        let connector = store::connector(&self.db, id)
            .await
            .map_err(internal)?
            .ok_or_else(|| "This connector no longer exists.".to_string())?;
        if session.engine {
            return Ok((revision, Vec::new()));
        }
        let shown = self
            .shown(
                &connector,
                session.workspace_id.as_deref(),
                &session.agent,
                Some(&session.session_id),
            )
            .await?;
        Ok((revision, shown.into_iter().map(|t| t.tool).collect()))
    }

    /// `connector`'s tools that are on and that a session in root workspace
    /// `workspace_id` run by `agent` isn't denied, by name. None while the
    /// connector isn't usable.
    async fn shown(
        &self,
        connector: &store::Connector,
        workspace_id: Option<&str>,
        agent: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<ListedTool>, String> {
        if connector.status != ConnectorStatus::Active || self.runtime(&connector.id).is_blocked() {
            return Ok(Vec::new());
        }
        let Some(grant) = grants::grant_for(&self.db, &connector.id, workspace_id, agent)
            .await
            .map_err(internal)?
        else {
            return Ok(Vec::new());
        };
        let mut shown = Vec::new();
        for listed in store::listed(&self.db, &connector.id)
            .await
            .map_err(internal)?
        {
            if !listed.enabled {
                continue;
            }
            let hash = listed.tool.hash();
            let allowed = session_id.and_then(|session| {
                self.asking
                    .session_allowed(session, &connector.id, &listed.tool.name)
            });
            let decision = policy::decide(Facts {
                connector_active: true,
                engine_session: false,
                tool: Some(ToolState {
                    name: &listed.tool.name,
                    hash: &hash,
                    first_seen_at: listed.first_seen_at,
                }),
                grant: Some(&grant),
                session_allowed: allowed.as_deref(),
            });
            if !matches!(decision, Decision::Deny(_)) {
                shown.push(listed);
            }
        }
        Ok(shown)
    }
}

fn internal(e: impl std::fmt::Display) -> String {
    tracing::warn!("[connectors] {e}");
    "Zwiim couldn't read this connector.".into()
}
