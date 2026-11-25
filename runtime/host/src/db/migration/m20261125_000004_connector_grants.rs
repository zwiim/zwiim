//! Zwiim-owned: which sessions may use a connection, and how each of its
//! tools is treated (connector design r3 §5.1, §9).
//!
//! A grant is one connection for a scope: every workspace or one root
//! workspace, any agent or one. `default_policy` covers tools with no row
//! and `new_tools` tools first seen after the grant was made; neither may
//! be `allow`. A tool row allows, asks or denies one tool; an `allow` is
//! pinned to the hash the user approved.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

fn ask_or_deny(column: ConnectorGrants) -> SimpleExpr {
    Expr::col(column).is_in(["ask", "deny"])
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConnectorGrants::Table)
                    .col(
                        ColumnDef::new(ConnectorGrants::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConnectorGrants::ConnectionId)
                            .string()
                            .not_null(),
                    )
                    // A root workspace's `workspace_id`; null for every one.
                    .col(ColumnDef::new(ConnectorGrants::WorkspaceId).string().null())
                    // The protocol's agent name; null for any agent.
                    .col(ColumnDef::new(ConnectorGrants::Agent).string().null())
                    .col(
                        ColumnDef::new(ConnectorGrants::DefaultPolicy)
                            .string()
                            .not_null()
                            .check(ask_or_deny(ConnectorGrants::DefaultPolicy)),
                    )
                    .col(
                        ColumnDef::new(ConnectorGrants::NewTools)
                            .string()
                            .not_null()
                            .check(ask_or_deny(ConnectorGrants::NewTools)),
                    )
                    .col(
                        ColumnDef::new(ConnectorGrants::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorGrants::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connector_grants_connection_id")
                            .from(ConnectorGrants::Table, ConnectorGrants::ConnectionId)
                            .to(Connections::Table, Connections::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // One grant per scope. SQLite treats NULLs as distinct in a plain
        // unique index, so the scope columns are compared through COALESCE.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX idx_connector_grants_scope ON connector_grants \
                 (connection_id, COALESCE(workspace_id, ''), COALESCE(agent, ''))",
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(GrantToolPolicies::Table)
                    .col(
                        ColumnDef::new(GrantToolPolicies::GrantId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GrantToolPolicies::ToolName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(GrantToolPolicies::Policy)
                            .string()
                            .not_null()
                            .check(
                                Expr::col(GrantToolPolicies::Policy)
                                    .is_in(["allow", "ask", "deny"]),
                            ),
                    )
                    // The tool hash an `allow` was given for.
                    .col(
                        ColumnDef::new(GrantToolPolicies::SchemaHash)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(GrantToolPolicies::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(GrantToolPolicies::GrantId)
                            .col(GrantToolPolicies::ToolName),
                    )
                    .check(
                        Expr::col(GrantToolPolicies::Policy)
                            .ne("allow")
                            .or(Expr::col(GrantToolPolicies::SchemaHash).is_not_null()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_grant_tool_policies_grant_id")
                            .from(GrantToolPolicies::Table, GrantToolPolicies::GrantId)
                            .to(ConnectorGrants::Table, ConnectorGrants::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(GrantToolPolicies::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConnectorGrants::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden, Clone, Copy)]
enum ConnectorGrants {
    Table,
    Id,
    ConnectionId,
    WorkspaceId,
    Agent,
    DefaultPolicy,
    NewTools,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum GrantToolPolicies {
    Table,
    GrantId,
    ToolName,
    Policy,
    SchemaHash,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Connections {
    Table,
    Id,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    const NOW: &str = "'2026-11-25T00:00:00Z'";

    fn grant(scope: &str, default_policy: &str) -> String {
        format!(
            "INSERT INTO connector_grants (connection_id, workspace_id, agent, \
             default_policy, new_tools, created_at, updated_at) VALUES ('c1', {scope}, \
             '{default_policy}', 'ask', {NOW}, {NOW})"
        )
    }

    fn row(tool: &str, policy: &str, hash: &str) -> String {
        format!(
            "INSERT INTO grant_tool_policies (grant_id, tool_name, policy, schema_hash, \
             updated_at) VALUES (1, '{tool}', '{policy}', {hash}, {NOW})"
        )
    }

    #[tokio::test]
    async fn grants_are_one_per_scope_and_never_allow_by_default() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        conn.execute_unprepared(
            "PRAGMA foreign_keys = ON; CREATE TABLE connections (id TEXT PRIMARY KEY); \
             INSERT INTO connections (id) VALUES ('c1')",
        )
        .await
        .unwrap();
        Migration.up(&manager).await.expect("run migration up");

        conn.execute_unprepared(&grant("NULL, NULL", "ask"))
            .await
            .expect("Host default");
        assert!(
            conn.execute_unprepared(&grant("NULL, NULL", "deny"))
                .await
                .is_err(),
            "a second grant for the same scope"
        );
        conn.execute_unprepared(&grant("'ws1', NULL", "deny"))
            .await
            .expect("one workspace");
        conn.execute_unprepared(&grant("'ws1', 'codex'", "ask"))
            .await
            .expect("one workspace, one agent");
        assert!(conn
            .execute_unprepared(&grant("'ws2', NULL", "allow"))
            .await
            .is_err());

        assert!(
            conn.execute_unprepared(&row("echo", "allow", "NULL"))
                .await
                .is_err(),
            "an allow needs the hash it was given for"
        );
        conn.execute_unprepared(&row("echo", "allow", "'sha256:0'"))
            .await
            .expect("pinned allow");
        assert!(
            conn.execute_unprepared(&row("echo", "deny", "NULL"))
                .await
                .is_err(),
            "one row per tool"
        );
        conn.execute_unprepared(&row("search", "ask", "NULL"))
            .await
            .expect("ask needs no hash");
        conn.execute_unprepared("DELETE FROM connections WHERE id = 'c1'")
            .await
            .unwrap();
        let left = conn
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT (SELECT COUNT(*) FROM connector_grants) + \
                 (SELECT COUNT(*) FROM grant_tool_policies) AS n",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(left, 0, "grants and rows go with their connection");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("connector_grants").await.unwrap());
        assert!(!manager.has_table("grant_tool_policies").await.unwrap());
    }
}
