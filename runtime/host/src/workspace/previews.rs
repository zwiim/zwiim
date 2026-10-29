//! Zwiim-owned: where a client opens a worktree's servers — the repository's
//! preview URLs with the worktree's ports in them, or, with none set, the
//! servers its terminals printed that still answer.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use sea_orm::DatabaseConnection;
use serde::Serialize;

use crate::browser::services::registry;
use crate::protocol::error::ZwiimError;
use crate::repository_settings;
use crate::terminal::manager::TerminalManager;

/// The owner every terminal the Host starts for a client has, so the one its
/// detected servers are recorded under.
const OWNER: &str = "zwiim";

/// One place to open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preview {
    /// What the user named it; detected servers have none.
    pub name: Option<String>,
    pub url: String,
}

/// Worktree `workspace_id`'s previews, first the one to open by default.
pub async fn previews(
    db: &DatabaseConnection,
    terminals: &TerminalManager,
    workspace_id: &str,
) -> Result<Vec<Preview>, ZwiimError> {
    let worktree = super::resolve(db, workspace_id).await?;
    let Some(parent) = worktree.parent_id else {
        return Err(ZwiimError::root_workspace());
    };
    let configured = repository_settings::preview_urls(db, parent).await;
    if !configured.is_empty() {
        let port = super::ports::port_of(db, worktree.id).await;
        return Ok(configured
            .into_iter()
            .filter_map(|(name, url)| Some(Preview { name, url: with_ports(&url, port)? }))
            .collect());
    }
    let mine: HashSet<String> = terminals
        .list_with_exit_check(None)
        .into_iter()
        .filter(|t| std::path::Path::new(&t.working_dir).starts_with(&worktree.path))
        .map(|t| t.id)
        .collect();
    if mine.is_empty() {
        return Ok(Vec::new());
    }
    // Asks each whether it still answers: blocking.
    let live = tokio::task::spawn_blocking(|| registry().list_live(OWNER))
        .await
        .unwrap_or_default();
    // Newest first: most likely what was just started.
    Ok(live
        .into_iter()
        .rev()
        .filter(|s| mine.contains(&s.terminal_id))
        .map(|s| Preview { name: None, url: s.url })
        .collect())
}

/// `$ZWIIM_PORT`, `${ZWIIM_PORT}` and `$((ZWIIM_PORT + n))` (n 0–9) as they
/// are in the worktree's terminals.
static PORT_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\(\(\s*ZWIIM_PORT\s*(?:\+\s*(\d)\s*)?\)\)|\$\{ZWIIM_PORT\}|\$ZWIIM_PORT\b").unwrap()
});

