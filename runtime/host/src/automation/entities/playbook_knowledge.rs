use sea_orm::entity::prelude::*;

/// Zwiim-owned: one answer or tip a playbook keeps for a site
/// (automation-engine spec 5.4, 7.3, 7.4), unique per (playbook, site, kind,
/// text).
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "playbook_knowledge")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub playbook_id: i32,
    pub site: String,
    /// `answer` or `tip`.
    pub kind: String,
    /// The question, or the tip.
    pub text: String,
    /// The person's answer; `None` while the question is open.
    pub value: Option<String>,
    /// Answers: `open` or `answered`. Tips: `proposed`, `approved` or
    /// `rejected`.
    pub state: String,
    pub item_id: Option<i32>,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
