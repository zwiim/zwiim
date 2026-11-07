//! Zwiim repository settings: what the user set for one repository, kept on
//! the Host so every device sees the same, such as the scripts run when a
//! worktree of it is created or removed.
//!
//! Every setting is declared once in [`KEYS`]. A repository's values live in
//! one `repository_settings` row of its root folder, a JSON object of the keys
//! set; an unset key is absent. A worktree's `workspace_id` names its
//! repository, so a client in a worktree reads and changes its repository's.
//!
//! [`ENV`] is the exception: its values can be credentials, so they are kept
//! in the OS keychain, and the row holds only the name of their entry there.

use std::collections::HashMap;

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, NotSet,
    QueryFilter, Set,
};
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::db::entities::repository_settings;
use crate::protocol::error::ZwiimError;

pub const SETUP_SCRIPT: &str = "scripts.setup";
pub const ARCHIVE_SCRIPT: &str = "scripts.archive";
/// What a client's Run starts in a worktree, such as its dev server.
pub const RUN_SCRIPT: &str = "scripts.run";
/// Patterns, in `.gitignore` syntax, of the Git-ignored files copied from the
/// repository folder into each new worktree. Unset is [`DEFAULT_FILES_TO_COPY`];
/// an empty list copies nothing.
pub const FILES_TO_COPY: &str = "files.copy";
/// Environment variables, an object of names to values, given to the agents,
/// terminals and scripts started in the repository's worktrees.
pub const ENV: &str = "env";
/// Where clients open a worktree's servers: a list of `{url, name?}`, each
/// `url` http(s) and given the worktree's ports by `crate::workspace::previews`.
/// Unset, they open the servers its terminals print.
pub const PREVIEW_URLS: &str = "preview.urls";

/// Every repository setting.
const KEYS: &[&str] = &[SETUP_SCRIPT, RUN_SCRIPT, ARCHIVE_SCRIPT, FILES_TO_COPY, ENV, PREVIEW_URLS];

/// In a row, where [`ENV`]'s values are kept: the keychain entry's name.
const ENV_REF: &str = "env.ref";

/// Longest script accepted, in bytes.
pub const MAX_SCRIPT: usize = 16_000;

/// What [`FILES_TO_COPY`] is when unset.
pub const DEFAULT_FILES_TO_COPY: &[&str] = &[".env*"];
/// Most patterns kept, and the longest, in bytes.
pub const MAX_PATTERNS: usize = 100;
pub const MAX_PATTERN: usize = 256;

/// Longest variable name, and most bytes of names and values together.
pub const MAX_ENV_NAME: usize = 128;
pub const MAX_ENV_BYTES: usize = 32_000;

/// Most preview URLs kept, the longest, and the longest name, in bytes.
pub const MAX_PREVIEW_URLS: usize = 10;
pub const MAX_PREVIEW_URL: usize = 2_048;
pub const MAX_PREVIEW_NAME: usize = 64;

/// Updates read, merge and write a row: one at a time, so none is lost.
static WRITES: Mutex<()> = Mutex::const_new(());

fn db_error(e: sea_orm::DbErr) -> ZwiimError {
    ZwiimError::from(crate::app_error::AppCommandError::from(
        crate::db::error::DbError::from(e),
    ))
}

/// The root folder of live workspace `workspace_id`: itself for a repository,
/// its repository for a worktree.
async fn repository_of(db: &DatabaseConnection, workspace_id: &str) -> Result<i32, ZwiimError> {
    let row = crate::workspace::resolve(db, workspace_id).await?;
    Ok(row.parent_id.unwrap_or(row.id))
}

async fn stored(db: &DatabaseConnection, folder_id: i32) -> Result<Map<String, Value>, ZwiimError> {
    let row = repository_settings::Entity::find()
        .filter(repository_settings::Column::FolderId.eq(folder_id))
        .one(db)
        .await
        .map_err(db_error)?;
    Ok(row
        .and_then(|r| serde_json::from_str(&r.settings).ok())
        .unwrap_or_default())
}

