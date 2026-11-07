// Modified from upstream Apache-2.0 source; see NOTICE.
use sea_orm::entity::prelude::*;

/// Zwiim-owned: one repository's settings (one row per root folder, made on
/// the first save). `settings` is a JSON object of the keys set; see
/// `crate::repository_settings`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "repository_settings")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    #[sea_orm(unique)]
    pub folder_id: i32,
    #[sea_orm(column_type = "Text")]
    pub settings: String,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::folder::Entity",
        from = "Column::FolderId",
        to = "super::folder::Column::Id"
    )]
    Folder,
}

impl Related<super::folder::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Folder.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
