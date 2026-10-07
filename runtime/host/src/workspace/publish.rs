//! Publishing a new workspace to a private GitHub repository the user owns:
//! the name rules, what a folder is before it is added (`workspace.inspect`),
//! and the publish steps of `workspace.add`.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::repo_init::{self, Created, Identity};
use crate::db::AppDatabase;
use crate::git_harness::{self, GitError, GitErrorCode, GithubOwner};
use crate::protocol::error::ZwiimError;

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// GitHub's repository name rule: 1–100 of `[A-Za-z0-9._-]`, not `.`/`..`.
pub(crate) fn valid_repo_name(name: &str) -> bool {
    (1..=100).contains(&name.len()) && name.chars().all(name_char) && name != "." && name != ".."
}

/// A GitHub login: 1–39 of `[A-Za-z0-9-_]`, not starting with `-` (which
/// would also read as a `gh` option) or `_`. `_` is allowed anywhere but
/// first for Enterprise Managed User logins (e.g. `octo_corp`).
pub(crate) fn valid_owner(owner: &str) -> bool {
    (1..=39).contains(&owner.len())
        && !owner.starts_with('-')
        && !owner.starts_with('_')
        && owner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub(crate) fn check_target(owner: &str, name: &str) -> Result<(), ZwiimError> {
    if !valid_owner(owner) {
        return Err(ZwiimError::invalid_input("Choose an owner"));
    }
    if !valid_repo_name(name) {
        return Err(ZwiimError::invalid_input(
            "Repository names use letters, digits, '.', '-' and '_' (at most 100)",
        ));
    }
    Ok(())
}

/// The folder name as a repository name: each run of other characters
/// becomes `-`, trimmed, at most 100; `repository` if nothing is left.
pub(crate) fn suggested_name(folder: &str) -> String {
    let mut out = String::new();
    for c in folder.chars() {
        if name_char(c) {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed: String = out.trim_matches('-').chars().take(100).collect();
    let trimmed = trimmed.trim_end_matches('-').to_string();
    if valid_repo_name(&trimmed) {
        trimmed
    } else {
        "repository".to_string()
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GitState {
    None,
    NoOrigin,
    Origin,
}

#[derive(Debug, Serialize)]
pub struct FirstCommit {
    files: u64,
    bytes: u64,
    at_least: bool,
    large: bool,
}

/// Whether the folder can be published under the signed-in account.
#[derive(Debug, Serialize)]
pub struct GithubStatus {
    available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    login: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owners: Option<Vec<GithubOwner>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

/// What a folder is before it is added. Touches nothing.
#[derive(Debug, Serialize)]
pub struct Inspection {
    git: GitState,
    /// The branch publishing pushes: `main` for a new repository.
    branch: Option<String>,
    suggested_name: String,
    first_commit: Option<FirstCommit>,
    github: Option<GithubStatus>,
}

pub async fn inspect(
    db: &AppDatabase,
    path: &str,
    gh: Option<&Path>,
) -> Result<Inspection, ZwiimError> {
    let canonical = super::canonical_for_add(db, path).await?;
    let repo = canonical.to_string_lossy().into_owned();
    let git = if !repo_init::has_git(&canonical) || repo_init::interrupted(&canonical) {
        GitState::None
    } else if super::has_origin(&repo).await {
        GitState::Origin
    } else {
        GitState::NoOrigin
    };
    let branch = match git {
        GitState::None => Some("main".to_string()),
        GitState::NoOrigin => current_branch(&canonical).await,
        GitState::Origin => None,
    };
    let first_commit = if git == GitState::None {
        let dir = canonical.clone();
        let size = tokio::task::spawn_blocking(move || repo_init::measure_first_commit(&dir))
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "[workspace] inspect: measuring did not finish");
                ZwiimError::internal()
            })?;
        Some(FirstCommit {
            large: size.is_large(),
            files: size.files,
            bytes: size.bytes,
            at_least: size.at_least,
        })
    } else {
        None
    };
    let github = match git {
        GitState::Origin => None,
        _ => Some(github_status(gh, &repo).await),
    };
    let folder_name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Inspection {
        git,
        branch,
        suggested_name: suggested_name(&folder_name),
        first_commit,
        github,
    })
}

/// `workspace.add`'s `publish`: create private `owner/name` from the folder.
#[derive(Debug, Clone)]
pub struct Publish {
    pub owner: String,
    pub name: String,
    /// `AppState.gh`.
    pub gh: Option<PathBuf>,
}

/// The signed-in account, before anything is touched.
pub(super) async fn account(p: &Publish, repo: &str) -> Result<Identity, ZwiimError> {
    let account = git_harness::github_account(p.gh.as_deref(), repo)
        .await
        .map_err(|e| {
            tracing::warn!(code = ?e.code, "[workspace] publish: reading the GitHub account failed: {}", e.message);
            ZwiimError::from(e)
        })?;
    Ok(Identity::from_account(&account))
}

/// Create the GitHub repository from `path` (a repository now: `created` if
/// this request just made it) and point `origin/HEAD` at its branch. On
/// failure the folder is as it was before the add, except a repository
/// GitHub kept.
pub(super) async fn publish(
    path: &Path,
    p: &Publish,
    identity: &Identity,
    created: Option<Created>,
) -> Result<(), ZwiimError> {
    let repo = path.to_string_lossy().into_owned();
    let set_identity = match created {
        Some(_) => false,
        None => repo_init::set_identity_if_unset(path, identity)
            .await
            .map_err(|e| {
                tracing::warn!("[workspace] publish: setting the commit identity failed: {e}");
                ZwiimError::repository_init_failed()
            })?,
    };
    if let Err(e) = git_harness::github_repo_create(p.gh.as_deref(), &repo, &p.owner, &p.name).await
    {
        match created {
            Some(created) => repo_init::rollback(path, created).await,
            None => {
                if set_identity {
                    repo_init::unset_identity(path).await;
                }
                // There was no `origin` before: any now is gh's.
                let _ = repo_init::git(path, &[], &["remote", "remove", "origin"]).await;
            }
        }
        return Err(create_error(p, &repo, e).await);
    }
    match current_branch(path).await {
        Some(branch) => {
            if let Err(e) =
                repo_init::git(path, &[], &["remote", "set-head", "origin", &branch]).await
            {
                tracing::warn!("[workspace] publish: setting origin/HEAD failed: {e}");
            }
        }
        None => tracing::warn!("[workspace] publish: detached HEAD; origin/HEAD not set"),
    }
    Ok(())
}

async fn create_error(p: &Publish, repo: &str, e: GitError) -> ZwiimError {
    tracing::warn!(code = ?e.code, detail = ?e.detail, "[workspace] publish: gh repo create failed: {}", e.message);
    match e.code {
        GitErrorCode::AlreadyExists => {
            ZwiimError::already_exists("A repository with this name already exists on GitHub")
        }
        GitErrorCode::GhUnavailable | GitErrorCode::GhUnauthenticated => e.into(),
        // Whatever went wrong, the repository may have landed anyway (a
        // dropped response). If not, an offline or signed-out `gh` is not
        // something another name fixes.
        _ => match git_harness::github_repo_exists(p.gh.as_deref(), repo, &p.owner, &p.name).await
        {
            Ok(true) => ZwiimError::publish_failed(true),
            _ if matches!(
                e.code,
                GitErrorCode::Network | GitErrorCode::Timeout | GitErrorCode::AuthFailed
            ) =>
            {
                e.into()
            }
            _ => ZwiimError::publish_failed(false),
        },
    }
}

/// The checked-out branch; `None` when detached.
pub(super) async fn current_branch(path: &Path) -> Option<String> {
    repo_init::git(path, &[], &["symbolic-ref", "--short", "-q", "HEAD"])
        .await
        .ok()
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
}

async fn github_status(gh: Option<&Path>, cwd: &str) -> GithubStatus {
    let unavailable = |e: GitError| GithubStatus {
        available: false,
        login: None,
        owners: None,
        reason: Some(match e.code {
            GitErrorCode::GhUnavailable => "gh_unavailable",
            GitErrorCode::GhUnauthenticated | GitErrorCode::AuthFailed => "gh_unauthenticated",
            _ => {
                tracing::warn!(code = ?e.code, "[workspace] inspect: GitHub account unavailable: {}", e.message);
                "github_unreachable"
            }
        }),
    };
    let account = match git_harness::github_account(gh, cwd).await {
        Ok(account) => account,
        Err(e) => return unavailable(e),
    };
    match git_harness::github_owners(gh, cwd, &account).await {
        Ok(owners) => GithubStatus {
            available: true,
            login: Some(account.login),
            owners: Some(owners),
            reason: None,
        },
        Err(e) => unavailable(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_names_follow_github_rules() {
        for ok in ["app", "my-app", "my_app.v2", "A1", &"a".repeat(100)] {
            assert!(valid_repo_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", "my app", "a/b", "ü", &"a".repeat(101)] {
            assert!(!valid_repo_name(bad), "{bad}");
        }
    }

    #[test]
    fn owners_are_logins() {
        assert!(valid_owner("Ashish1101"));
        assert!(valid_owner("mazik-labs"));
        // Enterprise Managed User logins contain `_`, never as the first character.
        assert!(valid_owner("a_b"));
        assert!(valid_owner("octo_corp"));
        for bad in ["", "-x", "_x", "a.b", "a/b", &"a".repeat(40)] {
            assert!(!valid_owner(bad), "{bad}");
        }
    }

    #[test]
    fn suggested_name_sanitizes_the_folder_name() {
        assert_eq!(suggested_name("my-app"), "my-app");
        assert_eq!(suggested_name("My Cool App!"), "My-Cool-App");
        assert_eq!(suggested_name("  ünï  côdé "), "n-c-d");
        assert_eq!(suggested_name("!!!"), "repository");
        assert_eq!(suggested_name("."), "repository");
        assert_eq!(suggested_name(&"a".repeat(150)).len(), 100);
    }

    #[test]
    fn check_target_refuses_bad_input() {
        assert!(check_target("octo", "app").is_ok());
        assert_eq!(
            check_target("octo", "a b").unwrap_err().code,
            "invalid_input"
        );
        assert_eq!(check_target("-x", "app").unwrap_err().code, "invalid_input");
    }
}
