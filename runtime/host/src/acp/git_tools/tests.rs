use std::path::Path;

use sea_orm::EntityTrait;
use serde_json::{json, Value};

use super::*;
use crate::acp::delegation::companion::TOOL_SCHEMA_JSON;
use crate::acp::types::{
    SessionConfigKindInfo, SessionConfigOptionInfo, SessionConfigSelectInfo,
    SessionConfigSelectOptionInfo,
};
use crate::db::service::folder_service;
use crate::db::test_helpers::{fresh_in_memory_db, seed_conversation, seed_folder};
use crate::git_harness::{MergeMethod, PullMode};
use crate::models::agent::AgentType;
use crate::web::event_bridge::EventEmitter;

// ── fixtures ────────────────────────────────────────────────────────────

fn git(dir: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A repository with one commit on `main`, its identity, signing and hooks
/// pinned so the Harness behaves the same on any machine.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "Fixture"),
        ("user.email", "fixture@example.com"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", "no-hooks"),
    ] {
        git(dir, &["config", k, v]);
    }
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git(dir, &["add", "README.md"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

struct Fixture {
    tmp: tempfile::TempDir,
    db: Arc<AppDatabase>,
    root: std::path::PathBuf,
    folder_id: i32,
    conversation_id: i32,
}

impl Fixture {
    async fn new(git_repo: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("work");
        std::fs::create_dir_all(&root).unwrap();
        if git_repo {
            init_repo(&root);
        }
        let db = Arc::new(fresh_in_memory_db().await);
        let folder_id = seed_folder(&db, root.to_str().unwrap()).await;
        let conversation_id = seed_conversation(&db, folder_id, AgentType::Codex).await;
        Self {
            tmp,
            db,
            root,
            folder_id,
            conversation_id,
        }
    }

    async fn run(&self, ctx: AgentContext, op: GitOp) -> Result<Value, GitToolError> {
        let data = self.tmp.path().join("data");
        run_for(&self.db.conn, &GitHost::new(&self.db.conn, &data), ctx, op).await
    }

    fn in_session(&self) -> AgentContext {
        AgentContext {
            conversation_id: Some(self.conversation_id),
            planning: false,
        }
    }

    fn planning(&self) -> AgentContext {
        AgentContext {
            planning: true,
            ..self.in_session()
        }
    }

    async fn conversation(&self) -> conversation::Model {
        conversation::Entity::find_by_id(self.conversation_id)
            .one(&self.db.conn)
            .await
            .unwrap()
            .unwrap()
    }

    async fn workspace_id(&self, folder_id: i32) -> String {
        folder::Entity::find_by_id(folder_id)
            .one(&self.db.conn)
            .await
            .unwrap()
            .unwrap()
            .workspace_id
            .unwrap()
    }
}

fn git_tool_schemas() -> Vec<Value> {
    let all: Value = serde_json::from_str(TOOL_SCHEMA_JSON).unwrap();
    all.as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"].as_str().unwrap().starts_with("git_"))
        .cloned()
        .collect()
}

fn op(tool: &str, args: Value) -> GitOp {
    op_from_tool_call(tool, &args).unwrap_or_else(|e| panic!("{tool} {args}: {e}"))
}

// ── tools ↔ operations ──────────────────────────────────────────────────

#[test]
fn every_operation_has_exactly_one_tool_and_back() {
    let names: Vec<String> = git_tool_schemas()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    let expected: Vec<String> = KINDS.into_iter().map(tool_name).collect();
    assert_eq!(names, expected);
    for kind in KINDS {
        assert_eq!(kind_of_tool(&tool_name(kind)), Some(kind));
    }
    assert_eq!(kind_of_tool("git_reset"), None);
    assert_eq!(kind_of_tool("status"), None);
    assert_eq!(kind_of_tool("git_"), None);
}

/// A sample value for one schema property.
fn sample(schema: &Value) -> Value {
    if let Some(value) = schema["enum"].as_array().and_then(|e| e.last()) {
        return value.clone();
    }
    match schema["type"].as_str().unwrap() {
        "string" => json!("sample"),
        "integer" => json!(3),
        "boolean" => json!(true),
        "array" => json!(["a.txt"]),
        other => panic!("unexpected schema type {other}"),
    }
}

/// The advertised schema and the Harness's typed operation agree: every
/// property maps onto a field, the required ones are required, nothing else
/// is accepted, and no tool exposes raw arguments or command execution.
#[test]
fn each_tool_schema_matches_its_operation() {
    for tool in git_tool_schemas() {
        let name = tool["name"].as_str().unwrap();
        let kind = kind_of_tool(name).unwrap();
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        let props = schema["properties"].as_object().unwrap();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|r| r.iter().map(|v| v.as_str().unwrap()).collect())
            .unwrap_or_default();
        for banned in [
            "args",
            "argv",
            "command",
            "cmd",
            "force",
            "op",
            "workspace_id",
            "cwd",
        ] {
            assert!(!props.contains_key(banned), "{name} exposes `{banned}`");
        }

        // Every property, at once.
        let full: Map<String, Value> = props.iter().map(|(k, v)| (k.clone(), sample(v))).collect();
        let parsed = op(name, Value::Object(full.clone()));
        assert_eq!(parsed.kind(), kind, "{name}");
        // And back: what serializes is exactly the advertised properties.
        let round = serde_json::to_value(&parsed).unwrap();
        let mut fields: Vec<&String> = round
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| *k != "op")
            .collect();
        fields.sort();
        let mut advertised: Vec<&String> = props.keys().collect();
        advertised.sort();
        assert_eq!(fields, advertised, "{name}");

        // Only the required ones.
        let minimal: Map<String, Value> = required
            .iter()
            .map(|k| (k.to_string(), sample(&props[*k])))
            .collect();
        assert_eq!(
            op(name, Value::Object(minimal.clone())).kind(),
            kind,
            "{name}"
        );

        // Each required one is needed.
        for missing in &required {
            let mut args = minimal.clone();
            args.remove(*missing);
            let e = op_from_tool_call(name, &Value::Object(args)).unwrap_err();
            assert!(
                e.contains(&format!("missing field `{missing}`")),
                "{name}: {e}"
            );
        }

        // Nothing else.
        let mut extra = full;
        extra.insert("extra".into(), json!("x"));
        let e = op_from_tool_call(name, &Value::Object(extra)).unwrap_err();
        assert!(e.contains("unknown field `extra`"), "{name}: {e}");
    }
}

#[test]
fn requests_map_to_the_intended_operation() {
    let cases = [
        ("git_status", json!({}), GitOp::Status),
        ("git_status", Value::Null, GitOp::Status),
        ("git_diff", json!({}), GitOp::Diff { file: None }),
        (
            "git_diff",
            json!({"file": "src/a.rs"}),
            GitOp::Diff {
                file: Some("src/a.rs".into()),
            },
        ),
        (
            "git_log",
            json!({"limit": 5}),
            GitOp::Log { limit: Some(5) },
        ),
        (
            "git_branch_create",
            json!({"name": "feat/x", "from": "main"}),
            GitOp::BranchCreate {
                name: "feat/x".into(),
                from: Some("main".into()),
            },
        ),
        (
            "git_switch",
            json!({"branch": "main"}),
            GitOp::Switch {
                branch: "main".into(),
            },
        ),
        (
            "git_stage",
            json!({"paths": ["a", "b/c"]}),
            GitOp::Stage {
                paths: vec!["a".into(), "b/c".into()],
            },
        ),
        (
            "git_commit",
            json!({"message": "m"}),
            GitOp::Commit {
                message: "m".into(),
                paths: None,
            },
        ),
        (
            "git_commit",
            json!({"message": "m", "paths": ["a"]}),
            GitOp::Commit {
                message: "m".into(),
                paths: Some(vec!["a".into()]),
            },
        ),
        ("git_fetch", json!({}), GitOp::Fetch { remote: None }),
        (
            "git_pull",
            json!({}),
            GitOp::Pull {
                mode: PullMode::FastForwardOnly,
            },
        ),
        (
            "git_pull",
            json!({"mode": "merge"}),
            GitOp::Pull {
                mode: PullMode::Merge,
            },
        ),
        (
            "git_push",
            json!({"remote": "origin"}),
            GitOp::Push {
                remote: Some("origin".into()),
            },
        ),
        (
            "git_pr_create",
            json!({"title": "T"}),
            GitOp::PrCreate {
                title: "T".into(),
                body: String::new(),
                base: None,
                draft: false,
            },
        ),
        (
            "git_pr_create",
            json!({"title": "T", "body": "B", "base": "main", "draft": true}),
            GitOp::PrCreate {
                title: "T".into(),
                body: "B".into(),
                base: Some("main".into()),
                draft: true,
            },
        ),
        ("git_pr_view", json!({}), GitOp::PrView { number: None }),
        (
            "git_pr_checks",
            json!({"number": 12}),
            GitOp::PrChecks { number: Some(12) },
        ),
        (
            "git_pr_comments",
            json!({}),
            GitOp::PrComments { number: None },
        ),
        (
            "git_pr_merge",
            json!({}),
            GitOp::PrMerge {
                number: None,
                method: MergeMethod::Merge,
            },
        ),
        (
            "git_pr_merge",
            json!({"number": 7, "method": "squash"}),
            GitOp::PrMerge {
                number: Some(7),
                method: MergeMethod::Squash,
            },
        ),
    ];
    for (tool, args, expected) in cases {
        assert_eq!(op(tool, args.clone()), expected, "{tool} {args}");
    }
}

#[test]
fn malformed_or_unknown_arguments_are_refused() {
    let cases = [
        // No raw arguments, no destructive options, no smuggled operation.
        (
            "git_push",
            json!({"args": ["--force"]}),
            "unknown field `args`",
        ),
        ("git_push", json!({"force": true}), "unknown field `force`"),
        (
            "git_pr_merge",
            json!({"delete_branch": true}),
            "unknown field `delete_branch`",
        ),
        (
            "git_pr_merge",
            json!({"method": "admin"}),
            "unknown variant `admin`",
        ),
        ("git_status", json!({"op": "push"}), "unknown field `op`"),
        (
            "git_diff",
            json!({"op": "commit", "message": "m"}),
            "unknown field `op`",
        ),
        // Status takes nothing at all.
        (
            "git_status",
            json!({"verbose": true}),
            "unknown field `verbose`",
        ),
        // Workspace scope is not the agent's to name.
        (
            "git_status",
            json!({"workspace_id": "w"}),
            "unknown field `workspace_id`",
        ),
        (
            "git_log",
            json!({"workspace_id": "w"}),
            "unknown field `workspace_id`",
        ),
        // Types.
        ("git_log", json!({"limit": "ten"}), "invalid type"),
        ("git_log", json!({"limit": -1}), "invalid value"),
        ("git_stage", json!({"paths": "a.txt"}), "invalid type"),
        ("git_pr_view", json!({"number": "12"}), "invalid type"),
        (
            "git_pull",
            json!({"mode": "rebase"}),
            "unknown variant `rebase`",
        ),
        // Shape.
        ("git_commit", json!({}), "missing field `message`"),
        ("git_status", json!(["x"]), "arguments must be an object"),
        ("git_log", json!("limit=5"), "arguments must be an object"),
        ("git_reset", json!({}), "unknown tool"),
    ];
    for (tool, args, needle) in cases {
        let e = op_from_tool_call(tool, &args).unwrap_err();
        assert!(e.contains(needle), "{tool} {args}: {e}");
    }
}

#[test]
fn read_only_tools_are_exactly_the_non_mutating_operations() {
    let read_only: Vec<String> = KINDS
        .into_iter()
        .filter(|k| !sample_op(*k).mutates())
        .map(tool_name)
        .collect();
    assert_eq!(
        read_only,
        [
            "git_status",
            "git_diff",
            "git_log",
            "git_pr_view",
            "git_pr_checks",
            "git_pr_comments"
        ]
    );
}

/// The operation a tool call with only its required arguments maps to.
fn sample_op(kind: GitOpKind) -> GitOp {
    let tool = tool_name(kind);
    let schema = git_tool_schemas()
        .into_iter()
        .find(|t| t["name"] == tool.as_str())
        .unwrap();
    let props = schema["inputSchema"]["properties"]
        .as_object()
        .unwrap()
        .clone();
    let args: Map<String, Value> = schema["inputSchema"]["required"]
        .as_array()
        .map(|r| {
            r.iter()
                .map(|k| {
                    let k = k.as_str().unwrap();
                    (k.to_string(), sample(&props[k]))
                })
                .collect()
        })
        .unwrap_or_default();
    op(&tool, Value::Object(args))
}

// ── identity ────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_caller_is_the_conversations_workspace_and_session() {
    let fx = Fixture::new(true).await;
    let caller = caller_for(&fx.db.conn, Some(fx.conversation_id))
        .await
        .unwrap();
    assert_eq!(caller.workspace_id, fx.workspace_id(fx.folder_id).await);
    let session_id = fx.conversation().await.session_id;
    assert!(session_id.is_some());
    assert_eq!(caller.session_id, session_id);
    // Neither the conversation id nor anything else agent-side leaks in.
    assert_ne!(caller.workspace_id, fx.conversation_id.to_string());
}

