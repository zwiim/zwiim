// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: [`PlaybookStore`], the read and write path for playbooks and
//! site knowledge (automation-engine spec 5.2.1, 5.4).
//!
//! Site knowledge is read only through here: the item-ID rule, pauses and
//! logins are shared by every playbook on a site, so one owner keeps their
//! scopes straight (per site, per (site, account), per playbook per site).

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::automation::entities::{playbook, playbook_version, site_knowledge};
use crate::db::error::DbError;

/// The kind column of `site_knowledge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteKnowledgeKind {
    /// Versioned, per site; the account column is empty.
    ItemIdRule,
    /// Per (site, account): the pause state with its cause.
    Pause,
    /// Per (site, account): the sealed login's metadata; the value is a
    /// keychain handle, never a secret.
    Login,
    /// Per (site, account): the person accepted the site's automation risk.
    RiskAccept,
}

impl SiteKnowledgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SiteKnowledgeKind::ItemIdRule => "item_id_rule",
            SiteKnowledgeKind::Pause => "pause",
            SiteKnowledgeKind::Login => "login",
            SiteKnowledgeKind::RiskAccept => "risk_accept",
        }
    }

    /// Site-wide kinds key the account column empty.
    pub fn is_site_wide(self) -> bool {
        matches!(self, SiteKnowledgeKind::ItemIdRule)
    }
}

/// A playbook as the store hands it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playbook {
    pub id: i32,
    pub public_id: String,
    pub name: String,
    pub source: String,
    pub format: i32,
    pub version: i32,
    pub show_me: bool,
    pub freeze_learning: bool,
}

/// The store handle.
#[derive(Debug, Clone)]
pub struct PlaybookStore {
    db: DatabaseConnection,
}

impl PlaybookStore {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Create a playbook and its version 1 together.
    pub async fn create(&self, name: &str, source: &str) -> Result<Playbook, DbError> {
        let now = Utc::now();
        let row = playbook::Entity::insert(playbook::ActiveModel {
            public_id: Set(uuid::Uuid::new_v4().to_string()),
            name: Set(name.to_string()),
            source: Set(source.to_string()),
            format: Set(1),
            version: Set(1),
            // Show-me is locked on in 1a-i (spec 19.3).
            show_me: Set(true),
            freeze_learning: Set(false),
            deleted_at: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        })
        .exec_with_returning(&self.db)
        .await?;
        playbook_version::Entity::insert(playbook_version::ActiveModel {
            playbook_id: Set(row.id),
            version: Set(1),
            source: Set(source.to_string()),
            created_at: Set(now),
            ..Default::default()
        })
        .exec(&self.db)
        .await?;
        Ok(to_playbook(row))
    }

