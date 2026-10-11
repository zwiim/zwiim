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

/// One repository the signed-in account can see, as `github.repos` lists it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GithubRepoRow {
    pub owner: String,
    pub name: String,
    pub private: bool,
    pub fork: bool,
    pub archived: bool,
    pub description: Option<String>,
    pub pushed_at: Option<String>,
    pub size_kb: u64,
    pub default_branch: Option<String>,
}

/// `gh api user/repos`, the fields Zwiim reads.
#[derive(Deserialize)]
struct RepoJson {
    full_name: String,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    fork: bool,
    #[serde(default)]
    archived: bool,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    pushed_at: Option<String>,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    default_branch: Option<String>,
}

/// Rows per `github.repos` page; a shorter page is the last.
pub(super) const REPOS_PER_PAGE: usize = 100;

fn parse_repos(json: &str, page: u32) -> Result<(Vec<GithubRepoRow>, Option<u32>), GitError> {
    let raw: Vec<RepoJson> = serde_json::from_str(json.trim()).map_err(|e| {
        GitError::new(GitErrorCode::Failed, "gh api user/repos: unexpected output")
            .with_detail(&e.to_string())
    })?;
    let next = if raw.len() >= REPOS_PER_PAGE {
        page.checked_add(1)
    } else {
        None
    };
    let rows = raw
        .into_iter()
        .filter_map(|r| {
            let (owner, name) = r.full_name.split_once('/')?;
            Some(GithubRepoRow {
                owner: owner.to_string(),
                name: name.to_string(),
                private: r.private,
                fork: r.fork,
                archived: r.archived,
                description: r.description,
                pushed_at: r.pushed_at,
                size_kb: r.size,
                default_branch: r.default_branch,
            })
        })
        .collect();
    Ok((rows, next))
}

/// One page (from 1) of the repositories the account owns, collaborates on,
/// or sees through an organization, most recently pushed first, and the next
/// page when this one was full. One page per call: an organization member can
/// see thousands.
pub async fn github_repos(
    gh_path: Option<&Path>,
    cwd: &str,
    page: u32,
) -> Result<(Vec<GithubRepoRow>, Option<u32>), GitError> {
    let endpoint = format!(
        "user/repos?affiliation=owner,collaborator,organization_member&sort=pushed&per_page={REPOS_PER_PAGE}&page={page}"
    );
    let out = gh(
        gh_path,
        cwd,
        "api user/repos",
        &["api", &endpoint],
        GITHUB_READ,
    )
    .await?;
    parse_repos(&out.stdout, page)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_REPOS: &str = r#"[
  {"full_name":"octo/app","private":true,"fork":false,"archived":false,
   "description":"The app","pushed_at":"2026-10-01T10:00:00Z","size":1234,
   "default_branch":"main","owner":{"login":"octo"}},
  {"full_name":"acme/lib","private":false,"fork":true,"archived":true,
   "description":null,"pushed_at":null,"size":0,"default_branch":null}
]"#;

    #[test]
    fn parse_repos_reads_rows_and_ends_on_a_short_page() {
        let (rows, next) = parse_repos(TWO_REPOS, 1).unwrap();
        assert_eq!(next, None);
        assert_eq!(
            rows[0],
            GithubRepoRow {
                owner: "octo".into(),
                name: "app".into(),
                private: true,
                fork: false,
                archived: false,
                description: Some("The app".into()),
                pushed_at: Some("2026-10-01T10:00:00Z".into()),
                size_kb: 1234,
                default_branch: Some("main".into()),
            }
        );
        assert!(rows[1].fork && rows[1].archived && rows[1].description.is_none());
    }

    #[test]
    fn parse_repos_offers_the_next_page_after_a_full_one() {
        let one = r#"{"full_name":"o/n","private":false,"size":1}"#;
        let full = format!("[{}]", vec![one; REPOS_PER_PAGE].join(","));
        assert_eq!(parse_repos(&full, 3).unwrap().1, Some(4));
    }

    #[test]
    fn parse_repos_rejects_unexpected_output() {
        let e = parse_repos("<html>", 1).unwrap_err();
        assert_eq!(e.code, GitErrorCode::Failed);
    }

    #[tokio::test]
    async fn github_repos_asks_for_one_page() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "\"api user/repos\"*) echo '{}';;",
            TWO_REPOS.replace('\n', " ")
        );
        let (gh, calls) = crate::git_harness::fake_gh(dir.path(), &body);
        let (rows, next) = github_repos(Some(&gh), dir.path().to_str().unwrap(), 2)
            .await
            .unwrap();
        assert_eq!((rows.len(), next), (2, None));
        let calls = std::fs::read_to_string(calls).unwrap();
        assert!(
            calls.contains(
                "api user/repos?affiliation=owner,collaborator,organization_member&sort=pushed&per_page=100&page=2 | prompt=1"
            ),
            "{calls}"
        );
    }
}
