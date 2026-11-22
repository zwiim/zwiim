// Modified from upstream Apache-2.0 source; see NOTICE.
//! Zwiim-owned: the automation engine's `/zwiim/v1` operations (automation-
//! engine spec 18.2, 11.1, 6.5, 10.2).
//!
//! Prefixes `playbook.*`, `run.*` and `site.*`. The Host answers with the run
//! record, not internal rows: `playbook` names, statuses and reason codes are
//! the client vocabulary (spec 17.6). Errors are `ZwiimError` with the codes in
//! spec 11.2.2. Every operation is behind the `playbooks.enabled` Labs switch.

use std::sync::Arc;

use axum::{extract::Extension, Json};
use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::automation::entities::{playbook, playbook_item, playbook_run};
use crate::automation::pauses::{PauseCause, SitePause};
use crate::automation::playbook_store::{PlaybookStore, SiteKnowledgeKind};
use crate::automation::run_views::RunTotals;
use crate::automation::{AutomationEngine, RunStatus};
use crate::protocol::error::ZwiimError;
use crate::web::auth::Principal;

type ZResult<T> = Result<Json<T>, ZwiimError>;

/// Turn any engine failure into the `ZwiimError` vocabulary (spec 11.2.2).
fn engine_err(e: impl std::fmt::Display) -> ZwiimError {
    {
        tracing::warn!(error = %e, "[zwiim] automation engine error translated to `internal`");
        ZwiimError::internal()
    }
}

/// The engine handle for a request: its writes tell the Host feed (11.4).
fn engine(state: &AppState) -> AutomationEngine {
    AutomationEngine::new(state.db.conn.clone()).with_changes(state.playbook_changes.clone())
}

/// Runs, items or approvals changed outside the engine handle: tell the
/// Host feed to re-read them (spec 11.4.1).
fn changed(state: &AppState) {
    state.playbook_changes.send_modify(|v| *v += 1);
}

async fn require_enabled(db: &sea_orm::DatabaseConnection) -> Result<(), ZwiimError> {
    if crate::settings::playbooks_enabled(db).await {
        Ok(())
    } else {
        Err(ZwiimError::new(
            axum::http::StatusCode::FORBIDDEN,
            crate::automation::ErrorCode::PlaybooksDisabled.as_str(),
            "Playbooks are off. Turn them on in Settings > Labs.",
        ))
    }
}

// ── shapes ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybookSummary {
    pub playbook_id: String,
    pub name: String,
    pub version: i32,
    pub show_me: bool,
    pub freeze_learning: bool,
    /// The sites its text declares, and whether the person accepted each
    /// one's risk; empty while the text does not parse.
    #[serde(default)]
    pub sites: Vec<PlaybookSite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybookSite {
    pub site: String,
    pub risk_accepted: bool,
}

