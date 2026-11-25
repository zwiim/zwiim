use sea_orm::entity::prelude::*;

/// Zwiim-owned: how one tool is treated under one grant. See
/// `crate::connectors::grants`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "grant_tool_policies")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub grant_id: i32,
    #[sea_orm(primary_key, auto_increment = false)]
    pub tool_name: String,
    pub policy: String,
    pub schema_hash: Option<String>,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
