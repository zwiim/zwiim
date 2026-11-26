//! Zwiim-owned: the user's approval of one connector call (connector design
//! r3 §5.2, §9). One per execution at most; the row, not the card a client
//! shows, decides whether the call may run.
//!
//! `binding_hash` ties an answer to exactly this call: its execution,
//! connector, tool, tool hash and arguments. `expires_at` is fixed when
//! the approval is made. `preview` is what the card shows of the arguments,
//! redacted and short, and is cleared once the approval can't be used.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConnectorApprovals::Table)
                    .col(
                        ColumnDef::new(ConnectorApprovals::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::ExecutionId)
                            .string()
                            .not_null()
                            .unique_key(),
                    )
                    // The session that asked.
                    .col(
                        ColumnDef::new(ConnectorApprovals::SessionId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::ConnectorId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::ToolName)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::SchemaHash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::ArgsHash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::BindingHash)
                            .string()
                            .not_null(),
                    )
                    // What the tool said of itself when asked; decides the expiry.
                    .col(
                        ColumnDef::new(ConnectorApprovals::Risk)
                            .string()
                            .not_null()
                            .check(Expr::col(ConnectorApprovals::Risk).is_in([
                                "read",
                                "write",
                                "destructive",
                                "unknown",
                            ])),
                    )
                    .col(ColumnDef::new(ConnectorApprovals::Preview).text().null())
                    .col(
                        ColumnDef::new(ConnectorApprovals::Status)
                            .string()
                            .not_null()
                            .check(Expr::col(ConnectorApprovals::Status).is_in([
                                "pending",
                                "approved",
                                "consumed",
                                "denied",
                                "expired",
                                "cancelled",
                                "void",
                            ])),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::VoidReason)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // `host`, `device:<id>`.
                    .col(
                        ColumnDef::new(ConnectorApprovals::DecidedBy)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::DecidedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorApprovals::ConsumedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .check(Expr::cust("(status = 'void') = (void_reason IS NOT NULL)"))
                    .check(Expr::cust(
                        "(status = 'consumed') = (consumed_at IS NOT NULL)",
                    ))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connector_approvals_execution_id")
                            .from(ConnectorApprovals::Table, ConnectorApprovals::ExecutionId)
                            .to(ConnectorExecutions::Table, ConnectorExecutions::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connector_approvals_connector_id")
                            .from(ConnectorApprovals::Table, ConnectorApprovals::ConnectorId)
                            .to(Connectors::Table, Connectors::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // The sweeper finds what is overdue.
        manager
            .create_index(
                Index::create()
                    .name("idx_connector_approvals_status_expires")
                    .table(ConnectorApprovals::Table)
                    .col(ConnectorApprovals::Status)
                    .col(ConnectorApprovals::ExpiresAt)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connector_approvals_session_status")
                    .table(ConnectorApprovals::Table)
                    .col(ConnectorApprovals::SessionId)
                    .col(ConnectorApprovals::Status)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ConnectorApprovals::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ConnectorApprovals {
    Table,
    Id,
    ExecutionId,
    SessionId,
    ConnectorId,
    ToolName,
    SchemaHash,
    ArgsHash,
    BindingHash,
    Risk,
    Preview,
    Status,
    VoidReason,
    CreatedAt,
    ExpiresAt,
    DecidedBy,
    DecidedAt,
    ConsumedAt,
}

#[derive(DeriveIden)]
enum ConnectorExecutions {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Connectors {
    Table,
    Id,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    const NOW: &str = "'2026-11-25T00:00:00Z'";

    fn insert(id: &str, execution: &str, status: &str, void: &str, consumed: &str) -> String {
        format!(
            "INSERT INTO connector_approvals (id, execution_id, session_id, connector_id, \
             tool_name, schema_hash, args_hash, binding_hash, risk, status, void_reason, \
             created_at, expires_at, consumed_at) VALUES ('{id}', '{execution}', 's1', 'c1', \
             'echo', 'sha256:0', 'sha256:1', 'sha256:2', 'write', '{status}', {void}, {NOW}, \
             {NOW}, {consumed})"
        )
    }

    #[tokio::test]
    async fn one_approval_per_call_and_each_end_says_how() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        conn.execute_unprepared(
            "PRAGMA foreign_keys = ON; CREATE TABLE connectors (id TEXT PRIMARY KEY); \
             CREATE TABLE connector_executions (id TEXT PRIMARY KEY); \
             INSERT INTO connectors (id) VALUES ('c1'); \
             INSERT INTO connector_executions (id) VALUES ('e1'), ('e2'), ('e3')",
        )
        .await
        .unwrap();
        Migration.up(&manager).await.expect("run migration up");

        conn.execute_unprepared(&insert("a1", "e1", "pending", "NULL", "NULL"))
            .await
            .expect("pending");
        conn.execute_unprepared(&insert("a2", "e2", "void", "'revoked'", "NULL"))
            .await
            .expect("void, with its reason");
        conn.execute_unprepared(&insert("a3", "e3", "consumed", "NULL", NOW))
            .await
            .expect("consumed, with its time");
        for (id, execution, status, void, consumed, why) in [
            (
                "x1",
                "e1",
                "pending",
                "NULL",
                "NULL",
                "one approval per call",
            ),
            ("x2", "e9", "pending", "NULL", "NULL", "a call that exists"),
            ("x3", "e2", "void", "NULL", "NULL", "a void says why"),
            (
                "x4",
                "e2",
                "consumed",
                "NULL",
                "NULL",
                "a consumed one says when",
            ),
            (
                "x5",
                "e2",
                "pending",
                "'revoked'",
                "NULL",
                "only a void has a reason",
            ),
            ("x6", "e2", "maybe", "NULL", "NULL", "a known status"),
        ] {
            assert!(
                conn.execute_unprepared(&insert(id, execution, status, void, consumed))
                    .await
                    .is_err(),
                "{why}"
            );
        }

        conn.execute_unprepared("DELETE FROM connectors WHERE id = 'c1'")
            .await
            .unwrap();
        let left = conn
            .query_one(sea_orm::Statement::from_string(
                sea_orm::DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS n FROM connector_approvals",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(left, 0, "approvals go with their connector");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("connector_approvals").await.unwrap());
    }
}