impl From<playbook::Model> for PlaybookSummary {
    fn from(m: playbook::Model) -> Self {
        Self {
            playbook_id: m.public_id,
            name: m.name,
            version: m.version,
            show_me: m.show_me,
            freeze_learning: m.freeze_learning,
            sites: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub playbook_id: String,
    pub playbook_version: i32,
    pub trigger: String,
    pub status: String,
    pub pause_reason: Option<String>,
    pub outcome: Option<String>,
    pub items_total: i32,
    pub items_done: i32,
    /// Why a failed run failed (a reason code), else null.
    pub error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl RunSummary {
    /// `playbook_id` is the run's playbook's public id; the counts come from
    /// the run's items, since only a run that finishes writes them to its row.
    fn new(m: playbook_run::Model, playbook_id: String, totals: &RunTotals) -> Self {
        Self {
            run_id: m.public_id,
            playbook_id,
            playbook_version: m.playbook_version,
            trigger: m.trigger,
            status: m.status,
            pause_reason: m.pause_reason,
            outcome: m.outcome,
            items_total: totals.items as i32,
            items_done: totals.done as i32,
            error: crate::automation::RunError::code_of(m.error.as_deref()),
            created_at: m.created_at,
            ended_at: m.ended_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemSummary {
    pub item_id: String,
    pub site: String,
    pub account: String,
    pub key: String,
    pub title: Option<String>,
    pub url: Option<String>,
    pub status: String,
    pub reason_code: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// The question the item waits on the person for (`run.answer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// The browser tab it works in or is parked on, for a client to show.
    pub tab_id: Option<String>,
    /// Its send may have gone out: the person says whether it did
    /// (`run.resolve`).
    #[serde(default)]
    pub may_have_sent: bool,
}

impl From<playbook_item::Model> for ItemSummary {
    fn from(m: playbook_item::Model) -> Self {
        Self {
            item_id: m.public_id,
            site: m.site,
            account: m.account,
            key: m.item_key,
            title: m.title,
            url: m.url,
            status: m.status,
            reason_code: m.reason_code,
            updated_at: m.updated_at,
            question: None,
            tab_id: m.tab_id,
            may_have_sent: m.outward_started_at.is_some(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemPage {
    pub items: Vec<ItemSummary>,
    /// Pass as `cursor` for the next page; `null` at the end (spec 11.1).
    pub next_cursor: Option<String>,
}

// ── playbook.list ─────────────────────────────────────────────────────────

/// A playbook as clients see it, with its sites read from its text.
async fn summary(state: &AppState, row: playbook::Model) -> Result<PlaybookSummary, ZwiimError> {
    let Ok(doc) = crate::automation::validate_playbook(&row.source) else {
        return Ok(row.into());
    };
    let store = PlaybookStore::new(state.db.conn.clone());
    let mut sites = Vec::with_capacity(doc.sites.len());
    for site in doc.sites {
        let risk_accepted = store
            .risk_accepted(&site, &default_account())
            .await
            .map_err(engine_err)?;
        sites.push(PlaybookSite {
            site,
            risk_accepted,
        });
    }
    Ok(PlaybookSummary {
        sites,
        ..row.into()
    })
}

pub async fn playbook_list(
    Extension(state): Extension<Arc<AppState>>,
) -> ZResult<Vec<PlaybookSummary>> {
    require_enabled(&state.db.conn).await?;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    let rows = playbook::Entity::find()
        .filter(playbook::Column::DeletedAt.is_null())
        .order_by_asc(playbook::Column::Id)
        .all(&state.db.conn)
        .await
        .map_err(engine_err)?;
    let mut books = Vec::with_capacity(rows.len());
    for row in rows {
        books.push(summary(&state, row).await?);
    }
    Ok(Json(books))
}

// ── playbook.get ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybookGetParams {
    pub playbook_id: String,
}

#[derive(Serialize)]
pub struct PlaybookGetResult {
    pub playbook: PlaybookSummary,
    pub source: String,
}

pub async fn playbook_get(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<PlaybookGetParams>,
) -> ZResult<PlaybookGetResult> {
    require_enabled(&state.db.conn).await?;
    let row = playbook_row(&state, &p.playbook_id).await?;
    let source = row.source.clone();
    Ok(Json(PlaybookGetResult {
        playbook: summary(&state, row).await?,
        source,
    }))
}

// ── playbook.save ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybookSaveParams {
    pub playbook_id: String,
    pub source: String,
}

#[derive(Serialize)]
pub struct PlaybookSaveResult {
    pub playbook: PlaybookSummary,
}

/// Replace the playbook's text: one new version (spec 6.5.8). A later run
/// reads the new version; a running run keeps the one it pinned.
/// Validation runs on every create and save, including agent-proposed
/// changes (spec 16.3). Errors are line-anchored under `playbook_invalid`.
fn validated(source: &str) -> Result<crate::automation::PlaybookDoc, ZwiimError> {
    crate::automation::validate_playbook(source).map_err(|errors| {
        let mut err = ZwiimError::new(
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            crate::automation::ErrorCode::PlaybookInvalid.as_str(),
            "The playbook has errors",
        );
        err.details = Some(serde_json::json!(errors));
        err
    })
}

// ── playbook.create ───────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybookCreateParams {
    pub source: String,
}

/// A new playbook from valid text, named by its `name:`, at version 1.
pub async fn playbook_create(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<PlaybookCreateParams>,
) -> ZResult<PlaybookSaveResult> {
    require_enabled(&state.db.conn).await?;
    let doc = validated(&p.source)?;
    let created = PlaybookStore::new(state.db.conn.clone())
        .create(&doc.name, &p.source)
        .await
        .map_err(engine_err)?;
    use sea_orm::EntityTrait;
    let row = playbook::Entity::find_by_id(created.id)
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("playbook {}", created.id)))?;
    Ok(Json(PlaybookSaveResult {
        playbook: summary(&state, row).await?,
    }))
}

pub async fn playbook_save(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<PlaybookSaveParams>,
) -> ZResult<PlaybookSaveResult> {
    require_enabled(&state.db.conn).await?;
    let doc = validated(&p.source)?;
    let book = playbook_row(&state, &p.playbook_id).await?;
    let store = PlaybookStore::new(state.db.conn.clone());
    let saved = store
        .save_source(book.id, &doc.name, &p.source)
        .await
        .map_err(engine_err)?;
    use sea_orm::EntityTrait;
    let row = playbook::Entity::find_by_id(saved.id)
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("playbook {}", saved.id)))?;
    Ok(Json(PlaybookSaveResult {
        playbook: summary(&state, row).await?,
    }))
}

// ── playbook.files / playbook.file.* (recording design §4) ────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybookFilesParams {
    pub playbook_id: String,
}

/// One file input as the person sees it.
#[derive(Debug, Serialize)]
pub struct PlaybookFileView {
    pub input_name: String,
    /// The stored copy's name; `None` for an older `path:` input.
    pub file: Option<String>,
    /// An older input's path on this computer.
    pub path: Option<String>,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    /// When this copy was first saved into the playbook.
    pub added_at: Option<String>,
    /// The steps (by index) that use it.
    pub used_by_steps: Vec<usize>,
}

#[derive(Serialize)]
pub struct PlaybookFilesResult {
    pub files: Vec<PlaybookFileView>,
}

fn input_store(state: &AppState) -> crate::automation::input_files::InputFileStore {
    crate::automation::input_files::InputFileStore::new(&state.data_dir)
}

/// The steps of `doc` that name input `name`.
fn steps_using(doc: &crate::automation::PlaybookDoc, name: &str) -> Vec<usize> {
    doc.steps
        .iter()
        .enumerate()
        .filter(|(_, step)| crate::automation::playbook_format::step_inputs(step).contains(&name))
        .map(|(i, _)| i)
        .collect()
}

/// `playbook.files {playbook_id}`: the playbook's file inputs.
pub async fn playbook_files(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<PlaybookFilesParams>,
) -> ZResult<PlaybookFilesResult> {
    use crate::automation::entities::playbook_version;
    use crate::automation::playbook_format::Input;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    require_enabled(&state.db.conn).await?;
    let book = playbook_row(&state, &p.playbook_id).await?;
    let doc = validated(&book.source)?;
    let versions = playbook_version::Entity::find()
        .filter(playbook_version::Column::PlaybookId.eq(book.id))
        .order_by_asc(playbook_version::Column::Version)
        .all(&state.db.conn)
        .await
        .map_err(engine_err)?;
    let store = input_store(&state);
    let files = doc
        .inputs
        .iter()
        .filter_map(|input| match input {
            Input::File {
                name,
                path,
                file,
                sha256,
            } => Some(PlaybookFileView {
                input_name: name.clone(),
                file: file.clone(),
                path: path.clone(),
                size: match (file, sha256) {
                    (Some(f), Some(h)) => {
                        std::fs::metadata(store.path_of(h, f)).ok().map(|m| m.len())
                    }
                    _ => None,
                },
                sha256: sha256.clone(),
                added_at: sha256.as_ref().and_then(|h| {
                    versions
                        .iter()
                        .find(|v| v.source.contains(h.as_str()))
                        .map(|v| v.created_at.to_rfc3339())
                }),
                used_by_steps: steps_using(&doc, name),
            }),
            _ => None,
        })
        .collect();
    Ok(Json(PlaybookFilesResult { files }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybookFileSetParams {
    pub playbook_id: String,
    pub input_name: String,
    /// A file on this computer; the Host copies it in.
    pub path: String,
}

#[derive(Serialize)]
pub struct StoredFileView {
    pub input_name: String,
    pub file: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Serialize)]
pub struct PlaybookFileSetResult {
    pub playbook: PlaybookSummary,
    pub file: StoredFileView,
}

/// Change the playbook's inputs and save the result as a new version. The
/// text is parsed and written back, so YAML comments do not survive.
async fn save_inputs(
    state: &AppState,
    book: &playbook::Model,
    change: impl FnOnce(&mut Vec<crate::automation::playbook_format::Input>) -> Result<(), ZwiimError>,
) -> Result<PlaybookSummary, ZwiimError> {
    let mut doc = validated(&book.source)?;
    change(&mut doc.inputs)?;
    let source = serde_yaml::to_string(&doc).map_err(engine_err)?;
    let doc = validated(&source)?;
    let store = PlaybookStore::new(state.db.conn.clone());
    let saved = store
        .save_source(book.id, &doc.name, &source)
        .await
        .map_err(engine_err)?;
    use sea_orm::EntityTrait;
    let row = playbook::Entity::find_by_id(saved.id)
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("playbook {}", saved.id)))?;
    summary(state, row).await
}

/// `playbook.file.set {playbook_id, input_name, path}`: copy the file in and
/// add or replace that input (a new version).
pub async fn playbook_file_set(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<PlaybookFileSetParams>,
) -> ZResult<PlaybookFileSetResult> {
    use crate::automation::input_files::StoreError;
    use crate::automation::playbook_format::Input;
    require_enabled(&state.db.conn).await?;
    let name = p.input_name.trim().to_string();
    if name.is_empty() {
        return Err(ZwiimError::invalid_input("Name the input"));
    }
    let book = playbook_row(&state, &p.playbook_id).await?;
    let path = std::path::PathBuf::from(&p.path);
    let stored = {
        let store = input_store(&state);
        tokio::task::spawn_blocking(move || store.put(&path))
            .await
            .map_err(engine_err)?
    }
    .map_err(|e| match e {
        StoreError::TooLarge => ZwiimError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            crate::automation::ErrorCode::FileTooLarge.as_str(),
            "The file is over 50 MB",
        ),
        StoreError::NotAFile => ZwiimError::new(
            axum::http::StatusCode::NOT_FOUND,
            crate::automation::ErrorCode::FileNotFound.as_str(),
            "That file is not on this computer",
        ),
        StoreError::Io(e) => engine_err(e),
    })?;
    let file = Input::File {
        name: name.clone(),
        path: None,
        file: Some(stored.file.clone()),
        sha256: Some(stored.sha256.clone()),
    };
    let playbook = save_inputs(&state, &book, |inputs| {
        match inputs.iter_mut().find(|i| i.name() == name) {
            Some(existing @ Input::File { .. }) => *existing = file,
            Some(_) => {
                return Err(ZwiimError::invalid_input(format!(
                    "`{name}` is not a file input"
                )))
            }
            None => inputs.push(file),
        }
        Ok(())
    })
    .await?;
    Ok(Json(PlaybookFileSetResult {
        playbook,
        file: StoredFileView {
            input_name: name,
            file: stored.file,
            size: stored.size,
            sha256: stored.sha256,
        },
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybookFileRemoveParams {
    pub playbook_id: String,
    pub input_name: String,
}

/// `playbook.file.remove {playbook_id, input_name}`: drop a file input no
/// step uses (a new version). The stored copy stays for older versions.
pub async fn playbook_file_remove(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<PlaybookFileRemoveParams>,
) -> ZResult<PlaybookSaveResult> {
    use crate::automation::playbook_format::Input;
    require_enabled(&state.db.conn).await?;
    let book = playbook_row(&state, &p.playbook_id).await?;
    let doc = validated(&book.source)?;
    if !steps_using(&doc, &p.input_name).is_empty() {
        return Err(ZwiimError::new(
            axum::http::StatusCode::CONFLICT,
            crate::automation::ErrorCode::InputInUse.as_str(),
            "A step still uses this file. Remove that step or replace the file.",
        ));
    }
    let name = p.input_name.clone();
    let playbook = save_inputs(&state, &book, |inputs| {
        let before = inputs.len();
        inputs.retain(|i| !(matches!(i, Input::File { .. }) && i.name() == name));
        if inputs.len() == before {
            return Err(ZwiimError::not_found(format!("file input {name}")));
        }
        Ok(())
    })
    .await?;
    Ok(Json(PlaybookSaveResult { playbook }))
}

// ── run.start ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunStartParams {
    pub playbook_id: String,
    /// `mac`, `phone`, `schedule` or `agent`.
    pub trigger: Option<String>,
    /// 1 for "Try on one item".
    pub item_limit: Option<i32>,
}

#[derive(Serialize)]
pub struct RunStartResult {
    pub run_id: String,
}

/// "Try on one item": a run with item limit 1 under all the normal rules
/// (spec 6.5.10, 16.5). The ledger is touched only if the item actually goes
/// out, which the dispatch transaction already guarantees.
pub async fn run_try(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    axum::extract::Json(p): axum::extract::Json<RunTryParams>,
) -> ZResult<RunStartResult> {
    start_run_inner(&state, principal, &p.playbook_id, None, Some(1)).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunTryParams {
    pub playbook_id: String,
}

pub async fn run_start(
    Extension(state): Extension<Arc<AppState>>,
    principal: Option<Extension<Principal>>,
    axum::extract::Json(p): axum::extract::Json<RunStartParams>,
) -> ZResult<RunStartResult> {
    start_run_inner(&state, principal, &p.playbook_id, p.trigger, p.item_limit).await
}

/// The trigger a start is allowed from this caller (spec 6.1, 19.3). The
/// Host derives it from the authenticated `Principal`; a client only ever
/// says whether it means a start it already has the right to make.
fn permitted_trigger(
    principal: Option<Extension<Principal>>,
    requested: Option<&str>,
) -> Result<crate::automation::RunTrigger, ZwiimError> {
    use crate::automation::RunTrigger;
    // `agent` and `schedule` are Host-owned: a client never asks for them.
    if let Some(t) = requested {
        if t != "mac" {
            return Err(ZwiimError::invalid_input(
                "Only `mac` may be requested; other triggers are set by the Host",
            ));
        }
    }
    match principal.map(|Extension(p)| p) {
        // 1a-i starts from the Mac only: phone and schedule arrive in 1a-ii.
        Some(Principal::Host) => Ok(RunTrigger::Mac),
        Some(Principal::Device(_)) => Err(ZwiimError::new(
            axum::http::StatusCode::CONFLICT,
            crate::automation::ErrorCode::RunStartRequiresMac.as_str(),
            "In this build runs start from the Mac",
        )),
        // No principal means the auth layer is not in play (a direct Host
        // call); treat it as the Mac.
        None => Ok(RunTrigger::Mac),
    }
}

async fn start_run_inner(
    state: &AppState,
    principal: Option<Extension<Principal>>,
    playbook_id: &str,
    requested_trigger: Option<String>,
    item_limit: Option<i32>,
) -> ZResult<RunStartResult> {
    require_enabled(&state.db.conn).await?;
    let trigger = permitted_trigger(principal, requested_trigger.as_deref())?;
    let store = PlaybookStore::new(state.db.conn.clone());
    let book = playbook_row(state, playbook_id).await?;
    let book = store.get(book.id).await.map_err(engine_err)?;
    // Nothing runs on a site before the person accepts its risk (spec
    // 14.5). Phase 1 has one account per site (14.6).
    if let Ok(doc) = crate::automation::validate_playbook(&book.source) {
        let mut pending = Vec::new();
        for site in &doc.sites {
            if !store
                .risk_accepted(site, &default_account())
                .await
                .map_err(engine_err)?
            {
                pending.push(site.clone());
            }
        }
        if !pending.is_empty() {
            let mut err = ZwiimError::new(
                axum::http::StatusCode::CONFLICT,
                crate::automation::ErrorCode::SiteRiskNotAccepted.as_str(),
                "Accept the risk on each of the playbook's sites first",
            );
            err.details = Some(serde_json::json!({ "sites": pending }));
            return Err(err);
        }
    }
    let engine = engine(state);
    // Read the active run first so `run_active` can name it.
    let active = active_run_id(state, book.id).await;
    let run = engine
        .start_run(book.id, book.version, trigger, item_limit)
        .await
        .map_err(|e| match e {
            crate::automation::StartRunError::RunActive => {
                let mut err = ZwiimError::new(
                    axum::http::StatusCode::CONFLICT,
                    crate::automation::ErrorCode::RunActive.as_str(),
                    "This playbook already has an active run",
                );
                err.details = Some(serde_json::json!({ "run_id": active }));
                err
            }
            other => engine_err(other),
        })?;
    let run = run_by_row(state, run).await?;
    Ok(Json(RunStartResult {
        run_id: run.public_id,
    }))
}

async fn active_run_id(state: &AppState, playbook_id: i32) -> Option<String> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    playbook_run::Entity::find()
        .filter(playbook_run::Column::PlaybookId.eq(playbook_id))
        .filter(playbook_run::Column::Status.is_in(["discovering", "running", "paused"]))
        .one(&state.db.conn)
        .await
        .ok()
        .flatten()
        .map(|m| m.public_id)
}

/// The playbook a client names, unless deleted.
async fn playbook_row(state: &AppState, public_id: &str) -> Result<playbook::Model, ZwiimError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    playbook::Entity::find()
        .filter(playbook::Column::PublicId.eq(public_id))
        .filter(playbook::Column::DeletedAt.is_null())
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("playbook {public_id}")))
}

/// The run a client names.
async fn run_row(state: &AppState, public_id: &str) -> Result<playbook_run::Model, ZwiimError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    playbook_run::Entity::find()
        .filter(playbook_run::Column::PublicId.eq(public_id))
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("run {public_id}")))
}

