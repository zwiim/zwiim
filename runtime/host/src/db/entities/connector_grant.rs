use sea_orm::entity::prelude::*;

/// Zwiim-owned: one connection granted to a scope of sessions. See
/// `crate::connectors::grants`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "connector_grants")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub connection_id: String,
    pub workspace_id: Option<String>,
    pub agent: Option<String>,
    pub default_policy: String,
    pub new_tools: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
