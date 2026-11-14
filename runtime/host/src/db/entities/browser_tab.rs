use sea_orm::entity::prelude::*;

/// Zwiim-owned: one tab of the Host browser. See `crate::browser::tabs`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "browser_tabs")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    /// `session` or `personal`.
    pub owner_kind: String,
    /// The public session id of a session tab.
    pub session_id: Option<String>,
    pub url: String,
    pub title: String,
    pub position: i32,
    pub created_at: DateTimeUtc,
    pub last_active_at: DateTimeUtc,
    /// Not loaded in the engine; reloads from `url` when next used.
    pub discarded: bool,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