async fn run_by_row(state: &AppState, run_id: i32) -> Result<playbook_run::Model, ZwiimError> {
    use sea_orm::EntityTrait;
    playbook_run::Entity::find_by_id(run_id)
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found("run"))
}

// ── run.get ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunGetParams {
    pub run_id: String,
}

pub async fn run_get(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<RunGetParams>,
) -> ZResult<RunSummary> {
    require_enabled(&state.db.conn).await?;
    let run = run_row(&state, &p.run_id).await?;
    load_run(&state, run.id).await
}

/// The run as clients see it, without the Labs gate: Pause and Stop answer
/// with it even when Playbooks are off (spec 10.12.2).
async fn load_run(state: &AppState, run_id: i32) -> ZResult<RunSummary> {
    use sea_orm::EntityTrait;
    let row = run_by_row(state, run_id).await?;
    let playbook_id = playbook::Entity::find_by_id(row.playbook_id)
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found("playbook"))?
        .public_id;
    let totals = crate::automation::run_views::totals_of(&state.db.conn, &[row.id])
        .await
        .map_err(engine_err)?
        .remove(&row.id)
        .unwrap_or_default();
    Ok(Json(RunSummary::new(row, playbook_id, &totals)))
}

// ── run.items (paginated, spec 11.1) ──────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunItemsParams {
    pub run_id: String,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

