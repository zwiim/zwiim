// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: clear the credentials an older build left in plaintext SQLite.
//!
//! `model_provider.api_key`, `remote_workspace_connection.token` and the
//! credential-named variables in `agent_setting.env_json` refuse new values
//! (`db::plaintext_secrets`); this removes the ones already on disk, so they
//! are neither kept in plaintext nor carried off the host by a config
//! snapshot. An environment that is not a variable map cannot be read by the
//! agent launch either, and could hide a credential, so it is cleared too.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement, Value};

use crate::db::plaintext_secrets::is_secret_env_name;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in [
            "UPDATE model_provider SET api_key = '' WHERE trim(api_key) <> ''",
            "UPDATE remote_workspace_connection SET token = '' WHERE trim(token) <> ''",
        ] {
            conn.execute(Statement::from_string(DbBackend::Sqlite, sql.to_string()))
                .await?;
        }

        let rows = conn
            .query_all(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT id, env_json FROM agent_setting \
                 WHERE env_json IS NOT NULL AND trim(env_json) <> ''"
                    .to_string(),
            ))
            .await?;
        for row in rows {
            let id: i32 = row.try_get("", "id")?;
            let raw: String = row.try_get("", "env_json")?;
            let Some(cleared) = without_credentials(&raw) else {
                continue;
            };
            conn.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "UPDATE agent_setting SET env_json = ? WHERE id = ?",
                [cleared.into(), Value::from(id)],
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Data-only and irreversible: the cleared values are not recorded
        // anywhere, which is the point.
        Ok(())
    }
}

/// `raw` without its credential-named variables, or `None` if it has none.
/// An environment that is not a variable map becomes `Some(None)` (NULL).
fn without_credentials(raw: &str) -> Option<Option<String>> {
    let parsed: Option<serde_json::Map<String, serde_json::Value>> = match serde_json::from_str(raw)
    {
        Ok(parsed) => parsed,
        Err(_) => return Some(None),
    };
    let mut env = parsed?;
    let before = env.len();
    env.retain(|name, _| !is_secret_env_name(name));
    (env.len() != before).then(|| Some(serde_json::Value::Object(env).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_helpers::fresh_in_memory_db;

    #[test]
    fn only_credential_variables_are_removed() {
        assert_eq!(without_credentials("null"), None);
        assert_eq!(
            without_credentials(r#"{"ANTHROPIC_BASE_URL":"https://x"}"#),
            None
        );
        assert_eq!(
            without_credentials(r#"{"ANTHROPIC_API_KEY":"sk-1","MAX_THINKING_TOKENS":"8000"}"#),
            Some(Some(r#"{"MAX_THINKING_TOKENS":"8000"}"#.to_string()))
        );
        assert_eq!(without_credentials("not json"), Some(None));
        assert_eq!(without_credentials("[1]"), Some(None));
    }

    #[tokio::test]
    async fn legacy_credentials_are_cleared_and_the_rest_is_kept() {
        let db = fresh_in_memory_db().await;
        let exec = |sql: &str| Statement::from_string(DbBackend::Sqlite, sql.to_string());
        for sql in [
            "INSERT INTO model_provider (name, api_url, api_key, agent_types_json, agent_type, created_at, updated_at) \
             VALUES ('p', 'https://api.example', 'sk-live-1', '[]', 'claude_code', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            "INSERT INTO remote_workspace_connection (name, base_url, token, headers, sort_order, created_at, updated_at) \
             VALUES ('r', 'http://127.0.0.1:3080', 'bearer-1', '[]', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            "INSERT INTO agent_setting (agent_type, registry_id, enabled, sort_order, env_json, created_at, updated_at) \
             VALUES ('\"claude_code\"', 'claude', 1, 0, '{\"ANTHROPIC_API_KEY\":\"sk-ant-1\",\"ANTHROPIC_BASE_URL\":\"https://x\"}', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        ] {
            db.conn.execute(exec(sql)).await.unwrap();
        }

        Migration.up(&SchemaManager::new(&db.conn)).await.unwrap();

        let one = |sql: &str| {
            let conn = db.conn.clone();
            let stmt = exec(sql);
            async move { conn.query_one(stmt).await.unwrap().unwrap() }
        };
        let key: String = one("SELECT api_key FROM model_provider")
            .await
            .try_get("", "api_key")
            .unwrap();
        assert_eq!(key, "");
        let token: String = one("SELECT token FROM remote_workspace_connection")
            .await
            .try_get("", "token")
            .unwrap();
        assert_eq!(token, "");
        let env: String = one("SELECT env_json FROM agent_setting")
            .await
            .try_get("", "env_json")
            .unwrap();
        assert_eq!(env, r#"{"ANTHROPIC_BASE_URL":"https://x"}"#);
    }
}