/// Every setting of the repository of `workspace_id`, `null` when unset.
pub async fn get(db: &DatabaseConnection, workspace_id: &str) -> Result<Map<String, Value>, ZwiimError> {
    let mut values = stored(db, repository_of(db, workspace_id).await?).await?;
    let env = match values.remove(ENV_REF) {
        Some(Value::String(entry)) => read_env(entry).await?.map(Value::Object),
        _ => None,
    };
    if let Some(env) = env {
        values.insert(ENV.to_string(), env);
    }
    Ok(KEYS
        .iter()
        .map(|key| (key.to_string(), values.remove(*key).unwrap_or(Value::Null)))
        .collect())
}

/// Set each key in `values` for the repository of `workspace_id`, or clear it
/// for `null`. Every value is checked before any is written.
pub async fn update(
    db: &DatabaseConnection,
    workspace_id: &str,
    values: &Map<String, Value>,
) -> Result<(), ZwiimError> {
    for (key, value) in values {
        check(key, value)?;
    }
    let folder_id = repository_of(db, workspace_id).await?;
    let _write = WRITES.lock().await;
    let mut settings = stored(db, folder_id).await?;
    for (key, value) in values {
        if key == ENV {
            keep_env(&mut settings, value).await?;
            continue;
        }
        match value {
            Value::Null => settings.remove(key),
            value => settings.insert(key.clone(), value.clone()),
        };
    }
    let settings = Value::Object(settings).to_string();
    let existing = repository_settings::Entity::find()
        .filter(repository_settings::Column::FolderId.eq(folder_id))
        .one(db)
        .await
        .map_err(db_error)?;
    match existing {
        Some(row) => {
            let mut active = row.into_active_model();
            active.settings = Set(settings);
            active.updated_at = Set(Utc::now());
            active.update(db).await.map_err(db_error)?;
        }
        None => {
            repository_settings::ActiveModel {
                id: NotSet,
                folder_id: Set(folder_id),
                settings: Set(settings),
                updated_at: Set(Utc::now()),
            }
            .insert(db)
            .await
            .map_err(db_error)?;
        }
    }
    Ok(())
}

/// Fill the scripts this repository has not set from its Conductor settings.
/// Returns the keys it set; a script already set is left alone.
pub async fn import_conductor(
    db: &DatabaseConnection,
    workspace_id: &str,
) -> Result<Vec<&'static str>, ZwiimError> {
    let folder_id = repository_of(db, workspace_id).await?;
    let root = crate::db::entities::folder::Entity::find_by_id(folder_id)
        .one(db)
        .await
        .map_err(db_error)?
        .ok_or_else(|| ZwiimError::not_found("Repository not found"))?;
    let scripts = crate::conductor_import::read_scripts(std::path::Path::new(&root.path))
        .map_err(ZwiimError::invalid_input)?
        .ok_or_else(|| ZwiimError::not_found("This repository has no Conductor settings"))?;
    let kept = stored(db, folder_id).await?;
    let values: Map<String, Value> = scripts
        .iter()
        .filter(|(key, _)| !kept.contains_key(**key))
        .map(|(key, script)| (key.to_string(), Value::String(script.clone())))
        .collect();
    update(db, workspace_id, &values).await?;
    Ok(scripts.into_keys().filter(|key| values.contains_key(*key)).collect())
}

