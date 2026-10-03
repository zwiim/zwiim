//! Pull requests through the user's installed GitHub CLI and its own login.
//! The Host never bundles `gh` and never hands it a token.

use std::path::PathBuf;

use serde::Deserialize;

use super::error::{classify_gh, GitError, GitErrorCode};
use super::local::{not_an_option, require_branch};
use super::op::{
    MergeMethod, PrCheck, PrComment, PrComments, PrCreated, PrMerged, PrReview, PrReviewComment,
    PrSummary,
};
use super::run::{self, Output, GITHUB};
use super::GitHost;

const PR_FIELDS: &str =
    "number,title,state,url,isDraft,baseRefName,headRefName,mergeable,reviewDecision,author,body";

pub(super) async fn pr_create(
    host: &GitHost<'_>,
    root: &str,
    title: &str,
    body: &str,
    base: Option<&str>,
    draft: bool,
) -> Result<PrCreated, GitError> {
    if title.trim().is_empty() {
        return Err(GitError::new(
            GitErrorCode::InvalidInput,
            "A pull request needs a title",
        ));
    }
    let branch = require_branch(root).await?;
    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        "--title".to_string(),
        title.to_string(),
        "--body".to_string(),
        body.to_string(),
        "--head".to_string(),
        branch,
    ];
    if let Some(base) = base {
        not_an_option("base", base)?;
        args.extend(["--base".to_string(), base.to_string()]);
    }
    if draft {
        args.push("--draft".to_string());
    }
    let out = gh(host, root, "pr create", &args).await?;
    // gh prints the new pull request's URL last.
    let url = out
        .stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with("http"))
        .unwrap_or_default()
        .to_string();
    Ok(PrCreated {
        number: number_from_url(&url),
        url,
    })
}

pub(super) async fn pr_view(
    host: &GitHost<'_>,
    root: &str,
    number: Option<u64>,
) -> Result<PrSummary, GitError> {
    let mut args = pr_args("view", number);
    args.extend(["--json".to_string(), PR_FIELDS.to_string()]);
    let out = gh(host, root, "pr view", &args).await?;
    let mut pr: PrSummary = parse("pr view", &out.stdout)?;
    pr.review_decision = pr.review_decision.filter(|d| !d.is_empty());
    Ok(pr)
}

pub(super) async fn pr_checks(
    host: &GitHost<'_>,
    root: &str,
    number: Option<u64>,
) -> Result<Vec<PrCheck>, GitError> {
    let mut args = pr_args("checks", number);
    args.extend([
        "--json".to_string(),
        "name,state,bucket,link,workflow".to_string(),
    ]);
    let out = gh_output(host, root, "pr checks", &args).await?;
    // gh exits 8 while checks are pending and 1 when one failed; the JSON is
    // the answer either way.
    if let Ok(checks) = serde_json::from_str::<Vec<PrCheck>>(out.stdout.trim()) {
        return Ok(checks);
    }
    if out.stderr.to_lowercase().contains("no checks reported") {
        return Ok(Vec::new());
    }
    Err(classify_gh("pr checks", out.status.code(), &out.text()))
}

pub(super) async fn pr_comments(
    host: &GitHost<'_>,
    root: &str,
    number: Option<u64>,
) -> Result<PrComments, GitError> {
    #[derive(Deserialize)]
    struct Conversation {
        number: u64,
        #[serde(default)]
        comments: Vec<PrComment>,
        #[serde(default)]
        reviews: Vec<PrReview>,
    }
    #[derive(Deserialize)]
    struct ApiComment {
        user: Option<ApiUser>,
        #[serde(default)]
        path: String,
        line: Option<u64>,
        original_line: Option<u64>,
        #[serde(default)]
        body: String,
        #[serde(default)]
        created_at: String,
        #[serde(default)]
        html_url: String,
    }
    #[derive(Deserialize)]
    struct ApiUser {
        login: String,
    }

    let mut args = pr_args("view", number);
    args.extend(["--json".to_string(), "number,comments,reviews".to_string()]);
    let out = gh(host, root, "pr view", &args).await?;
    let conversation: Conversation = parse("pr view", &out.stdout)?;

    // `{owner}/{repo}` is filled in by gh from the workspace's remote.
    let api = [
        "api".to_string(),
        format!(
            "repos/{{owner}}/{{repo}}/pulls/{}/comments?per_page=100",
            conversation.number
        ),
    ];
    let out = gh(host, root, "api", &api).await?;
    let review_comments: Vec<ApiComment> = parse("api", &out.stdout)?;

    Ok(PrComments {
        number: conversation.number,
        comments: conversation.comments,
        reviews: conversation.reviews,
        review_comments: review_comments
            .into_iter()
            .map(|c| PrReviewComment {
                author: c.user.map(|u| u.login),
                path: c.path,
                line: c.line.or(c.original_line),
                body: c.body,
                created_at: c.created_at,
                url: c.html_url,
            })
            .collect(),
    })
}

/// Merge the pull request now with `method`. Keeps the head branch; never
/// bypasses branch protection or queues an automatic merge.
pub(super) async fn pr_merge(
    host: &GitHost<'_>,
    root: &str,
    number: Option<u64>,
    method: MergeMethod,
) -> Result<PrMerged, GitError> {
    // Pin the number first so the merge acts on exactly the pull request
    // reported back.
    let number = match number {
        Some(n) => n,
        None => pr_view(host, root, None).await?.number,
    };
    let args = [
        "pr".to_string(),
        "merge".to_string(),
        number.to_string(),
        method.flag().to_string(),
    ];
    gh(host, root, "pr merge", &args).await?;
    Ok(PrMerged { number, method })
}

fn pr_args(sub: &str, number: Option<u64>) -> Vec<String> {
    let mut args = vec!["pr".to_string(), sub.to_string()];
    if let Some(n) = number {
        args.push(n.to_string());
    }
    args
}

/// Run `gh <args>`; a non-zero exit is a classified failure.
async fn gh(
    host: &GitHost<'_>,
    root: &str,
    what: &str,
    args: &[String],
) -> Result<Output, GitError> {
    let out = gh_output(host, root, what, args).await?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(classify_gh(what, out.status.code(), &out.text()))
    }
}

async fn gh_output(
    host: &GitHost<'_>,
    root: &str,
    what: &str,
    args: &[String],
) -> Result<Output, GitError> {
    let program: PathBuf = run::find_gh(host.gh)?;
    let mut cmd = run::gh(&program, root);
    cmd.args(args);
    run::run(cmd, GITHUB, &format!("gh {what}")).await
}

fn parse<T: serde::de::DeserializeOwned>(what: &str, stdout: &str) -> Result<T, GitError> {
    serde_json::from_str(stdout.trim()).map_err(|e| {
        GitError::new(
            GitErrorCode::Failed,
            format!("gh {what}: unexpected output"),
        )
        .with_detail(&e.to_string())
    })
}

fn number_from_url(url: &str) -> Option<u64> {
    let (_, tail) = url.rsplit_once("/pull/")?;
    tail.trim_end_matches('/').parse().ok()
}
