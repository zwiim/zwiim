//! The signed-in GitHub account, through the user's own `gh`: who the user
//! is, which owners they can create a repository under, whether a name is
//! taken, and creating the private repository a new workspace is published
//! to. Nothing here stores or passes a token.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::error::{classify_gh, GitError, GitErrorCode};
use super::run::{self, Output, GITHUB_READ, REMOTE};

/// `gh api user`, the fields Zwiim reads.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GithubAccount {
    pub login: String,
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GithubOwnerKind {
    User,
    Org,
}

/// An account a repository can be created under.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GithubOwner {
    pub login: String,
    pub kind: GithubOwnerKind,
}

async fn gh(
    gh: Option<&Path>,
    cwd: &str,
    what: &str,
    args: &[&str],
    limit: Duration,
) -> Result<Output, GitError> {
    let program = run::find_gh(gh)?;
    let mut cmd = run::gh(&program, cwd);
    cmd.args(args);
    let out = run::run(cmd, limit, &format!("gh {what}")).await?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(classify_gh(what, out.status.code(), &out.text()))
    }
}

/// The account `gh` is signed in as.
pub async fn github_account(gh_path: Option<&Path>, cwd: &str) -> Result<GithubAccount, GitError> {
    let out = gh(gh_path, cwd, "api user", &["api", "user"], GITHUB_READ).await?;
    serde_json::from_str(out.stdout.trim()).map_err(|e| {
        GitError::new(GitErrorCode::Failed, "gh api user: unexpected output")
            .with_detail(&e.to_string())
    })
}

/// `account`'s login, then its organizations in GitHub's order. Should the
/// organizations not list, the login alone.
pub async fn github_owners(
    gh_path: Option<&Path>,
    cwd: &str,
    account: &GithubAccount,
) -> Result<Vec<GithubOwner>, GitError> {
    let mut owners = vec![GithubOwner {
        login: account.login.clone(),
        kind: GithubOwnerKind::User,
    }];
    let args = ["api", "user/orgs", "--paginate", "--jq", ".[].login"];
    match gh(gh_path, cwd, "api user/orgs", &args, GITHUB_READ).await {
        Ok(out) => owners.extend(
            out.stdout
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|login| GithubOwner {
                    login: login.to_string(),
                    kind: GithubOwnerKind::Org,
                }),
        ),
        Err(e) => {
            tracing::warn!(code = ?e.code, "[github] listing organizations failed: {}", e.message)
        }
    }
    Ok(owners)
}

/// Whether `owner/name` exists on GitHub (and the account can see it).
pub async fn github_repo_exists(
    gh_path: Option<&Path>,
    cwd: &str,
    owner: &str,
    name: &str,
) -> Result<bool, GitError> {
    let path = format!("repos/{owner}/{name}");
    match gh(
        gh_path,
        cwd,
        "api repos",
        &["api", &path, "--jq", ".id"],
        GITHUB_READ,
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(e) if e.code == GitErrorCode::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Create private `owner/name` from the repository at `path`, add it as
/// `origin` and push the current branch.
pub async fn github_repo_create(
    gh_path: Option<&Path>,
    path: &str,
    owner: &str,
    name: &str,
) -> Result<(), GitError> {
    let full = format!("{owner}/{name}");
    let args = [
        "repo",
        "create",
        &full,
        "--private",
        "--source",
        path,
        "--remote",
        "origin",
        "--push",
    ];
    gh(gh_path, path, "repo create", &args, REMOTE)
        .await
        .map(|_| ())
}
