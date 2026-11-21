//! Zwiim-owned: an item's lane is the site it works on now, which after a
//! cross-site hand-off is its outward site (automation-engine spec 6.9). The
//! one-item-tab-per-lane index keys on that, so a handed-off item frees the
//! lane it was found on and holds the one it hands off to.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_playbook_item_one_tab_per_lane")
            .await?;
        db.execute_unprepared(
            "CREATE UNIQUE INDEX idx_playbook_item_one_tab_per_lane \
             ON playbook_item (COALESCE(outward_site, site), account) WHERE status = 'working'",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_playbook_item_one_tab_per_lane")
            .await?;
        db.execute_unprepared(
            "CREATE UNIQUE INDEX idx_playbook_item_one_tab_per_lane \
             ON playbook_item (site, account) WHERE status = 'working'",
        )
        .await?;
        Ok(())
    }
}