const DEFAULT_PAGE: i64 = 50;
const MAX_PAGE: i64 = 200;

pub async fn run_items(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<RunItemsParams>,
) -> ZResult<ItemPage> {
    require_enabled(&state.db.conn).await?;
    // An unknown run is `not_found`, not an empty page (spec 11.1).
    let run = run_row(&state, &p.run_id).await?;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
    let limit = p.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE);
    let mut query = playbook_item::Entity::find()
        .filter(playbook_item::Column::RunId.eq(run.id))
        .order_by_asc(playbook_item::Column::Id)
        // One extra row tells us whether a next page exists.
        .limit(limit as u64 + 1);
    if let Some(cursor) = p.cursor {
        // The cursor is the last item of the previous page.
        let after = playbook_item::Entity::find()
            .filter(playbook_item::Column::RunId.eq(run.id))
            .filter(playbook_item::Column::PublicId.eq(cursor))
            .one(&state.db.conn)
            .await
            .map_err(engine_err)?
            .ok_or_else(|| ZwiimError::invalid_input("Unknown cursor"))?;
        query = query.filter(playbook_item::Column::Id.gt(after.id));
    }
    let mut rows = query.all(&state.db.conn).await.map_err(engine_err)?;
    let next_cursor = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|m| m.public_id.clone())
    } else {
        None
    };
    let ids: Vec<i32> = rows.iter().map(|m| m.id).collect();
    let mut questions = crate::automation::knowledge::open_questions(&state.db.conn, &ids)
        .await
        .map_err(engine_err)?;
    Ok(Json(ItemPage {
        items: rows
            .into_iter()
            .map(|m| {
                let question = questions.remove(&m.id).map(|(_, text)| text);
                ItemSummary {
                    question,
                    ..m.into()
                }
            })
            .collect(),
        next_cursor,
    }))
}

