// Modified from upstream Apache-2.0 source; see NOTICE.
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// A user-named container for workspace folders, used only to organise the
/// sidebar's "Folders" section. Groups never nest, and only `regular` folders
/// can belong to one — worktree children follow their parent repo instead.
///
/// Hard-deleted (no `deleted_at`): removing a group clears `folder.group_id` on
/// its members and drops the row. See `m20260829_000001_folder_group`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "folder_group")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(column_type = "Text")]
    pub name: String,
    /// One of the shared theme colors, or `"inherit"` (use the app theme). Only
    /// ever tints the group's own header row — members keep their own color.
    #[sea_orm(column_type = "Text")]
    pub color: String,
    /// Position among TOP-LEVEL siblings, sharing one numeric space with the
    /// `sort_order` of ungrouped folders so the two interleave in the sidebar.
    pub sort_order: i32,
    /// Opaque id clients address the group by. Nullable like `folder.workspace_id`
    /// so the column can be added and backfilled; every row has one.
    #[sea_orm(unique)]
    pub public_id: Option<String>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::folder::Entity")]
    Folders,
}

impl Related<super::folder::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Folders.def()
    }
}

#[async_trait::async_trait]
impl ActiveModelBehavior for ActiveModel {
    async fn before_save<C>(mut self, _db: &C, insert: bool) -> Result<Self, DbErr>
    where
        C: ConnectionTrait,
    {
        if insert && self.public_id.is_not_set() {
            self.public_id = sea_orm::ActiveValue::Set(Some(uuid::Uuid::new_v4().to_string()));
        }
        Ok(self)
    }
}