fn check(key: &str, value: &Value) -> Result<(), ZwiimError> {
    if !KEYS.contains(&key) {
        return Err(ZwiimError::invalid_input(format!("Unknown setting `{key}`")));
    }
    let valid = match (key, value) {
        (_, Value::Null) => true,
        (FILES_TO_COPY, Value::Array(patterns)) => {
            patterns.len() <= MAX_PATTERNS
                && patterns.iter().all(|p| p.as_str().is_some_and(valid_pattern))
        }
        (ENV, Value::Object(vars)) => {
            let bytes: usize = vars
                .iter()
                .map(|(name, v)| name.len() + v.as_str().map_or(0, str::len))
                .sum();
            bytes <= MAX_ENV_BYTES
                && vars.iter().all(|(name, v)| {
                    valid_env_name(name) && v.as_str().is_some_and(valid_env_value)
                })
        }
        (PREVIEW_URLS, Value::Array(urls)) => {
            urls.len() <= MAX_PREVIEW_URLS && urls.iter().all(valid_preview)
        }
        // A NUL could never reach the shell running it.
        (SETUP_SCRIPT | RUN_SCRIPT | ARCHIVE_SCRIPT, Value::String(s)) => {
            !s.trim().is_empty() && s.len() <= MAX_SCRIPT && !s.contains('\0')
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(ZwiimError::invalid_input(format!("Not a valid value for `{key}`")))
    }
}

/// One `.gitignore` line that selects files: no blank line, comment or
/// whitespace around it, and a glob that compiles.
fn valid_pattern(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= MAX_PATTERN
        && p.trim() == p
        && !p.starts_with('#')
        && !p.contains(['\0', '\n', '\r'])
        && ignore::gitignore::GitignoreBuilder::new("/").add_line(None, p).is_ok()
}

/// A name a shell can export and the Host's own `ZWIIM_*` don't use.
fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= MAX_ENV_NAME
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.to_ascii_uppercase().starts_with("ZWIIM_")
}

/// `{url, name?}`: an http(s) URL and a name, each on one line.
fn valid_preview(v: &Value) -> bool {
    let Some(preview) = v.as_object() else {
        return false;
    };
    let one_line = |s: &str| !s.chars().any(char::is_control);
    let url = preview.get("url").and_then(Value::as_str).is_some_and(|u| {
        u.len() <= MAX_PREVIEW_URL && one_line(u) && is_web_url(u)
    });
    let name = match preview.get("name") {
        None | Some(Value::Null) => true,
        Some(Value::String(n)) => !n.trim().is_empty() && n.len() <= MAX_PREVIEW_NAME && one_line(n),
        _ => false,
    };
    url && name && preview.keys().all(|k| k == "url" || k == "name")
}

/// `http://` or `https://` and something after it.
pub fn is_web_url(url: &str) -> bool {
    ["http://", "https://"].iter().any(|scheme| {
        url.len() > scheme.len() && url.get(..scheme.len()).is_some_and(|s| s.eq_ignore_ascii_case(scheme))
    })
}

/// One line, which is all a process environment reliably passes on.
fn valid_env_value(value: &str) -> bool {
    !value.contains(['\0', '\n', '\r'])
}

/// Keep `value`, a checked [`ENV`], in the keychain entry the row names,
/// making one on the first save; `null` or no variables removes it.
async fn keep_env(settings: &mut Map<String, Value>, value: &Value) -> Result<(), ZwiimError> {
    let existing = settings.get(ENV_REF).and_then(Value::as_str).map(str::to_owned);
    match value.as_object().filter(|vars| !vars.is_empty()) {
        Some(vars) => {
            let entry = existing.unwrap_or_else(|| format!("repository-env:{}", uuid::Uuid::new_v4()));
            let json = Value::Object(vars.clone()).to_string();
            let name = entry.clone();
            blocking(move || keychain::set(&name, &json)).await?;
            settings.insert(ENV_REF.to_string(), Value::String(entry));
        }
        None => {
            if let Some(entry) = existing {
                blocking(move || keychain::delete(&entry)).await?;
            }
            settings.remove(ENV_REF);
        }
    }
    Ok(())
}

/// The variables kept in keychain entry `entry`; `None` when it is gone.
async fn read_env(entry: String) -> Result<Option<Map<String, Value>>, ZwiimError> {
    let json = blocking(move || keychain::get(&entry)).await?;
    Ok(json.and_then(|j| match serde_json::from_str(&j) {
        Ok(Value::Object(vars)) => Some(vars),
        _ => None,
    }))
}

