use sea_orm::entity::prelude::*;

/// Zwiim-owned: a stored Zwiim Connector Definition. See
/// `crate::connectors::store`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "connector_definitions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    pub zcd_version: i32,
    pub version: String,
    pub trust: String,
    pub runtime_kind: String,
    #[sea_orm(column_type = "Text")]
    pub definition_json: String,
    pub hash: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
