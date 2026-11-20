//! Zwiim-owned: the automation engine's tables (automation-engine spec 5.2).
//!
//! New tables, not the upstream `automation` ones: playbooks, pinned runs,
//! items, steps, the shared never-twice ledger, per-playbook skip list, retry
//! counters, daily cap usage and site knowledge. Enums live in Rust
//! (`crate::automation`); these columns hold their snake_case strings.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Playbook::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Playbook::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Playbook::Name).string().not_null())
                    // The current text, plain and readable (spec 5.3.1).
                    .col(ColumnDef::new(Playbook::Source).text().not_null())
                    .col(
                        ColumnDef::new(Playbook::Format)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    // Bumped on every edit; a run pins the version it started on.
                    .col(
                        ColumnDef::new(Playbook::Version)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    // `show_me` is locked on in 1a-i (spec 19.3).
                    .col(
                        ColumnDef::new(Playbook::ShowMe)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    // The freeze-learning switch (1b); present so 1a rows are stable.
                    .col(
                        ColumnDef::new(Playbook::FreezeLearning)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(Playbook::DeletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Playbook::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Playbook::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PlaybookVersion::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookVersion::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(PlaybookVersion::PlaybookId)
                            .integer()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybookVersion::Version)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PlaybookVersion::Source).text().not_null())
                    .col(
                        ColumnDef::new(PlaybookVersion::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_playbook_version_playbook")
                    .table(PlaybookVersion::Table)
                    .col(PlaybookVersion::PlaybookId)
                    .col(PlaybookVersion::Version)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PlaybookRun::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookRun::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PlaybookRun::PlaybookId).integer().not_null())
                    // The pinned version (spec 6.5.8).
                    .col(
                        ColumnDef::new(PlaybookRun::PlaybookVersion)
                            .integer()
                            .not_null(),
                    )
                    // `mac` | `phone` | `schedule` | `agent`
                    .col(ColumnDef::new(PlaybookRun::Trigger).string().not_null())
                    // `discovering` | `running` | `paused` | `done` | `stopped` | `failed`
                    .col(ColumnDef::new(PlaybookRun::Status).string().not_null())
                    .col(ColumnDef::new(PlaybookRun::PauseReason).string().null())
                    // `complete` | `needs_person`; set when the run ends (6.5.5).
                    .col(ColumnDef::new(PlaybookRun::Outcome).string().null())
                    .col(ColumnDef::new(PlaybookRun::ItemLimit).integer().null())
                    .col(
                        ColumnDef::new(PlaybookRun::ItemsTotal)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(PlaybookRun::ItemsDone)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    // `{code, message, context}` (6.5.6); only on `failed`.
                    .col(ColumnDef::new(PlaybookRun::Error).text().null())
                    .col(
                        ColumnDef::new(PlaybookRun::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybookRun::StartedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PlaybookRun::EndedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // One active run per playbook (spec 6.5.7). Active = discovering,
        // running or paused; SQLite enforces the partial unique index.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX idx_playbook_run_one_active \
                 ON playbook_run (playbook_id) \
                 WHERE status IN ('discovering', 'running', 'paused')",
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_playbook_run_playbook")
                    .table(PlaybookRun::Table)
                    .col(PlaybookRun::PlaybookId)
                    .col(PlaybookRun::CreatedAt)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PlaybookItem::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookItem::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PlaybookItem::RunId).integer().not_null())
                    .col(
                        ColumnDef::new(PlaybookItem::PlaybookId)
                            .integer()
                            .not_null(),
                    )
                    // The normalized item key (spec 6.3) and its site/account.
                    .col(ColumnDef::new(PlaybookItem::Site).string().not_null())
                    .col(ColumnDef::new(PlaybookItem::Account).string().not_null())
                    .col(ColumnDef::new(PlaybookItem::ItemKey).string().not_null())
                    .col(ColumnDef::new(PlaybookItem::Title).string().null())
                    .col(ColumnDef::new(PlaybookItem::Url).text().null())
                    // The outward target (spec 6.7.3); null for on-site actions.
                    .col(ColumnDef::new(PlaybookItem::OutwardSite).string().null())
                    .col(ColumnDef::new(PlaybookItem::OutwardUrl).text().null())
                    // `queued` | `working` | ... (spec 6.4); see automation::ItemStatus.
                    .col(ColumnDef::new(PlaybookItem::Status).string().not_null())
                    .col(ColumnDef::new(PlaybookItem::ReasonCode).string().null())
                    // Attempt epoch; a stale-epoch tool call is refused (6.6.7).
                    .col(
                        ColumnDef::new(PlaybookItem::Epoch)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    // Set inside the dispatch transaction (6.6.1); never cleared.
                    .col(
                        ColumnDef::new(PlaybookItem::OutwardStartedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(PlaybookItem::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(PlaybookItem::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // One item tab per (site, account) lane across all runs (spec 14.1).
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX idx_playbook_item_one_tab_per_lane \
                 ON playbook_item (site, account) WHERE status = 'working'",
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_playbook_item_run")
                    .table(PlaybookItem::Table)
                    .col(PlaybookItem::RunId)
                    .col(PlaybookItem::Status)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(PlaybookStep::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookStep::Id)
                            .integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(PlaybookStep::ItemId).integer().not_null())
                    .col(ColumnDef::new(PlaybookStep::Seq).integer().not_null())
                    // `navigated` | `clicked` | `typed` | `selected` | `uploaded` |
                    // `checked` | `declared` | `dispatched` | `checked_after`
                    .col(ColumnDef::new(PlaybookStep::Kind).string().not_null())
                    // Target descriptor JSON: role, name, text, attributes, url pattern.
                    .col(ColumnDef::new(PlaybookStep::Target).text().null())
                    .col(ColumnDef::new(PlaybookStep::Outcome).string().null())
                    // `none` | `matched` | `healed` (1b).
                    .col(ColumnDef::new(PlaybookStep::RouteMatch).string().null())
                    .col(ColumnDef::new(PlaybookStep::ScreenshotPath).text().null())
                    .col(
                        ColumnDef::new(PlaybookStep::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_playbook_step_item")
                    .table(PlaybookStep::Table)
                    .col(PlaybookStep::ItemId)
                    .col(PlaybookStep::Seq)
                    .to_owned(),
            )
            .await?;

        // The shared never-twice ledger (spec 6.7.1). `reserved` or `done`;
        // the primary key is the whole key so a reservation is one INSERT.
        manager
            .create_table(
                Table::create()
                    .table(NeverTwiceLedger::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(NeverTwiceLedger::Site).string().not_null())
                    .col(
                        ColumnDef::new(NeverTwiceLedger::Account)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(NeverTwiceLedger::ActionKind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(NeverTwiceLedger::ItemKey)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(NeverTwiceLedger::State).string().not_null())
                    .col(ColumnDef::new(NeverTwiceLedger::RoundId).integer().null())
                    .col(ColumnDef::new(NeverTwiceLedger::ItemId).integer().null())
                    .col(
                        ColumnDef::new(NeverTwiceLedger::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(NeverTwiceLedger::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(NeverTwiceLedger::Site)
                            .col(NeverTwiceLedger::Account)
                            .col(NeverTwiceLedger::ActionKind)
                            .col(NeverTwiceLedger::ItemKey),
                    )
                    .to_owned(),
            )
            .await?;

        // The per-playbook skip list (spec 6.7.2). Not found again by this
        // playbook; other playbooks still see the item.
        manager
            .create_table(
                Table::create()
                    .table(PlaybookSkip::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(PlaybookSkip::PlaybookId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(PlaybookSkip::Site).string().not_null())
                    .col(ColumnDef::new(PlaybookSkip::Account).string().not_null())
                    .col(ColumnDef::new(PlaybookSkip::ItemKey).string().not_null())
                    // `rule` | `approval_rejected` | `person` | `dropped`
                    .col(ColumnDef::new(PlaybookSkip::Reason).string().not_null())
                    .col(
                        ColumnDef::new(PlaybookSkip::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(PlaybookSkip::PlaybookId)
                            .col(PlaybookSkip::Site)
                            .col(PlaybookSkip::Account)
                            .col(PlaybookSkip::ItemKey),
                    )
                    .to_owned(),
            )
            .await?;

        // Retry counters live outside the per-run items table (spec 6.7.4).
        manager
            .create_table(
                Table::create()
                    .table(ItemRetryCounter::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ItemRetryCounter::PlaybookId)
                            .integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ItemRetryCounter::Site).string().not_null())
                    .col(
                        ColumnDef::new(ItemRetryCounter::Account)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ItemRetryCounter::ItemKey)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ItemRetryCounter::Failures)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(ItemRetryCounter::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(ItemRetryCounter::PlaybookId)
                            .col(ItemRetryCounter::Site)
                            .col(ItemRetryCounter::Account)
                            .col(ItemRetryCounter::ItemKey),
                    )
                    .to_owned(),
            )
            .await?;

        // Daily cap usage per (site, account, action kind, local day). The
        // day is the local calendar date, so a DST change cannot merge days.
        manager
            .create_table(
                Table::create()
                    .table(CapUsage::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(CapUsage::Site).string().not_null())
                    .col(ColumnDef::new(CapUsage::Account).string().not_null())
                    .col(ColumnDef::new(CapUsage::ActionKind).string().not_null())
                    // `YYYY-MM-DD` in the Host's local time zone (6.7.7).
                    .col(ColumnDef::new(CapUsage::LocalDay).string().not_null())
                    .col(
                        ColumnDef::new(CapUsage::Used)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .primary_key(
                        Index::create()
                            .col(CapUsage::Site)
                            .col(CapUsage::Account)
                            .col(CapUsage::ActionKind)
                            .col(CapUsage::LocalDay),
                    )
                    .to_owned(),
            )
            .await?;

        // Site knowledge scopes (spec 5.4): item-ID rule per site, and per
        // (site, account) pause + login state.
        manager
            .create_table(
                Table::create()
                    .table(SiteKnowledge::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(SiteKnowledge::Site).string().not_null())
                    // `item_id_rule` | `pause` | `login` | `risk_accept`
                    .col(ColumnDef::new(SiteKnowledge::Kind).string().not_null())
                    .col(ColumnDef::new(SiteKnowledge::Account).string().not_null())
                    .col(ColumnDef::new(SiteKnowledge::Value).text().not_null())
                    .col(
                        ColumnDef::new(SiteKnowledge::Version)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(SiteKnowledge::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(SiteKnowledge::Site)
                            .col(SiteKnowledge::Kind)
                            .col(SiteKnowledge::Account),
                    )
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SiteKnowledge::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(CapUsage::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ItemRetryCounter::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PlaybookSkip::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(NeverTwiceLedger::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PlaybookStep::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PlaybookItem::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PlaybookRun::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(PlaybookVersion::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Playbook::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Playbook {
    Table,
    Id,
    Name,
    Source,
    Format,
    Version,
    ShowMe,
    FreezeLearning,
    DeletedAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PlaybookVersion {
    Table,
    Id,
    PlaybookId,
    Version,
    Source,
    CreatedAt,
}

#[derive(DeriveIden)]
enum PlaybookRun {
    Table,
    Id,
    PlaybookId,
    PlaybookVersion,
    Trigger,
    Status,
    PauseReason,
    Outcome,
    ItemLimit,
    ItemsTotal,
    ItemsDone,
    Error,
    CreatedAt,
    StartedAt,
    EndedAt,
}

#[derive(DeriveIden)]
enum PlaybookItem {
    Table,
    Id,
    RunId,
    PlaybookId,
    Site,
    Account,
    ItemKey,
    Title,
    Url,
    OutwardSite,
    OutwardUrl,
    Status,
    ReasonCode,
    Epoch,
    OutwardStartedAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PlaybookStep {
    Table,
    Id,
    ItemId,
    Seq,
    Kind,
    Target,
    Outcome,
    RouteMatch,
    ScreenshotPath,
    CreatedAt,
}

#[derive(DeriveIden)]
enum NeverTwiceLedger {
    Table,
    Site,
    Account,
    ActionKind,
    ItemKey,
    State,
    RoundId,
    ItemId,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum PlaybookSkip {
    Table,
    PlaybookId,
    Site,
    Account,
    ItemKey,
    Reason,
    CreatedAt,
}

#[derive(DeriveIden)]
enum ItemRetryCounter {
    Table,
    PlaybookId,
    Site,
    Account,
    ItemKey,
    Failures,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum CapUsage {
    Table,
    Site,
    Account,
    ActionKind,
    LocalDay,
    Used,
}

#[derive(DeriveIden)]
enum SiteKnowledge {
    Table,
    Site,
    Kind,
    Account,
    Value,
    Version,
    UpdatedAt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::Database;

    #[tokio::test]
    async fn one_active_run_per_playbook_and_one_item_tab_per_lane() {
        let conn = Database::connect("sqlite::memory:")
            .await
            .expect("open in-memory sqlite");
        let manager = SchemaManager::new(&conn);
        Migration.up(&manager).await.expect("run migration up");

        // Two active runs on one playbook: refused. A finished one is fine.
        let run = "INSERT INTO playbook_run \
            (playbook_id, playbook_version, trigger, status, items_total, items_done, created_at) \
            VALUES (1, 1, 'mac', 'running', 0, 0, '2026-11-19T00:00:00Z')";
        conn.execute_unprepared(run).await.expect("first run");
        assert!(conn.execute_unprepared(run).await.is_err());
        let paused = run.replace("'running'", "'paused'");
        assert!(conn.execute_unprepared(&paused).await.is_err());
        let done = run.replace("'running'", "'done'");
        conn.execute_unprepared(&done).await.expect("finished run");

        // One working item per (site, account) lane.
        let item = "INSERT INTO playbook_item \
            (run_id, playbook_id, site, account, item_key, status, epoch, created_at, updated_at) \
            VALUES (1, 1, 'linkedin.com', 'main', 'k', 'working', 0, \
            '2026-11-19T00:00:00Z', '2026-11-19T00:00:00Z')";
        conn.execute_unprepared(item).await.expect("first item");
        let other_key = item.replace("'k'", "'k2'");
        assert!(conn.execute_unprepared(&other_key).await.is_err());
        let other_site = other_key.replace("'linkedin.com'", "'indeed.com'");
        conn.execute_unprepared(&other_site)
            .await
            .expect("other lane");

        // A never-twice key keeps one row.
        let key = "INSERT INTO never_twice_ledger \
            (site, account, action_kind, item_key, state, created_at, updated_at) \
            VALUES ('indeed.com', 'main', 'apply', 'k', 'reserved', \
            '2026-11-19T00:00:00Z', '2026-11-19T00:00:00Z')";
        conn.execute_unprepared(key).await.expect("first key");
        assert!(conn.execute_unprepared(key).await.is_err());

        Migration.down(&manager).await.expect("run migration down");
        assert!(!manager.has_table("playbook_run").await.expect("has table"));
    }
}
