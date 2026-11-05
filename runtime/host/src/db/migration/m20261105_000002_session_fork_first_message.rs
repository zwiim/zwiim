//! Zwiim-owned: what a forked session's first messages still owe its agent.
//!
//! `pending_transcript` is the transcript file to attach, kept until a turn
//! that carried it (`transcript_sent`) ends well. `notice_owed` is the warning
//! that the files stayed at the source's latest turn. `pending_options` is the
//! JSON `OptionsRequest` of an idle fork whose agent's choices weren't known
//! yet. A link from before this migration owes nothing.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SQLite takes one column per ALTER.
        manager
            .alter_table(
                Table::alter()
                    .table(SessionForks::Table)
                    .add_column(ColumnDef::new(SessionForks::PendingTranscript).string().null())
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SessionForks::Table)
                    .add_column(
                        ColumnDef::new(SessionForks::TranscriptSent)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SessionForks::Table)
                    .add_column(
                        ColumnDef::new(SessionForks::NoticeOwed)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(SessionForks::Table)
                    .add_column(ColumnDef::new(SessionForks::PendingOptions).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for column in [
            SessionForks::PendingOptions,
            SessionForks::NoticeOwed,
            SessionForks::TranscriptSent,
            SessionForks::PendingTranscript,
        ] {
            manager
                .alter_table(
                    Table::alter()
                        .table(SessionForks::Table)
                        .drop_column(column)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum SessionForks {
    Table,
    PendingTranscript,
    TranscriptSent,
    NoticeOwed,
    PendingOptions,
}

#[cfg(test)]
mod tests {
    use super::super::m20261105_000001_session_forks::Migration as Links;
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    /// A link recorded before `up` owes nothing after it; `down` takes the
    /// columns away and leaves the link.
    #[tokio::test]
    async fn links_recorded_before_owe_nothing_and_down_keeps_them() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        Links.up(&manager).await.expect("create the links");
        conn.execute_unprepared(
            "INSERT INTO session_forks \
             (session_id, source_session_id, source_turn_id, created_at) \
             VALUES ('s2', 's1', 'turn-3', '2026-11-05T00:00:00Z')",
        )
        .await
        .expect("a link from before");

        Migration.up(&manager).await.expect("run migration up");
        for column in ["pending_transcript", "transcript_sent", "notice_owed", "pending_options"] {
            assert!(manager.has_column("session_forks", column).await.expect("has column"));
        }
        let owes = conn
            .execute_unprepared(
                "UPDATE session_forks SET session_id = session_id \
                 WHERE pending_transcript IS NULL AND pending_options IS NULL \
                 AND transcript_sent = 0 AND notice_owed = 0",
            )
            .await
            .expect("read the link");
        assert_eq!(owes.rows_affected(), 1);

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_column("session_forks", "notice_owed").await.expect("has column"));
        let links = conn
            .execute_unprepared("UPDATE session_forks SET source_turn_id = source_turn_id")
            .await
            .expect("the link stays");
        assert_eq!(links.rows_affected(), 1);
    }
}