/// `url` with the worktree's ports from `port`, when it is still a URL to
/// open: `None` when it names a port the worktree hasn't got. Nothing else is
/// expanded — the repository's variables can be keys, and URLs go to clients.
fn with_ports(url: &str, port: Option<u16>) -> Option<String> {
    let mut missing = false;
    let url = PORT_REF.replace_all(url, |c: &Captures| {
        let offset = c.get(1).map_or(0, |d| d.as_str().parse::<u16>().unwrap_or(0));
        match port {
            Some(p) => (p + offset).to_string(),
            None => {
                missing = true;
                String::new()
            }
        }
    });
    let openable = repository_settings::is_web_url(&url) && !url.chars().any(char::is_whitespace);
    (!missing && openable).then(|| url.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::types::{DetectedService, ServiceSource};
    use crate::db::entities::folder;
    use crate::db::service::folder_service::add_folder_with_parent;
    use sea_orm::EntityTrait;
    use crate::db::test_helpers::fresh_in_memory_db;
    use crate::terminal::manager::SpawnOptions;
    use crate::web::event_bridge::EventEmitter;
    use serde_json::json;
    use std::net::{Ipv4Addr, TcpListener};

    #[test]
    fn gives_urls_the_worktrees_ports_and_nothing_else() {
        let p = Some(40_010);
        assert_eq!(with_ports("http://localhost:$ZWIIM_PORT", p).unwrap(), "http://localhost:40010");
        assert_eq!(with_ports("http://127.0.0.1:${ZWIIM_PORT}/a", p).unwrap(), "http://127.0.0.1:40010/a");
        assert_eq!(
            with_ports("https://localhost:$((ZWIIM_PORT + 3))/?q=$((ZWIIM_PORT+9))", p).unwrap(),
            "https://localhost:40013/?q=40019"
        );
        assert_eq!(with_ports("http://localhost:$((ZWIIM_PORT))", p).unwrap(), "http://localhost:40010");
        assert_eq!(with_ports("http://example.com", None).unwrap(), "http://example.com");
        // The repository's own variables stay as written.
        assert_eq!(with_ports("http://x/$API_KEY", p).unwrap(), "http://x/$API_KEY");
        assert_eq!(with_ports("http://x/$ZWIIM_PORTS", p).unwrap(), "http://x/$ZWIIM_PORTS");
        // Not one to open.
        assert_eq!(with_ports("http://localhost:$ZWIIM_PORT", None), None);
        assert_eq!(with_ports("http://localhost:$((ZWIIM_PORT + 10))", p), None);
        assert_eq!(with_ports("$ZWIIM_PORT", p), None);
    }

    #[tokio::test]
    async fn opens_what_the_repository_set_else_what_its_terminals_serve() {
        let db = fresh_in_memory_db().await;
        let repo = add_folder_with_parent(&db.conn, "/tmp/zwiim-previews-repo", None).await.unwrap();
        let wt = add_folder_with_parent(&db.conn, "/tmp/zwiim-previews-wt", Some(repo.id))
            .await
            .unwrap();
        let workspace_id = |id: i32| {
            let conn = &db.conn;
            async move {
                folder::Entity::find_by_id(id).one(conn).await.unwrap().unwrap().workspace_id.unwrap()
            }
        };
        let (repo_id, wt_id) = (workspace_id(repo.id).await, workspace_id(wt.id).await);
        let terminals = TerminalManager::new();

        assert_eq!(
            previews(&db.conn, &terminals, &repo_id).await.unwrap_err().code,
            "root_workspace"
        );
        assert!(previews(&db.conn, &terminals, &wt_id).await.unwrap().is_empty());

        // A server one of its terminals printed, while it answers.
        let dir = tempfile::tempdir().unwrap();
        let wt_dir = dir.path().to_string_lossy().into_owned();
        let wt2 = add_folder_with_parent(&db.conn, &wt_dir, Some(repo.id)).await.unwrap();
        let wt2_id = workspace_id(wt2.id).await;
        let terminal = terminals
            .spawn_with_id(
                SpawnOptions {
                    terminal_id: "zwiim-previews-run".into(),
                    working_dir: wt_dir.clone(),
                    owner_window_label: OWNER.into(),
                    shell: None,
                    initial_command: None,
                    extra_env: None,
                    temp_files: Vec::new(),
                    title: Some("Run".into()),
                },
                EventEmitter::Noop,
            )
            .unwrap();
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let authority = server.local_addr().unwrap().to_string();
        let service = |terminal_id: &str| DetectedService {
            url: format!("http://{authority}/"),
            origin: format!("http://{authority}"),
            authority: authority.clone(),
            owner_window: OWNER.to_string(),
            source: ServiceSource::Terminal,
            terminal_id: terminal_id.to_string(),
        };
        registry().record(service(&terminal));
        assert_eq!(
            previews(&db.conn, &terminals, &wt2_id).await.unwrap(),
            vec![Preview { name: None, url: format!("http://{authority}/") }]
        );
        assert!(previews(&db.conn, &terminals, &wt_id).await.unwrap().is_empty());
        drop(server);
        assert!(previews(&db.conn, &terminals, &wt2_id).await.unwrap().is_empty());

        // Set ones win, with the worktree's ports.
        repository_settings::update(
            &db.conn,
            &repo_id,
            &json!({ "preview.urls": [
                { "url": "http://localhost:$((ZWIIM_PORT + 1))" },
                { "name": "Docs", "url": "https://docs.example.com" },
            ] })
            .as_object()
            .unwrap()
            .clone(),
        )
        .await
        .unwrap();
        let port = super::super::ports::port_of(&db.conn, wt.id).await.unwrap();
        assert_eq!(
            previews(&db.conn, &terminals, &wt_id).await.unwrap(),
            vec![
                Preview { name: None, url: format!("http://localhost:{}", port + 1) },
                Preview { name: Some("Docs".into()), url: "https://docs.example.com".into() },
            ]
        );
        let _ = terminals.kill(&terminal);
    }
}