#[tokio::test]
async fn a_worktree_session_is_scoped_to_the_worktree() {
    let fx = Fixture::new(true).await;
    let wt = fx.tmp.path().join("wt");
    git(
        &fx.root,
        &["worktree", "add", "-q", "-b", "wt", wt.to_str().unwrap()],
    );
    let wt_row = folder_service::add_folder_with_parent(
        &fx.db.conn,
        wt.to_str().unwrap(),
        Some(fx.folder_id),
    )
    .await
    .unwrap();
    let conversation_id = seed_conversation(&fx.db, wt_row.id, AgentType::Codex).await;

    let caller = caller_for(&fx.db.conn, Some(conversation_id))
        .await
        .unwrap();
    assert_eq!(caller.workspace_id, fx.workspace_id(wt_row.id).await);
    assert_ne!(caller.workspace_id, fx.workspace_id(fx.folder_id).await);

    let ctx = AgentContext {
        conversation_id: Some(conversation_id),
        planning: false,
    };
    let status = fx.run(ctx, GitOp::Status).await.unwrap();
    assert_eq!(status["branch"], "wt");
}

#[tokio::test]
async fn without_a_conversation_there_is_no_workspace() {
    let fx = Fixture::new(true).await;
    for conversation_id in [None, Some(9_999)] {
        let ctx = AgentContext {
            conversation_id,
            planning: false,
        };
        let e = fx.run(ctx, GitOp::Status).await.unwrap_err();
        assert_eq!(e.code, "no_workspace", "{conversation_id:?}");
    }
    assert_eq!(no_workspace().code, "no_workspace");
}

