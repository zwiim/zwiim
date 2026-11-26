//! Zwiim-owned: one row per tool call an agent made through a connector
//! (connector design r3 §4.3, §8.1, §9). The audit trail: which tool, what
//! happened to the call, and how long Zwiim and the service took.
//!
//! Arguments are never stored: only a digest and their top-level keys.
//! `dispatched_at` is set exactly when the call reached the service, so it
//! tells "nothing was sent" from "may have run".

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Every state of r3 §4.3. Approval states are written once approvals land.
const STATES: [&str; 12] = [
    "received",
    "rejected",
    "awaiting_approval",
    "approval_denied",
    "approval_expired",
    "approval_void",
    "cancelled_before_dispatch",
    "dispatched",
    "succeeded",
    "tool_error",
    "failed",
    "outcome_unknown",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConnectorExecutions::Table)
                    .col(
                        ColumnDef::new(ConnectorExecutions::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::ConnectorId)
                            .string()
                            .not_null(),
                    )
                    // The session that made the call; null until proxy
                    // tokens carry one.
                    .col(
                        ColumnDef::new(ConnectorExecutions::SessionId)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::ToolName)
                            .string()
                            .not_null(),
                    )
                    // The tool's hash when called; null if it wasn't listed.
                    .col(
                        ColumnDef::new(ConnectorExecutions::SchemaHash)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::State)
                            .string()
                            .not_null()
                            .check(Expr::col(ConnectorExecutions::State).is_in(STATES)),
                    )
                    // Why it was rejected, or why its outcome is unknown.
                    .col(ColumnDef::new(ConnectorExecutions::Reason).string().null())
                    // The line the agent was given, redacted.
                    .col(ColumnDef::new(ConnectorExecutions::Detail).text().null())
                    // `policy`, `session_grant`, `device:<id>`, `host`, `app`.
                    .col(
                        ColumnDef::new(ConnectorExecutions::DecidedBy)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::ArgsDigest)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::ArgKeysJson)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::ReceivedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::DispatchedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::FinishedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::UpstreamMs)
                            .big_integer()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ConnectorExecutions::OverheadMs)
                            .big_integer()
                            .null(),
                    )
                    // A call that reached the service, and only such a call,
                    // has `dispatched_at`.
                    .check(Expr::cust(
                        "(state IN ('dispatched', 'succeeded', 'tool_error', 'failed', \
                         'outcome_unknown')) = (dispatched_at IS NOT NULL)",
                    ))
                    .check(Expr::cust(
                        "state <> 'outcome_unknown' OR COALESCE(reason, '') IN ('timeout', \
                         'cancelled_after_dispatch', 'connection_lost', 'host_shutdown')",
                    ))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_connector_executions_connector_id")
                            .from(ConnectorExecutions::Table, ConnectorExecutions::ConnectorId)
                            .to(Connectors::Table, Connectors::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // The boot sweep finds unfinished calls by state.
        manager
            .create_index(
                Index::create()
                    .name("idx_connector_executions_state")
                    .table(ConnectorExecutions::Table)
                    .col(ConnectorExecutions::State)
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_connector_executions_connector")
                    .table(ConnectorExecutions::Table)
                    .col(ConnectorExecutions::ConnectorId)
                    .col(ConnectorExecutions::ReceivedAt)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ConnectorExecutions::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum ConnectorExecutions {
    Table,
    Id,
    ConnectorId,
    SessionId,
    ToolName,
    SchemaHash,
    State,
    Reason,
    Detail,
    DecidedBy,
    ArgsDigest,
    ArgKeysJson,
    ReceivedAt,
    DispatchedAt,
    FinishedAt,
    UpstreamMs,
    OverheadMs,
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

    fn insert(id: &str, state: &str, reason: &str, dispatched_at: &str) -> String {
        format!(
            "INSERT INTO connector_executions (id, connector_id, tool_name, state, reason, \
             args_digest, arg_keys_json, received_at, dispatched_at) VALUES ('{id}', 'c1', \
             'echo', '{state}', {reason}, 'sha256:0', '[]', {NOW}, {dispatched_at})"
        )
    }

    #[tokio::test]
    async fn only_a_call_that_reached_the_service_has_dispatched_at() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        conn.execute_unprepared(
            "PRAGMA foreign_keys = ON; CREATE TABLE connectors (id TEXT PRIMARY KEY); \
             INSERT INTO connectors (id) VALUES ('c1')",
        )
        .await
        .unwrap();
        Migration.up(&manager).await.expect("run migration up");

        conn.execute_unprepared(&insert("e1", "received", "NULL", "NULL"))
            .await
            .expect("received, not sent");
        conn.execute_unprepared(&insert("e2", "succeeded", "NULL", NOW))
            .await
            .expect("sent and answered");
        conn.execute_unprepared(&insert("e3", "outcome_unknown", "'timeout'", NOW))
            .await
            .expect("sent, no answer");
        for (id, state, reason, dispatched_at, why) in [
            (
                "x1",
                "succeeded",
                "NULL",
                "NULL",
                "an answer needs a dispatch",
            ),
            (
                "x2",
                "rejected",
                "'unavailable'",
                NOW,
                "a rejected call wasn't sent",
            ),
            (
                "x3",
                "outcome_unknown",
                "NULL",
                NOW,
                "an unknown outcome says why",
            ),
            ("x4", "outcome_unknown", "'bored'", NOW, "a known cause"),
            ("x5", "finished", "NULL", "NULL", "a known state"),
        ] {
            assert!(
                conn.execute_unprepared(&insert(id, state, reason, dispatched_at))
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
                "SELECT COUNT(*) AS n FROM connector_executions",
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get::<i64>("", "n")
            .unwrap();
        assert_eq!(left, 0, "records go with their connector");

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("connector_executions").await.unwrap());
    }
}