// ── run.list (past runs, newest first) ────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunListParams {
    /// Only this playbook's runs; every playbook's when absent.
    pub playbook_id: Option<String>,
    /// The `next_cursor` of the previous page.
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct RunPage {
    pub runs: Vec<RunSummary>,
    pub next_cursor: Option<String>,
}

const DEFAULT_RUN_PAGE: i64 = 20;
const MAX_RUN_PAGE: i64 = 100;

pub async fn run_list(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<RunListParams>,
) -> ZResult<RunPage> {
    require_enabled(&state.db.conn).await?;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
    let limit = p.limit.unwrap_or(DEFAULT_RUN_PAGE).clamp(1, MAX_RUN_PAGE);
    // Row ids order runs as they were created, ties included.
    let mut query = playbook_run::Entity::find()
        .order_by_desc(playbook_run::Column::Id)
        .limit(limit as u64 + 1);
    if let Some(id) = &p.playbook_id {
        let book = playbook_row(&state, id).await?;
        query = query.filter(playbook_run::Column::PlaybookId.eq(book.id));
    }
    if let Some(cursor) = p.cursor {
        // The cursor is the last run of the previous page.
        let after = playbook_run::Entity::find()
            .filter(playbook_run::Column::PublicId.eq(cursor))
            .one(&state.db.conn)
            .await
            .map_err(engine_err)?
            .ok_or_else(|| ZwiimError::invalid_input("Unknown cursor"))?;
        query = query.filter(playbook_run::Column::Id.lt(after.id));
    }
    let mut rows = query.all(&state.db.conn).await.map_err(engine_err)?;
    let next_cursor = if rows.len() as i64 > limit {
        rows.truncate(limit as usize);
        rows.last().map(|m| m.public_id.clone())
    } else {
        None
    };
    let ids: Vec<i32> = rows.iter().map(|m| m.id).collect();
    let mut totals = crate::automation::run_views::totals_of(&state.db.conn, &ids)
        .await
        .map_err(engine_err)?;
    let books: std::collections::HashMap<i32, String> = playbook::Entity::find()
        .filter(playbook::Column::Id.is_in(rows.iter().map(|r| r.playbook_id)))
        .all(&state.db.conn)
        .await
        .map_err(engine_err)?
        .into_iter()
        .map(|b| (b.id, b.public_id))
        .collect();
    let runs = rows
        .into_iter()
        .map(|m| {
            let t = totals.remove(&m.id).unwrap_or_default();
            let book = books.get(&m.playbook_id).cloned().unwrap_or_default();
            RunSummary::new(m, book, &t)
        })
        .collect();
    Ok(Json(RunPage { runs, next_cursor }))
}

// ── run.answer (an item's question, spec 7.3) ─────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunAnswerParams {
    pub item_id: String,
    pub answer: String,
}

#[derive(Serialize)]
pub struct RunAnswerResult {
    pub item_id: String,
    /// The item went back to the queue; false when it had moved on.
    pub requeued: bool,
}

/// The person answered the question an item waits on ("Save and retry"):
/// the answer is kept for every later item on the site, and the item goes
/// back to the queue.
pub async fn run_answer(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<RunAnswerParams>,
) -> ZResult<RunAnswerResult> {
    require_enabled(&state.db.conn).await?;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    if p.answer.trim().is_empty() {
        return Err(ZwiimError::invalid_input("The answer is empty"));
    }
    let item = playbook_item::Entity::find()
        .filter(playbook_item::Column::PublicId.eq(&p.item_id))
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("item {}", p.item_id)))?;
    let open = crate::automation::knowledge::open_questions(&state.db.conn, &[item.id])
        .await
        .map_err(engine_err)?;
    let Some((question, _)) = open.get(&item.id) else {
        return Err(ZwiimError::new(
            axum::http::StatusCode::CONFLICT,
            crate::automation::ErrorCode::NoOpenQuestion.as_str(),
            "This item is not waiting on a question",
        ));
    };
    let requeued = crate::automation::knowledge::answer(&engine(&state), *question, &p.answer)
        .await
        .map_err(engine_err)?;
    changed(&state);
    Ok(Json(RunAnswerResult {
        item_id: p.item_id,
        requeued,
    }))
}

