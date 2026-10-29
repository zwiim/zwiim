//! Pull requests through the user's installed GitHub CLI and its own login.
//! The Host never bundles `gh` and never hands it a token.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use super::error::{classify_gh, GitError, GitErrorCode};
use super::local::{not_an_option, require_branch};
use super::op::{
    AutoMergeOutcome, MergeMethod, PrAutoMerged, PrCheck, PrComment, PrComments, PrCreated,
    PrMerged, PrReview, PrReviewComment, PrSummary,
};
use super::pr_state::{
    MergeStatus, PrLifecycle, PrState, RawPr, RepoMergeSettings, PR_STATE_FIELDS, REPO_MERGE_FIELDS,
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

/// The pull request for `branch`, merged and closed ones included; `None`
/// when the branch has none.
pub(super) async fn pr_state(
    host: &GitHost<'_>,
    root: &str,
    branch: &str,
) -> Result<Option<PrState>, GitError> {
    not_an_option("branch", branch)?;
    let raw = match view_raw(host, root, branch).await {
        Ok(raw) => raw,
        Err(e) if e.code == GitErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let methods = merge_methods(host, root).await?;
    Ok(Some(raw.into_state(methods)))
}

/// Merge pull request `number` (else the current branch's) now with `method`
/// (else the repository's preferred one). With `expected_head`, only if its
/// head is still that commit.
///
/// Merges only what GitHub would merge right now: a pull request it reports
/// not mergeable is refused before `gh` runs, the merge is pinned to the head
/// it was checked at, and a merge `gh` queued for later instead (auto-merge)
/// is turned off again and reported as `not_mergeable` (an auto-merge the
/// user turned on before stays on). Keeps the head branch; never bypasses
/// branch protection.
pub(super) async fn pr_merge(
    host: &GitHost<'_>,
    root: &str,
    number: Option<u64>,
    method: Option<MergeMethod>,
    expected_head: Option<&str>,
) -> Result<PrMerged, GitError> {
    // Pin the number first so the merge acts on exactly the pull request
    // reported back.
    let target = match number {
        Some(n) => n.to_string(),
        None => pr_view(host, root, None).await?.number.to_string(),
    };
    let pr = view_known(host, root, &target).await?;
    let number = pr.number;
    check_head(&pr, expected_head)?;
    let methods = merge_methods(host, root).await?;
    let method = method
        .or_else(|| methods.first().copied())
        .unwrap_or_default();
    let head = pr.head_ref_oid.clone();
    let was_auto = pr.auto_merge_request.is_some();
    let state = pr.into_state(methods);
    if !state.merge.allowed {
        let why = match state.state {
            PrLifecycle::Merged => "it is already merged".to_string(),
            PrLifecycle::Closed => "it is closed".to_string(),
            _ => state
                .merge
                .reason
                .clone()
                .unwrap_or_else(|| "GitHub will not merge it now".to_string()),
        };
        return Err(GitError::new(
            GitErrorCode::NotMergeable,
            format!("Pull request #{number} cannot be merged: {why}"),
        ));
    }
    let mut args = vec![
        "pr".to_string(),
        "merge".to_string(),
        number.to_string(),
        method.flag().to_string(),
    ];
    if !head.is_empty() {
        args.extend(["--match-head-commit".to_string(), head]);
    }
    gh(host, root, "pr merge", &args).await?;

    let after = view_raw(host, root, &number.to_string()).await?;
    if after.lifecycle() == PrLifecycle::Merged {
        return Ok(PrMerged {
            number,
            method,
            merged_at: after.merged_at,
        });
    }
    if after.auto_merge_request.is_some() && was_auto {
        // The user's own auto-merge stays on.
        return Err(GitError::new(
            GitErrorCode::NotMergeable,
            format!("Pull request #{number} is set to merge once GitHub's requirements are met"),
        ));
    }
    if after.auto_merge_request.is_some() {
        let disable = [
            "pr".to_string(),
            "merge".to_string(),
            number.to_string(),
            "--disable-auto".to_string(),
        ];
        if let Err(e) = gh(host, root, "pr merge --disable-auto", &disable).await {
            tracing::warn!(
                number,
                code = e.code.as_str(),
                "[git-harness] could not turn auto-merge off"
            );
        }
        return Err(GitError::new(
            GitErrorCode::NotMergeable,
            format!(
                "Pull request #{number} was queued to merge later instead of merging now; \
                 the queued merge was turned off"
            ),
        ));
    }
    Err(GitError::new(
        GitErrorCode::NotMergeable,
        format!("GitHub did not merge pull request #{number}"),
    ))
}

/// Turn auto-merge for pull request `number` on (`enabled`), so GitHub
/// merges it with `method` (else the repository's preferred one) once its
/// requirements are met, or off. Turning it on is pinned to `expected_head`.
///
/// A pull request GitHub would merge already is merged now instead, through
/// [`pr_merge`]'s checks. Only one that waits on required checks or review
/// takes auto-merge: one that is behind its base, conflicted or a draft is
/// refused as `not_mergeable`. Turning on or off what already is so is a
/// no-op. Never bypasses branch protection.
pub(super) async fn pr_auto_merge(
    host: &GitHost<'_>,
    root: &str,
    number: u64,
    enabled: bool,
    method: Option<MergeMethod>,
    expected_head: Option<&str>,
) -> Result<PrAutoMerged, GitError> {
    let target = number.to_string();
    let pr = if enabled {
        view_known(host, root, &target).await?
    } else {
        view_raw(host, root, &target).await?
    };
    let auto_method = |pr: &RawPr| {
        pr.auto_merge_request
            .as_ref()
            .and_then(|a| a.merge_method.as_deref())
            .and_then(MergeMethod::from_github)
    };
    let merged = |method| PrAutoMerged {
        number,
        outcome: AutoMergeOutcome::Merged,
        method,
    };
    if pr.lifecycle() == PrLifecycle::Merged {
        return Ok(merged(auto_method(&pr)));
    }
    if !enabled {
        if pr.auto_merge_request.is_some() {
            let args = [
                "pr".to_string(),
                "merge".to_string(),
                target.clone(),
                "--disable-auto".to_string(),
            ];
            gh(host, root, "pr merge --disable-auto", &args).await?;
        }
        return Ok(PrAutoMerged {
            number,
            outcome: AutoMergeOutcome::Off,
            method: None,
        });
    }
    check_head(&pr, expected_head)?;
    if pr.auto_merge_request.is_some() {
        return Ok(PrAutoMerged {
            number,
            outcome: AutoMergeOutcome::On,
            method: auto_method(&pr),
        });
    }
    let methods = merge_methods(host, root).await?;
    let method = method
        .or_else(|| methods.first().copied())
        .unwrap_or_default();
    let head = pr.head_ref_oid.clone();
    let state = pr.into_state(methods);
    if state.merge.allowed {
        let done = pr_merge(host, root, Some(number), Some(method), expected_head).await?;
        return Ok(merged(Some(done.method)));
    }
    if !state.merge.auto_available {
        let why = match state.state {
            PrLifecycle::Closed => "it is closed".to_string(),
            _ => state
                .merge
                .reason
                .clone()
                .unwrap_or_else(|| "GitHub will not queue it".to_string()),
        };
        return Err(GitError::new(
            GitErrorCode::NotMergeable,
            format!("Auto-merge cannot be turned on for pull request #{number}: {why}"),
        ));
    }
    let mut args = vec![
        "pr".to_string(),
        "merge".to_string(),
        target.clone(),
        "--auto".to_string(),
        method.flag().to_string(),
    ];
    if !head.is_empty() {
        args.extend(["--match-head-commit".to_string(), head]);
    }
    if let Err(e) = gh(host, root, "pr merge --auto", &args).await {
        // Its checks finished between the read and the request: GitHub
        // refuses to queue a pull request it would merge now.
        let detail = e.detail.as_deref().unwrap_or_default().to_lowercase();
        if detail.contains("is in clean status") || detail.contains("is in unstable status") {
            let done = pr_merge(host, root, Some(number), Some(method), expected_head).await?;
            return Ok(merged(Some(done.method)));
        }
        return Err(e);
    }
    let after = view_raw(host, root, &target).await?;
    if after.lifecycle() == PrLifecycle::Merged {
        return Ok(merged(Some(method)));
    }
    if after.auto_merge_request.is_some() {
        return Ok(PrAutoMerged {
            number,
            outcome: AutoMergeOutcome::On,
            method: auto_method(&after).or(Some(method)),
        });
    }
    Err(GitError::new(
        GitErrorCode::Failed,
        format!("GitHub did not turn auto-merge on for pull request #{number}"),
    ))
}

/// Refuse a pull request whose head moved from `expected`.
fn check_head(pr: &RawPr, expected: Option<&str>) -> Result<(), GitError> {
    match expected {
        Some(expected) if pr.head_ref_oid != expected => Err(GitError::new(
            GitErrorCode::PrChanged,
            format!(
                "Pull request #{} has new commits; review them and merge again",
                pr.number
            ),
        )),
        _ => Ok(()),
    }
}

/// [`view_raw`], waiting out GitHub's lazy mergeability: the first read after
/// a push is often UNKNOWN.
async fn view_known(host: &GitHost<'_>, root: &str, target: &str) -> Result<RawPr, GitError> {
    let mut pr = view_raw(host, root, target).await?;
    for _ in 0..UNKNOWN_RETRIES {
        if pr.merge_status() != MergeStatus::Unknown || pr.lifecycle().is_final() {
            break;
        }
        tokio::time::sleep(UNKNOWN_RETRY_DELAY).await;
        pr = view_raw(host, root, target).await?;
    }
    Ok(pr)
}

const UNKNOWN_RETRIES: usize = 3;
const UNKNOWN_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

async fn view_raw(host: &GitHost<'_>, root: &str, target: &str) -> Result<RawPr, GitError> {
    let args = [
        "pr".to_string(),
        "view".to_string(),
        target.to_string(),
        "--json".to_string(),
        PR_STATE_FIELDS.to_string(),
    ];
    let out = gh(host, root, "pr view", &args).await?;
    parse("pr view", &out.stdout)
}

/// The merge methods `root`'s repository allows, preferred first. Read once
/// per repository root for the life of the process: repository settings
/// rarely change, and a merge GitHub refuses says so anyway.
async fn merge_methods(host: &GitHost<'_>, root: &str) -> Result<Vec<MergeMethod>, GitError> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<MergeMethod>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(methods) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(root) {
        return Ok(methods.clone());
    }
    let args = [
        "repo".to_string(),
        "view".to_string(),
        "--json".to_string(),
        REPO_MERGE_FIELDS.to_string(),
    ];
    let out = gh(host, root, "repo view", &args).await?;
    let settings: RepoMergeSettings = parse("repo view", &out.stdout)?;
    let methods = settings.methods();
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(root.to_string(), methods.clone());
    Ok(methods)
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