    /// Replace the text: one new version, bumped atomically with the text.
    pub async fn save_source(&self, id: i32, source: &str) -> Result<Playbook, DbError> {
        let current = self.get(id).await?;
        let next = current.version + 1;
        let now = Utc::now();
        playbook_version::Entity::insert(playbook_version::ActiveModel {
            playbook_id: Set(id),
            version: Set(next),
            source: Set(source.to_string()),
            created_at: Set(now),
            ..Default::default()
        })
        .exec(&self.db)
        .await?;
        let mut active: playbook::ActiveModel = playbook::Entity::find_by_id(id)
            .one(&self.db)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("playbook {id}")))?
            .into();
        active.source = Set(source.to_string());
        active.version = Set(next);
        active.updated_at = Set(now);
        Ok(to_playbook(active.update(&self.db).await?))
    }

    pub async fn get(&self, id: i32) -> Result<Playbook, DbError> {
        let row = playbook::Entity::find_by_id(id)
            .filter(playbook::Column::DeletedAt.is_null())
            .one(&self.db)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("playbook {id}")))?;
        Ok(to_playbook(row))
    }

    /// The text a run pinned, regardless of later edits (spec 6.5.8).
    pub async fn pinned_source(&self, id: i32, version: i32) -> Result<String, DbError> {
        playbook_version::Entity::find()
            .filter(playbook_version::Column::PlaybookId.eq(id))
            .filter(playbook_version::Column::Version.eq(version))
            .one(&self.db)
            .await?
            .map(|m| m.source)
            .ok_or_else(|| DbError::NotFound(format!("playbook {id} version {version}")))
    }

    /// Read one site-knowledge value; `None` when never set.
    pub async fn site_value(
        &self,
        site: &str,
        kind: SiteKnowledgeKind,
        account: &str,
    ) -> Result<Option<String>, DbError> {
        let account = if kind.is_site_wide() { "" } else { account };
        let row = site_knowledge::Entity::find_by_id((
            site.to_string(),
            kind.as_str().to_string(),
            account.to_string(),
        ))
        .one(&self.db)
        .await?;
        Ok(row.map(|m| m.value))
    }

    /// Upsert one site-knowledge value; the item-ID rule bumps its version so
    /// existing ledger entries are never re-keyed (spec 6.3.2).
    pub async fn set_site_value(
        &self,
        site: &str,
        kind: SiteKnowledgeKind,
        account: &str,
        value: &str,
    ) -> Result<(), DbError> {
        use sea_orm::sea_query::OnConflict;
        let account = if kind.is_site_wide() { "" } else { account };
        let existing = site_knowledge::Entity::find_by_id((
            site.to_string(),
            kind.as_str().to_string(),
            account.to_string(),
        ))
        .one(&self.db)
        .await?;
        let version = existing.map(|m| m.version + 1).unwrap_or(1);
        site_knowledge::Entity::insert(site_knowledge::ActiveModel {
            site: Set(site.to_string()),
            kind: Set(kind.as_str().to_string()),
            account: Set(account.to_string()),
            value: Set(value.to_string()),
            version: Set(version),
            updated_at: Set(Utc::now()),
        })
        .on_conflict(
            OnConflict::columns([
                site_knowledge::Column::Site,
                site_knowledge::Column::Kind,
                site_knowledge::Column::Account,
            ])
            .update_columns([
                site_knowledge::Column::Value,
                site_knowledge::Column::Version,
                site_knowledge::Column::UpdatedAt,
            ])
            .to_owned(),
        )
        .exec(&self.db)
        .await?;
        Ok(())
    }

    /// Remove one site-knowledge row; `false` when it was not set.
    pub async fn clear_site_value(
        &self,
        site: &str,
        kind: SiteKnowledgeKind,
        account: &str,
    ) -> Result<bool, DbError> {
        let account = if kind.is_site_wide() { "" } else { account };
        let res = site_knowledge::Entity::delete_many()
            .filter(site_knowledge::Column::Site.eq(site))
            .filter(site_knowledge::Column::Kind.eq(kind.as_str()))
            .filter(site_knowledge::Column::Account.eq(account))
            .exec(&self.db)
            .await?;
        Ok(res.rows_affected > 0)
    }

    /// Add an item to the playbook's skip list (spec 6.7.2): discovery will
    /// not find it again for this playbook, but other playbooks still see it.
    pub async fn skip_item(
        &self,
        playbook_id: i32,
        site: &str,
        account: &str,
        item_key: &str,
        reason: &str,
    ) -> Result<(), DbError> {
        use crate::automation::entities::playbook_skip;
        use sea_orm::{sea_query::OnConflict, EntityTrait, Set};
        playbook_skip::Entity::insert(playbook_skip::ActiveModel {
            playbook_id: Set(playbook_id),
            site: Set(site.to_string()),
            account: Set(account.to_string()),
            item_key: Set(item_key.to_string()),
            reason: Set(reason.to_string()),
            created_at: Set(Utc::now()),
        })
        .on_conflict(OnConflict::new().do_nothing().to_owned())
        .exec(&self.db)
        .await?;
        Ok(())
    }

    /// Whether a key is on the playbook's skip list (spec 6.2.4).
    pub async fn is_skipped(
        &self,
        playbook_id: i32,
        site: &str,
        account: &str,
        item_key: &str,
    ) -> Result<bool, DbError> {
        use crate::automation::entities::playbook_skip;
        use sea_orm::EntityTrait;
        let found = playbook_skip::Entity::find_by_id((
            playbook_id,
            site.to_string(),
            account.to_string(),
            item_key.to_string(),
        ))
        .one(&self.db)
        .await?;
        Ok(found.is_some())
    }

    /// Whether the person accepted the risk for this site (spec 14.5): no
    /// run on the site before it.
    pub async fn risk_accepted(&self, site: &str, account: &str) -> Result<bool, DbError> {
        Ok(self
            .site_value(site, SiteKnowledgeKind::RiskAccept, account)
            .await?
            .is_some())
    }
}

fn to_playbook(m: playbook::Model) -> Playbook {
    Playbook {
        id: m.id,
        public_id: m.public_id,
        name: m.name,
        source: m.source,
        format: m.format,
        version: m.version,
        show_me: m.show_me,
        freeze_learning: m.freeze_learning,
    }
}