// ── run.resolve (an item waiting on the person, spec 8.9) ─────────────────

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResolveChoice {
    /// "It was sent": the item is done and keeps its slot.
    Sent,
    /// "Not sent, redo" (or "Try again" when nothing went out): back to
    /// the queue, its keys and slot released.
    NotSent,
    /// "Skip": skipped and on the playbook's skip list; its keys stay taken.
    Skip,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunResolveParams {
    pub item_id: String,
    pub choice: ResolveChoice,
}

#[derive(Serialize)]
pub struct RunResolveResult {
    pub item_id: String,
    pub status: String,
}

/// The person settled an item waiting on them (spec 8.9 "May have been
/// sent", and a page that needed them). The first answer wins; a later one
/// finds it `item_not_waiting`.
pub async fn run_resolve(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<RunResolveParams>,
) -> ZResult<RunResolveResult> {
    require_enabled(&state.db.conn).await?;
    use crate::automation::engine::LedgerKey;
    use crate::automation::entities::never_twice_ledger;
    use crate::automation::states::ItemStatus;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let not_waiting = || {
        ZwiimError::new(
            axum::http::StatusCode::CONFLICT,
            crate::automation::ErrorCode::ItemNotWaiting.as_str(),
            "This item is not waiting on you",
        )
    };
    let item = playbook_item::Entity::find()
        .filter(playbook_item::Column::PublicId.eq(&p.item_id))
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("item {}", p.item_id)))?;
    if item.status != ItemStatus::WaitingPerson.as_str() {
        return Err(not_waiting());
    }
    let sent = item.outward_started_at.is_some();
    if p.choice == ResolveChoice::Sent && !sent {
        return Err(ZwiimError::invalid_input("Nothing was sent for this item"));
    }
    // The keys this item holds, as its dispatch reserved them.
    let keys: Vec<LedgerKey> = never_twice_ledger::Entity::find()
        .filter(never_twice_ledger::Column::ItemId.eq(item.id))
        .filter(never_twice_ledger::Column::State.eq("reserved"))
        .all(&state.db.conn)
        .await
        .map_err(engine_err)?
        .into_iter()
        .map(|k| LedgerKey {
            site: k.site,
            account: k.account,
            action_kind: k.action_kind,
            item_key: k.item_key,
        })
        .collect();
    let store = PlaybookStore::new(state.db.conn.clone());
    let runner = crate::automation::item_runner::ItemRunner::new(engine(&state), store.clone());
    let stale = |e: crate::automation::item_runner::RunError| match e {
        crate::automation::item_runner::RunError::Transition(
            crate::db::service::playbook_engine_service::TransitionError::ItemStale { .. },
        ) => not_waiting(),
        other => engine_err(other),
    };
    let status = match p.choice {
        ResolveChoice::Sent => {
            runner.accept_sent(item.id, &keys).await.map_err(stale)?;
            ItemStatus::Done
        }
        ResolveChoice::NotSent => {
            runner.redo_not_sent(item.id, &keys).await.map_err(stale)?;
            ItemStatus::Queued
        }
        ResolveChoice::Skip => {
            // The person's skip, recorded as an approval Skip is (spec 11.2 has no other code).
            let reason = crate::automation::reasons::ReasonCode::ApprovalRejected.as_str();
            engine(&state)
                .transition_item(
                    item.id,
                    ItemStatus::WaitingPerson,
                    ItemStatus::Skipped,
                    Some(reason),
                )
                .await
                .map_err(|e| match e {
                    crate::db::service::playbook_engine_service::TransitionError::ItemStale {
                        ..
                    } => not_waiting(),
                    other => engine_err(other),
                })?;
            store
                .skip_item(
                    item.playbook_id,
                    &item.site,
                    &item.account,
                    &item.item_key,
                    reason,
                )
                .await
                .map_err(engine_err)?;
            ItemStatus::Skipped
        }
    };
    changed(&state);
    Ok(Json(RunResolveResult {
        item_id: p.item_id,
        status: status.as_str().to_string(),
    }))
}

// ── run.pause / run.resume / run.stop ─────────────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunIdParams {
    pub run_id: String,
}

pub async fn run_pause(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<RunIdParams>,
) -> ZResult<RunSummary> {
    // Pausing must work even with the Labs flag off, so a running run is
    // never stranded (spec 10.12.2).
    let run_id = run_row(&state, &p.run_id).await?.id;
    let engine = engine(&state);
    let current = run_status(&state, run_id).await?;
    if current == RunStatus::Paused || current.is_terminal() {
        // Idempotent (spec 8.5.4): pausing a paused or finished run is not
        // an error.
        return load_run(&state, run_id).await;
    }
    engine
        .transition_run(run_id, current, RunStatus::Paused, Some("person"))
        .await
        .map_err(|e| match e {
            crate::db::service::playbook_engine_service::TransitionError::RunStale { .. } => {
                ZwiimError::new(
                    axum::http::StatusCode::CONFLICT,
                    crate::automation::ErrorCode::RunNotRunning.as_str(),
                    "The run is not running",
                )
            }
            other => engine_err(other),
        })?;
    load_run(&state, run_id).await
}