#[tokio::test]
async fn a_deleted_workspace_is_no_workspace() {
    let fx = Fixture::new(true).await;
    folder_service::soft_delete_folder(&fx.db.conn, fx.folder_id)
        .await
        .unwrap();
    let e = fx.run(fx.in_session(), GitOp::Status).await.unwrap_err();
    assert_eq!(e.code, "no_workspace");
}

#[tokio::test]
async fn a_removed_worktree_is_workspace_removed() {
    let fx = Fixture::new(true).await;
    let wt = fx.tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let wt_row = folder_service::add_folder_with_parent(
        &fx.db.conn,
        wt.to_str().unwrap(),
        Some(fx.folder_id),
    )
    .await
    .unwrap();
    let conversation_id = seed_conversation(&fx.db, wt_row.id, AgentType::Codex).await;
    folder_service::soft_delete_folder(&fx.db.conn, wt_row.id)
        .await
        .unwrap();
    let ctx = AgentContext {
        conversation_id: Some(conversation_id),
        planning: false,
    };
    let e = fx.run(ctx, GitOp::Status).await.unwrap_err();
    assert_eq!(e.code, "workspace_removed");
}

#[tokio::test]
async fn a_workspace_that_is_not_a_repository_says_so() {
    let fx = Fixture::new(false).await;
    let e = fx.run(fx.in_session(), GitOp::Status).await.unwrap_err();
    assert_eq!(e.code, "not_a_git_repository");
}

