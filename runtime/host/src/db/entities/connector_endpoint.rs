use sea_orm::entity::prelude::*;

/// Zwiim-owned: one origin a connector may reach. See
/// `crate::connectors::store`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "connector_endpoints")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub connector_id: String,
    pub origin: String,
    pub purpose: String,
    pub credentials: bool,
    pub network_class: String,
    pub source: String,
    pub confirmed_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