/// Run keychain call `f` off the async threads: it can wait on the OS.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, ZwiimError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| ZwiimError::internal())?
        .map_err(|e| {
            tracing::warn!(error = %e, "[repository] the keychain refused");
            ZwiimError::keychain_unavailable()
        })
}

#[cfg(not(test))]
mod keychain {
    pub fn get(name: &str) -> Result<Option<String>, String> {
        crate::keyring_store::get_secret(name)
    }
    pub fn set(name: &str, value: &str) -> Result<(), String> {
        crate::keyring_store::set_secret(name, value)
    }
    pub fn delete(name: &str) -> Result<(), String> {
        crate::keyring_store::delete_secret(name)
    }
}

/// Tests never reach the developer's keychain: a process-wide map stands in.
/// Entry names are random, so tests running together never share one.
#[cfg(test)]
mod keychain {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    pub static ENTRIES: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

    pub fn get(name: &str) -> Result<Option<String>, String> {
        Ok(ENTRIES.lock().unwrap().get(name).cloned())
    }
    pub fn set(name: &str, value: &str) -> Result<(), String> {
        ENTRIES.lock().unwrap().insert(name.to_string(), value.to_string());
        Ok(())
    }
    pub fn delete(name: &str) -> Result<(), String> {
        ENTRIES.lock().unwrap().remove(name);
        Ok(())
    }
}

/// Script `key` of repository `folder_id`, when set. Unreadable is unset: a
/// script is never a reason to refuse creating or removing a worktree.
pub async fn script(db: &DatabaseConnection, folder_id: i32, key: &str) -> Option<String> {
    match stored(db, folder_id).await.ok()?.remove(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s),
        _ => None,
    }
}

/// The patterns of the files to copy into a new worktree of repository
/// `folder_id`. Unreadable is unset.
pub async fn files_to_copy(db: &DatabaseConnection, folder_id: i32) -> Vec<String> {
    match stored(db, folder_id).await.ok().and_then(|mut s| s.remove(FILES_TO_COPY)) {
        Some(Value::Array(patterns)) => patterns
            .into_iter()
            .filter_map(|p| p.as_str().map(str::to_owned))
            .collect(),
        _ => DEFAULT_FILES_TO_COPY.iter().map(|p| p.to_string()).collect(),
    }
}