// ── running ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_operation_runs_in_the_sessions_workspace() {
    let fx = Fixture::new(true).await;
    std::fs::write(fx.root.join("new.txt"), "x\n").unwrap();

    let status = fx.run(fx.in_session(), GitOp::Status).await.unwrap();
    assert_eq!(status["op"], "status");
    assert_eq!(status["branch"], "main");
    assert_eq!(status["entries"][0]["path"], "new.txt");

    let log = fx
        .run(fx.in_session(), GitOp::Log { limit: Some(1) })
        .await
        .unwrap();
    assert_eq!(log["op"], "log");
}

#[tokio::test]
async fn plan_mode_refuses_every_mutating_operation_before_it_runs() {
    let fx = Fixture::new(true).await;
    std::fs::write(fx.root.join("new.txt"), "x\n").unwrap();
    for kind in KINDS {
        let op = sample_op(kind);
        if !op.mutates() {
            continue;
        }
        let e = fx.run(fx.planning(), op).await.unwrap_err();
        assert_eq!(e.code, PLAN_MODE, "{kind:?}");
        assert!(e.message.contains("plan mode"), "{}", e.message);
    }
    // Nothing happened: the file is still untracked, still one commit.
    let status = fx.run(fx.in_session(), GitOp::Status).await.unwrap();
    assert_eq!(status["entries"][0]["status"], "??");
    assert_eq!(status["branch"], "main");
    let log = fx
        .run(fx.in_session(), GitOp::Log { limit: None })
        .await
        .unwrap();
    assert_eq!(log["entries"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn plan_mode_still_reads() {
    let fx = Fixture::new(true).await;
    for op in [
        GitOp::Status,
        GitOp::Diff { file: None },
        GitOp::Log { limit: None },
    ] {
        assert!(fx.run(fx.planning(), op).await.is_ok());
    }
}

#[tokio::test]
async fn outside_plan_mode_a_mutating_operation_runs() {
    let fx = Fixture::new(true).await;
    std::fs::write(fx.root.join("new.txt"), "x\n").unwrap();
    let commit = fx
        .run(
            fx.in_session(),
            GitOp::Commit {
                message: "add new".into(),
                paths: Some(vec!["new.txt".into()]),
            },
        )
        .await
        .unwrap();
    assert_eq!(commit["op"], "commit");
    assert_eq!(commit["files"], json!(["new.txt"]));
}

// ── branch changes under other agents ───────────────────────────────────

/// The Host's Git tools for `fx`'s session on connection `a`, mid-turn (it is
/// calling a tool), plus a second session on `b` in the same workspace,
/// working if `Some(true)`.
async fn host_tools(fx: &Fixture, b_working: Option<bool>) -> HostGitTools {
    let manager = Arc::new(ConnectionManager::new());
    let mut agents = vec![("a", fx.conversation_id, true)];
    if let Some(working) = b_working {
        let other = seed_conversation(&fx.db, fx.folder_id, AgentType::Codex).await;
        agents.push(("b", other, working));
    }
    for (id, conversation_id, working) in agents {
        manager
            .insert_test_connection(
                id,
                AgentType::Codex,
                Some(fx.root.clone()),
                EventEmitter::Noop,
            )
            .await;
        let state = manager.get_state(id).await.unwrap();
        let mut state = state.write().await;
        state.conversation_id = Some(conversation_id);
        state.turn_in_flight = working;
    }
    HostGitTools {
        manager,
        db: fx.db.clone(),
        data_dir: fx.tmp.path().join("data"),
    }
}

fn branch_create(name: &str) -> GitOp {
    GitOp::BranchCreate {
        name: name.into(),
        from: None,
    }
}

async fn branch(fx: &Fixture) -> Value {
    fx.run(fx.in_session(), GitOp::Status).await.unwrap()["branch"].clone()
}

#[tokio::test]
async fn a_branch_switch_waits_for_no_one_when_alone() {
    let fx = Fixture::new(true).await;
    let tools = host_tools(&fx, None).await;
    let created = tools.run("a", branch_create("next")).await.unwrap();
    assert_eq!(created["branch"], "next");
    assert_eq!(branch(&fx).await, "next");
}

#[tokio::test]
async fn a_branch_switch_under_another_working_agent_is_refused() {
    let fx = Fixture::new(true).await;
    git(&fx.root, &["branch", "other"]);
    let tools = host_tools(&fx, Some(true)).await;
    for op in [
        branch_create("next"),
        GitOp::Switch {
            branch: "other".into(),
        },
    ] {
        let e = tools.run("a", op.clone()).await.unwrap_err();
        assert_eq!(e.code, "workspace_busy", "{op:?}");
        assert!(e.message.contains("Another agent"), "{}", e.message);
    }
    assert_eq!(branch(&fx).await, "main");
}

#[tokio::test]
async fn an_idle_neighbour_does_not_block_a_branch_switch() {
    let fx = Fixture::new(true).await;
    let tools = host_tools(&fx, Some(false)).await;
    tools.run("a", branch_create("next")).await.unwrap();
    assert_eq!(branch(&fx).await, "next");
}

#[tokio::test]
async fn commits_are_not_gated() {
    let fx = Fixture::new(true).await;
    std::fs::write(fx.root.join("new.txt"), "x\n").unwrap();
    let tools = host_tools(&fx, Some(true)).await;
    let commit = tools
        .run(
            "a",
            GitOp::Commit {
                message: "add new".into(),
                paths: Some(vec!["new.txt".into()]),
            },
        )
        .await
        .unwrap();
    assert_eq!(commit["op"], "commit");
}

#[test]
fn plan_mode_is_read_from_the_sessions_generic_mode() {
    fn mode(current: &str) -> Vec<SessionConfigOptionInfo> {
        let value = |v: &str, kind: &str| SessionConfigSelectOptionInfo {
            value: v.into(),
            name: v.into(),
            description: None,
            kind: Some(kind.into()),
        };
        vec![SessionConfigOptionInfo {
            id: "mode".into(),
            name: "Mode".into(),
            description: None,
            category: Some("mode".into()),
            kind: SessionConfigKindInfo::Select(SessionConfigSelectInfo {
                current_value: current.into(),
                options: vec![
                    value("default", "standard"),
                    value("plan", "plan"),
                    value("bypassPermissions", "full_access"),
                ],
                groups: vec![],
            }),
            recommended_value: None,
        }]
    }
    assert!(is_planning(&mode("plan")));
    assert!(!is_planning(&mode("default")));
    assert!(!is_planning(&mode("bypassPermissions")));
    assert!(!is_planning(&[]));
}

// ── rendering ───────────────────────────────────────────────────────────

#[tokio::test]
async fn harness_errors_render_as_mcp_errors_with_their_code() {
    let fx = Fixture::new(false).await;
    let result = fx.run(fx.in_session(), GitOp::Status).await;
    let rendered = render_result(&reply(result));
    assert_eq!(rendered["isError"], true);
    let text = rendered["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("not_a_git_repository: "), "{text}");
    assert_eq!(
        rendered["structuredContent"]["code"],
        "not_a_git_repository"
    );
}

#[test]
fn errors_carry_their_detail_and_plan_mode_its_code() {
    let e: GitToolError = GitError::new(GitErrorCode::NonFastForward, "git push: rejected")
        .with_detail("! [rejected] main -> main (fetch first)")
        .into();
    let rendered = render_result(&reply(Err(e)));
    assert_eq!(rendered["isError"], true);
    assert_eq!(
        rendered["content"][0]["text"],
        "non_fast_forward: git push: rejected\n\n! [rejected] main -> main (fetch first)"
    );
    assert_eq!(rendered["structuredContent"]["code"], "non_fast_forward");

    let plan = GitToolError {
        code: PLAN_MODE.into(),
        message: "m".into(),
        detail: None,
    };
    let rendered = render_result(&reply(Err(plan)));
    assert_eq!(rendered["content"][0]["text"], "plan_mode: m");
    assert!(rendered["structuredContent"].get("detail").is_none());
}

#[test]
fn outcomes_render_as_their_json() {
    let outcome = json!({"op": "fetch", "remote": "origin"});
    let rendered = render_result(&reply(Ok(outcome.clone())));
    assert_eq!(rendered["isError"], false);
    assert_eq!(rendered["structuredContent"], outcome);
    let text = rendered["content"][0]["text"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(text).unwrap(), outcome);
}

// ── boundary ────────────────────────────────────────────────────────────

/// The adapter hands the Harness only a `GitCaller` and a `GitOp`; it is the
/// one place that knows about connections, conversations and plan mode. The
/// Harness itself is held to that by its own source guard
/// (`the_harness_knows_nothing_about_agent_integrations`); this checks the
/// direction the other way: nothing in the Harness names this adapter.
#[test]
fn the_harness_does_not_know_this_adapter() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/git_harness");
    let mut files = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src/git_harness.rs")];
    files.extend(
        std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap() != "tests.rs"),
    );
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap().to_lowercase();
        for banned in [
            "git_tools",
            "gittoolaccess",
            "agentcontext",
            "plan_mode",
            "planning",
        ] {
            assert!(
                !text.contains(banned),
                "{} mentions {banned}",
                file.display()
            );
        }
    }
}

#[test]
fn every_git_tool_has_a_readable_title_in_every_agent_spelling() {
    let mut seen = std::collections::HashSet::new();
    for kind in KINDS {
        let title = display_title(kind);
        assert!(seen.insert(title), "{title} is used twice");
        assert!(
            !title.contains('_') && !title.to_lowercase().contains("mcp"),
            "{title}"
        );
        for prefix in AGENT_TOOL_PREFIXES {
            let raw = format!("{prefix}{}", tool_name(kind));
            assert_eq!(kind_of_title(&raw), Some(kind), "{raw}");
            assert_eq!(
                readable_tool_names(&format!("Tool: {raw}")),
                format!("Tool: {title}")
            );
        }
    }
    for (kind, title) in [
        (GitOpKind::Status, "Git status"),
        (GitOpKind::Commit, "Commit"),
        (GitOpKind::Push, "Push branch"),
        (GitOpKind::PrCreate, "Create pull request"),
        (GitOpKind::PrMerge, "Merge pull request"),
        (GitOpKind::PrChecks, "Check PR status"),
    ] {
        assert_eq!(display_title(kind), title);
    }
    assert_eq!(kind_of_title("git_status"), None, "only zwiim-mcp's tools");
    assert_eq!(kind_of_title("mcp__zwiim-mcp__git_unknown"), None);
    assert_eq!(kind_of_title("mcp__zwiim-mcp__git_pushy"), None);
    assert_eq!(
        display_result(r#"{"op":"pr_create","number":null,"url":"u"}"#).as_deref(),
        Some("Opened u")
    );
    assert_eq!(
        display_result(r#"{"op":"pr_merge","number":4}"#).as_deref(),
        Some("Merged #4")
    );
    assert_eq!(display_result("not json"), None);
}