pub async fn run_resume(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<RunIdParams>,
) -> ZResult<RunSummary> {
    // Resuming needs the engine on (no new work without it).
    require_enabled(&state.db.conn).await?;
    let run_id = run_row(&state, &p.run_id).await?.id;
    let engine = engine(&state);
    let not_paused = || {
        ZwiimError::new(
            axum::http::StatusCode::CONFLICT,
            crate::automation::ErrorCode::RunNotPaused.as_str(),
            "The run is not paused",
        )
    };
    // Idempotent like pause (spec 8.5.4, 18.5): resuming a running run, or
    // losing the race to another resume, answers with the running run.
    // Either way the approvals given while paused are released below, so a
    // repeated resume finishes one that stopped between its two writes.
    match run_status(&state, run_id).await? {
        RunStatus::Paused => match engine
            .transition_run(run_id, RunStatus::Paused, RunStatus::Running, None)
            .await
        {
            Ok(()) => {}
            Err(crate::db::service::playbook_engine_service::TransitionError::RunStale {
                ..
            }) => {
                if run_status(&state, run_id).await? != RunStatus::Running {
                    return Err(not_paused());
                }
            }
            Err(e) => return Err(engine_err(e)),
        },
        RunStatus::Running => {}
        _ => return Err(not_paused()),
    }
    // Approvals given while paused act now (spec 8.8.3).
    crate::automation::approvals::release_approved(&state.db.conn, run_id)
        .await
        .map_err(engine_err)?;
    changed(&state);
    load_run(&state, run_id).await
}

pub async fn run_stop(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<RunIdParams>,
) -> ZResult<RunSummary> {
    // Stopping must work even with the Labs flag off (spec 10.12.2).
    let run_id = run_row(&state, &p.run_id).await?.id;
    let engine = engine(&state);
    let current = run_status(&state, run_id).await?;
    if current.is_terminal() {
        return load_run(&state, run_id).await;
    }
    engine
        .transition_run(run_id, current, RunStatus::Stopped, None)
        .await
        .map_err(engine_err)?;
    // Stop cancels parked approvals and stops items without outward_started
    // (spec 8.8.1-2). Items past `outward_started` finish their outward step.
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let _ = crate::automation::approvals::stop_pending(&state.db.conn, run_id).await;
    let _ = playbook_item::Entity::update_many()
        .col_expr(
            playbook_item::Column::Status,
            sea_orm::sea_query::Expr::value("stopped"),
        )
        .filter(playbook_item::Column::RunId.eq(run_id))
        .filter(playbook_item::Column::OutwardStartedAt.is_null())
        .filter(playbook_item::Column::Status.is_in(["working", "waiting_approval"]))
        .exec(&state.db.conn)
        .await;
    changed(&state);
    load_run(&state, run_id).await
}

async fn run_status(state: &AppState, run_id: i32) -> Result<RunStatus, ZwiimError> {
    use sea_orm::EntityTrait;
    let row = playbook_run::Entity::find_by_id(run_id)
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("run {run_id}")))?;
    RunStatus::parse(&row.status).ok_or_else(|| {
        tracing::warn!(status = %row.status, "[zwiim] unknown run status");
        ZwiimError::internal()
    })
}

// ── site.list / site.resume / site.pause ──────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteSummary {
    pub site: String,
    pub account: String,
    pub paused_cause: Option<String>,
    pub risk_accepted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteListParams {
    /// Every site the Host knows for this account, from runs and site
    /// knowledge. A client passes the account it shows (default `main`).
    #[serde(default = "default_account")]
    pub account: String,
}