/// The preview URLs of repository `folder_id`, as `(name, url)`, in order.
/// Unreadable is unset.
pub async fn preview_urls(db: &DatabaseConnection, folder_id: i32) -> Vec<(Option<String>, String)> {
    match stored(db, folder_id).await.ok().and_then(|mut s| s.remove(PREVIEW_URLS)) {
        Some(Value::Array(urls)) => urls
            .into_iter()
            .filter_map(|p| {
                let url = p.get("url")?.as_str()?.to_owned();
                Some((p.get("name").and_then(Value::as_str).map(str::to_owned), url))
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The environment variables of repository `folder_id`, for a process started
/// in one of its workspaces. Unreadable is none, logged: a variable is never a
/// reason to refuse starting an agent or a terminal.
pub async fn env(db: &DatabaseConnection, folder_id: i32) -> HashMap<String, String> {
    let entry = match stored(db, folder_id).await.ok().and_then(|mut s| s.remove(ENV_REF)) {
        Some(Value::String(entry)) => entry,
        _ => return HashMap::new(),
    };
    match read_env(entry).await {
        Ok(vars) => vars
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(name, v)| Some((name, v.as_str()?.to_owned())))
            .collect(),
        Err(_) => HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::entities::folder;
    use crate::db::test_helpers::{fresh_in_memory_db, seed_folder};
    use crate::db::AppDatabase;
    use serde_json::json;

    async fn repository(db: &AppDatabase, path: &str) -> String {
        let id = seed_folder(db, path).await;
        folder::Entity::find_by_id(id)
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap()
            .workspace_id
            .unwrap()
    }

    #[tokio::test]
    async fn importing_fills_only_the_scripts_that_are_unset() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("conductor.json"),
            r#"{"scripts":{"setup":"npm install","run":"npm run dev"}}"#,
        )
        .unwrap();
        let id = repository(&db, dir.path().to_str().unwrap()).await;
        update(&db.conn, &id, &values(json!({ "scripts.run": "mine" }))).await.unwrap();

        let imported = import_conductor(&db.conn, &id).await.unwrap();

        assert_eq!(imported, [SETUP_SCRIPT]);
        let kept = get(&db.conn, &id).await.unwrap();
        assert_eq!(kept[SETUP_SCRIPT], "npm install");
        assert_eq!(kept[RUN_SCRIPT], "mine");
        assert!(import_conductor(&db.conn, &id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_repository_without_conductor_settings_cannot_import() {
        let db = fresh_in_memory_db().await;
        let dir = tempfile::tempdir().unwrap();
        let id = repository(&db, dir.path().to_str().unwrap()).await;
        assert!(import_conductor(&db.conn, &id).await.is_err());
    }

    fn values(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[tokio::test]
    async fn keeps_each_repositorys_scripts_apart() {
        let db = fresh_in_memory_db().await;
        let a = repository(&db, "/tmp/zwiim-repo-settings-a").await;
        let b = repository(&db, "/tmp/zwiim-repo-settings-b").await;

        assert_eq!(
            get(&db.conn, &a).await.unwrap(),
            values(json!({
                "scripts.setup": null,
                "scripts.run": null,
                "scripts.archive": null,
                "files.copy": null,
                "env": null,
                "preview.urls": null,
            }))
        );
        update(&db.conn, &a, &values(json!({ "scripts.setup": "bun install" })))
            .await
            .unwrap();
        update(
            &db.conn,
            &a,
            &values(json!({ "scripts.archive": "docker compose down", "scripts.run": "bun dev" })),
        )
        .await
        .unwrap();
        assert_eq!(
            get(&db.conn, &a).await.unwrap(),
            values(json!({
                "scripts.setup": "bun install",
                "scripts.run": "bun dev",
                "scripts.archive": "docker compose down",
                "files.copy": null,
                "env": null,
                "preview.urls": null,
            }))
        );
        assert_eq!(get(&db.conn, &b).await.unwrap()["scripts.setup"], Value::Null);

        update(&db.conn, &a, &values(json!({ "scripts.setup": null }))).await.unwrap();
        assert_eq!(get(&db.conn, &a).await.unwrap()["scripts.setup"], Value::Null);
        assert_eq!(
            get(&db.conn, &a).await.unwrap()["scripts.archive"],
            json!("docker compose down")
        );
    }

    #[tokio::test]
    async fn refuses_what_it_would_not_keep_and_writes_nothing() {
        let db = fresh_in_memory_db().await;
        let repo = repository(&db, "/tmp/zwiim-repo-settings-bad").await;
        for bad in [
            json!({ "scripts.setup": "make", "scripts.deploy": "x" }),
            json!({ "scripts.setup": "make", "scripts.archive": "  " }),
            json!({ "scripts.setup": "make", "scripts.run": ["npm run dev"] }),
            json!({ "scripts.setup": "make", "scripts.archive": true }),
            json!({ "scripts.setup": "make", "scripts.archive": "a\0b" }),
            json!({ "scripts.setup": "x".repeat(MAX_SCRIPT + 1) }),
            json!({ "scripts.setup": "make", "files.copy": ".env" }),
            json!({ "scripts.setup": "make", "files.copy": [""] }),
            json!({ "scripts.setup": "make", "files.copy": [" .env"] }),
            json!({ "scripts.setup": "make", "files.copy": ["# note"] }),
            json!({ "scripts.setup": "make", "files.copy": ["a\nb"] }),
            json!({ "scripts.setup": "make", "files.copy": [1] }),
            json!({ "files.copy": vec![".env"; MAX_PATTERNS + 1] }),
            json!({ "scripts.setup": "make", "env": "A=1" }),
            json!({ "scripts.setup": "make", "env": { "1A": "x" } }),
            json!({ "scripts.setup": "make", "env": { "A-B": "x" } }),
            json!({ "scripts.setup": "make", "env": { "zwiim_shell": "x" } }),
            json!({ "scripts.setup": "make", "env": { "A": 1 } }),
            json!({ "scripts.setup": "make", "env": { "A": "x\ny" } }),
            json!({ "env": { "A": "x".repeat(MAX_ENV_BYTES) } }),
            json!({ "preview.urls": "http://localhost:3000" }),
            json!({ "preview.urls": ["http://localhost:3000"] }),
            json!({ "preview.urls": [{ "url": "localhost:3000" }] }),
            json!({ "preview.urls": [{ "url": "javascript:alert(1)" }] }),
            json!({ "preview.urls": [{ "url": "file:///etc/passwd" }] }),
            json!({ "preview.urls": [{ "url": "http://" }] }),
            json!({ "preview.urls": [{ "url": "http://a\nb" }] }),
            json!({ "preview.urls": [{ "url": "http://a", "name": " " }] }),
            json!({ "preview.urls": [{ "url": "http://a", "name": "x".repeat(MAX_PREVIEW_NAME + 1) }] }),
            json!({ "preview.urls": [{ "url": "http://a", "open": true }] }),
            json!({ "preview.urls": [{ "url": format!("http://{}", "a".repeat(MAX_PREVIEW_URL)) }] }),
            json!({ "preview.urls": vec![json!({ "url": "http://a" }); MAX_PREVIEW_URLS + 1] }),
        ] {
            let err = update(&db.conn, &repo, &values(bad)).await.unwrap_err();
            assert_eq!(err.code, "invalid_input");
        }
        assert_eq!(get(&db.conn, &repo).await.unwrap()["scripts.setup"], Value::Null);
        assert!(update(&db.conn, "nope", &values(json!({}))).await.is_err());
        let id = repository_of(&db.conn, &repo).await.unwrap();
        assert!(stored(&db.conn, id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn keeps_preview_urls_in_order() {
        let db = fresh_in_memory_db().await;
        let repo = repository(&db, "/tmp/zwiim-repo-settings-previews").await;
        let urls = json!([
            { "url": "http://localhost:$ZWIIM_PORT" },
            { "name": "Storybook", "url": "HTTPS://localhost:$((ZWIIM_PORT + 1))/docs" },
        ]);
        update(&db.conn, &repo, &values(json!({ "preview.urls": urls.clone() })))
            .await
            .unwrap();
        assert_eq!(get(&db.conn, &repo).await.unwrap()["preview.urls"], urls);
        let id = repository_of(&db.conn, &repo).await.unwrap();
        assert_eq!(
            preview_urls(&db.conn, id).await,
            vec![
                (None, "http://localhost:$ZWIIM_PORT".to_string()),
                (
                    Some("Storybook".to_string()),
                    "HTTPS://localhost:$((ZWIIM_PORT + 1))/docs".to_string()
                ),
            ]
        );
        update(&db.conn, &repo, &values(json!({ "preview.urls": null }))).await.unwrap();
        assert!(preview_urls(&db.conn, id).await.is_empty());
    }

    #[tokio::test]
    async fn keeps_patterns_and_falls_back_to_env_files() {
        let db = fresh_in_memory_db().await;
        let repo = repository(&db, "/tmp/zwiim-repo-settings-files").await;
        let id = repository_of(&db.conn, &repo).await.unwrap();
        assert_eq!(files_to_copy(&db.conn, id).await, vec![".env*"]);

        let patterns = json!(["config/*.local.json", "!.env.example", "/secrets/"]);
        update(&db.conn, &repo, &values(json!({ "files.copy": patterns.clone() })))
            .await
            .unwrap();
        assert_eq!(get(&db.conn, &repo).await.unwrap()["files.copy"], patterns);
        assert_eq!(
            files_to_copy(&db.conn, id).await,
            vec!["config/*.local.json", "!.env.example", "/secrets/"]
        );

        update(&db.conn, &repo, &values(json!({ "files.copy": [] }))).await.unwrap();
        assert!(files_to_copy(&db.conn, id).await.is_empty());
        update(&db.conn, &repo, &values(json!({ "files.copy": null }))).await.unwrap();
        assert_eq!(files_to_copy(&db.conn, id).await, vec![".env*"]);
    }

    #[tokio::test]
    async fn keeps_env_values_in_the_keychain_not_the_database() {
        let db = fresh_in_memory_db().await;
        let repo = repository(&db, "/tmp/zwiim-repo-settings-env").await;
        let id = repository_of(&db.conn, &repo).await.unwrap();
        assert!(env(&db.conn, id).await.is_empty());

        let vars = json!({ "API_TOKEN": "s3cr3t-value", "EMPTY": "", "_Mixed1": "a=b c" });
        update(&db.conn, &repo, &values(json!({ "env": vars.clone() }))).await.unwrap();
        assert_eq!(get(&db.conn, &repo).await.unwrap()["env"], vars);
        assert_eq!(env(&db.conn, id).await["API_TOKEN"], "s3cr3t-value");
        assert_eq!(env(&db.conn, id).await["_Mixed1"], "a=b c");

        // The row names the entry; neither names nor values are in it.
        let row = repository_settings::Entity::find()
            .filter(repository_settings::Column::FolderId.eq(id))
            .one(&db.conn)
            .await
            .unwrap()
            .unwrap();
        assert!(!row.settings.contains("s3cr3t") && !row.settings.contains("API_TOKEN"));
        let entry = stored(&db.conn, id).await.unwrap()[ENV_REF].as_str().unwrap().to_owned();
        assert!(keychain::ENTRIES.lock().unwrap()[&entry].contains("s3cr3t-value"));

        // Agents a delegating agent starts find them by their working folder,
        // with where they are.
        crate::db::service::folder_service::add_folder_with_parent(
            &db.conn,
            "/tmp/zwiim-repo-settings-env-wt",
            Some(id),
        )
        .await
        .unwrap();
        let at = |dir| crate::workspace::scripts::workspace_env_at(&db.conn, dir);
        let found = at(Some("/tmp/zwiim-repo-settings-env-wt")).await;
        assert_eq!(found["API_TOKEN"], "s3cr3t-value");
        assert_eq!(found["ZWIIM_WORKSPACE_PATH"], "/tmp/zwiim-repo-settings-env-wt");
        assert!(found["ZWIIM_PORT"].parse::<u16>().is_ok());
        assert!(at(Some("/tmp/elsewhere")).await.is_empty() && at(None).await.is_empty());

        // A save replaces every variable, in the same entry.
        update(&db.conn, &repo, &values(json!({ "env": { "ONLY": "1" } }))).await.unwrap();
        assert_eq!(get(&db.conn, &repo).await.unwrap()["env"], json!({ "ONLY": "1" }));
        assert_eq!(stored(&db.conn, id).await.unwrap()[ENV_REF], json!(entry));

        // No variables removes the entry.
        update(&db.conn, &repo, &values(json!({ "env": {} }))).await.unwrap();
        assert_eq!(get(&db.conn, &repo).await.unwrap()["env"], Value::Null);
        assert!(!keychain::ENTRIES.lock().unwrap().contains_key(&entry));
        assert!(stored(&db.conn, id).await.unwrap().get(ENV_REF).is_none());

        // An entry gone from the keychain reads as unset.
        update(&db.conn, &repo, &values(json!({ "env": { "B": "2" } }))).await.unwrap();
        let entry = stored(&db.conn, id).await.unwrap()[ENV_REF].as_str().unwrap().to_owned();
        keychain::ENTRIES.lock().unwrap().remove(&entry);
        assert_eq!(get(&db.conn, &repo).await.unwrap()["env"], Value::Null);
        assert!(env(&db.conn, id).await.is_empty());
    }
}