fn default_account() -> String {
    "main".to_string()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteParams {
    pub site: String,
    #[serde(default = "default_account")]
    pub account: String,
}

pub async fn site_list(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<SiteListParams>,
) -> ZResult<Vec<SiteSummary>> {
    require_enabled(&state.db.conn).await?;
    use crate::automation::entities::site_knowledge;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    // Any pause or risk-accept row names a site for this account.
    let rows = site_knowledge::Entity::find()
        .filter(site_knowledge::Column::Account.eq(p.account.clone()))
        .filter(site_knowledge::Column::Kind.is_in(["pause", "risk_accept"]))
        .all(&state.db.conn)
        .await
        .map_err(engine_err)?;
    let mut by_site: std::collections::BTreeMap<String, SiteSummary> = Default::default();
    for row in rows {
        let entry = by_site.entry(row.site.clone()).or_insert(SiteSummary {
            site: row.site.clone(),
            account: p.account.clone(),
            paused_cause: None,
            risk_accepted: false,
        });
        match row.kind.as_str() {
            "pause" if !row.value.is_empty() => entry.paused_cause = Some(row.value),
            "risk_accept" => entry.risk_accepted = true,
            _ => {}
        }
    }
    Ok(Json(by_site.into_values().collect()))
}

pub async fn site_resume(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<SiteParams>,
) -> ZResult<serde_json::Value> {
    require_enabled(&state.db.conn).await?;
    let store = PlaybookStore::new(state.db.conn.clone());
    let resumed = SitePause::resume(&store, &p.site, &p.account)
        .await
        .map_err(engine_err)?;
    if !resumed {
        return Err(ZwiimError::new(
            axum::http::StatusCode::CONFLICT,
            crate::automation::ErrorCode::SiteNotPaused.as_str(),
            "The site is not paused",
        ));
    }
    Ok(Json(serde_json::json!({ "resumed": true })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SitePauseParams {
    pub site: String,
    #[serde(default = "default_account")]
    pub account: String,
    /// `captcha`, `blocked` or `login`.
    pub cause: String,
}

pub async fn site_pause(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<SitePauseParams>,
) -> ZResult<serde_json::Value> {
    require_enabled(&state.db.conn).await?;
    let cause = PauseCause::parse(&p.cause)
        .ok_or_else(|| ZwiimError::invalid_input("Unknown pause cause"))?;
    let store = PlaybookStore::new(state.db.conn.clone());
    SitePause::pause(&store, &p.site, &p.account, cause)
        .await
        .map_err(engine_err)?;
    Ok(Json(serde_json::json!({ "paused": true })))
}

// ── site.risk_accept (spec 14.5) ──────────────────────────────────────────

pub async fn site_risk_accept(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<SiteParams>,
) -> ZResult<serde_json::Value> {
    require_enabled(&state.db.conn).await?;
    let store = PlaybookStore::new(state.db.conn.clone());
    store
        .set_site_value(
            &p.site,
            SiteKnowledgeKind::RiskAccept,
            &p.account,
            &chrono::Utc::now().to_rfc3339(),
        )
        .await
        .map_err(engine_err)?;
    Ok(Json(serde_json::json!({ "accepted": true })))
}

// ── caps: the daily usage a Sites list shows (spec 10.1) ──────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteUsageParams {
    pub site: String,
    #[serde(default = "default_account")]
    pub account: String,
    /// `YYYY-MM-DD`; defaults to the Host's local day.
    pub day: Option<String>,
}

pub async fn site_usage(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<SiteUsageParams>,
) -> ZResult<serde_json::Value> {
    require_enabled(&state.db.conn).await?;
    use crate::automation::entities::cap_usage;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let day = p
        .day
        .unwrap_or_else(|| crate::automation::caps::local_day_here(chrono::Utc::now()));
    let rows = cap_usage::Entity::find()
        .filter(cap_usage::Column::Site.eq(p.site.clone()))
        .filter(cap_usage::Column::Account.eq(p.account.clone()))
        .filter(cap_usage::Column::LocalDay.eq(day.clone()))
        .all(&state.db.conn)
        .await
        .map_err(engine_err)?;
    let used: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|r| serde_json::json!({ "action_kind": r.action_kind, "used": r.used }))
        .collect();
    Ok(Json(serde_json::json!({ "day": day, "used": used })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_summary_reads_the_client_fields() {
        let now = chrono::Utc::now();
        let model = playbook_item::Model {
            id: 3,
            public_id: "item-3".into(),
            run_id: 1,
            playbook_id: 2,
            site: "indeed.com".into(),
            account: "main".into(),
            item_key: "job-1".into(),
            title: Some("Backend Engineer".into()),
            url: None,
            outward_site: None,
            outward_url: None,
            status: "done".into(),
            reason_code: None,
            epoch: 2,
            outward_started_at: None,
            cap_site: None,
            cap_kind: None,
            cap_day: None,
            tab_id: Some("tab-4".into()),
            created_at: now,
            updated_at: now,
        };
        let summary: ItemSummary = model.into();
        assert_eq!(summary.tab_id.as_deref(), Some("tab-4"));
        assert_eq!(summary.key, "job-1");
        assert_eq!(summary.item_id, "item-3");
        assert_eq!(summary.status, "done");
    }

    #[test]
    fn default_account_is_main() {
        assert_eq!(default_account(), "main");
    }
}

// ── run.approve / run.reject (engine-owned approvals, spec 8.5) ───────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalAnswerParams {
    pub approval_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalSendParams {
    pub approval_id: String,
    /// Edit then send (spec 8.5.2): new text for declared text fields.
    #[serde(default)]
    pub edits: Vec<crate::automation::approvals::FieldEdit>,
}

#[derive(Serialize)]
pub struct ApprovalAnswerResult {
    pub approval_id: String,
    pub state: String,
}

/// The person approved (Send): the item continues and the Host performs the
/// approved click in its next step. First answer wins; a second answer gets
/// `approval_not_pending` with the current state.
pub async fn run_approve(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<ApprovalSendParams>,
) -> ZResult<ApprovalAnswerResult> {
    answer_approval(
        &state,
        &p.approval_id,
        crate::automation::ApprovalAnswer::Approve,
        &p.edits,
    )
    .await
}

/// The person skipped the item: it ends `skipped` (`approval_rejected`).
pub async fn run_reject(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Json(p): axum::extract::Json<ApprovalAnswerParams>,
) -> ZResult<ApprovalAnswerResult> {
    answer_approval(
        &state,
        &p.approval_id,
        crate::automation::ApprovalAnswer::Skip,
        &[],
    )
    .await
}

async fn answer_approval(
    state: &AppState,
    approval_id: &str,
    answer: crate::automation::ApprovalAnswer,
    edits: &[crate::automation::approvals::FieldEdit],
) -> ZResult<ApprovalAnswerResult> {
    require_enabled(&state.db.conn).await?;
    use crate::automation::approvals::{self, AnswerOutcome};
    use crate::automation::entities::playbook_approval;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let row = playbook_approval::Entity::find()
        .filter(playbook_approval::Column::PublicId.eq(approval_id))
        .one(&state.db.conn)
        .await
        .map_err(engine_err)?
        .ok_or_else(|| ZwiimError::not_found(format!("approval {approval_id}")))?;
    // The answer and its effect on the item commit together (spec 8.5.4).
    let outcome = approvals::respond(&state.db.conn, row.id, answer.clone(), edits)
        .await
        .map_err(|e| match e {
            crate::db::error::DbError::NotFound(what) => ZwiimError::not_found(what),
            other => engine_err(other),
        })?;
    // The pending set may have changed: clients drop the card (spec 8.5.4).
    changed(state);
    match outcome {
        AnswerOutcome::NotPending(current) => {
            let mut err = ZwiimError::new(
                axum::http::StatusCode::CONFLICT,
                crate::automation::ErrorCode::ApprovalNotPending.as_str(),
                "This approval was already answered",
            );
            err.details = Some(serde_json::json!({ "state": current.as_str() }));
            Err(err)
        }
        AnswerOutcome::InvalidEdit(message) => Err(ZwiimError::invalid_input(message)),
        AnswerOutcome::Applied => Ok(Json(ApprovalAnswerResult {
            approval_id: approval_id.to_string(),
            state: match answer {
                crate::automation::ApprovalAnswer::Approve => "approved",
                crate::automation::ApprovalAnswer::Skip => "rejected",
            }
            .to_string(),
        })),
    }
}
