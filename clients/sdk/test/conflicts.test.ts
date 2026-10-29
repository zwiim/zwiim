import { describe, expect, it } from "vitest";
import { ZwiimClientError } from "../src/client/errors";
import { conflictView, resolveLabel, resolveRefusalText } from "../src/state/conflicts";
import { initialSessionView } from "../src/state/session";
import { initialHostState, reduceHost, type HostState } from "../src/state/store";
import type { ConflictCheck, SessionSummary, WorkspaceInfo } from "../src/types/protocol";

const root: WorkspaceInfo = {
  workspace_id: "root",
  name: "repo",
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
};
const wt: WorkspaceInfo = {
  ...root,
  workspace_id: "a",
  name: "a",
  kind: "worktree",
  parent_workspace_id: "root",
  branch: "zwiim/a",
  base_branch: "main",
  base_commit: "3f9e".padEnd(40, "0"),
};
const session = (id: string, extra: Partial<SessionSummary> = {}): SessionSummary => ({
  session_id: id,
  workspace_id: "a",
  title: null,
  agent: "codex",
  status: "pending_review",
  live: false,
  created_at: "2026-10-29T00:00:00Z",
  updated_at: "2026-10-29T00:00:00Z",
  ...extra,
});
const shas = { base_sha: "b".repeat(40), head_sha: "h".repeat(40) };
const conflicts = (files: string[]): ConflictCheck => ({ base: "origin/main", ...shas, state: "conflicts", files });
const inProgress = (operation: string, files: string[]): ConflictCheck => ({
  base: "origin/main",
  ...shas,
  state: "in_progress",
  operation,
  files,
});
const ALL = ["workspace.conflicts", "workspace.resolve_conflicts"];

function stateWith({
  check,
  sessions = [session("s1")],
  capabilities = ALL,
  resolving,
  workspace = wt,
}: {
  check?: ConflictCheck | null;
  sessions?: SessionSummary[];
  capabilities?: string[];
  resolving?: string;
  workspace?: WorkspaceInfo;
}): HostState {
  let s = reduceHost(initialHostState(), { type: "workspaces", workspaces: [root, workspace] });
  s = reduceHost(s, { type: "sessions", workspace_id: workspace.workspace_id, sessions });
  return {
    ...s,
    connection: { ...s.connection, status: "live", compat: { status: "compatible", hostRevision: 1, capabilities } },
    conflicts: check ? { [workspace.workspace_id]: check } : {},
    resolving: resolving ? { [workspace.workspace_id]: resolving } : {},
  };
}

describe("conflict view", () => {
  it("names the files that conflict with the base, without origin/", () => {
    const view = conflictView(stateWith({ check: conflicts(["src/a.ts", "src/b.ts", "README.md"]) }), "a");
    expect(view).toEqual({
      line: "3 files conflict with main",
      files: ["src/a.ts", "src/b.ts", "README.md"],
      resolve: { state: "ready", session_id: "s1" },
    });
    expect(conflictView(stateWith({ check: conflicts(["src/a.ts"]) }), "a")?.line).toBe("1 file conflicts with main");
  });

  it("says a stopped merge or rebase is in progress, with what is unresolved", () => {
    expect(conflictView(stateWith({ check: inProgress("merge", ["a", "b"]) }), "a")?.line).toBe(
      "Merge in progress: 2 files unresolved",
    );
    expect(conflictView(stateWith({ check: inProgress("rebase", ["a"]) }), "a")?.line).toBe(
      "Rebase in progress: 1 file unresolved",
    );
    expect(conflictView(stateWith({ check: inProgress("unmerged", ["a"]) }), "a")?.line).toBe("1 file unresolved");
  });

  it("offers no resolve once nothing is unresolved: the Host would say no_conflicts", () => {
    const view = conflictView(stateWith({ check: inProgress("merge", []) }), "a");
    expect(view).toEqual({ line: "Merge in progress", files: [], resolve: null });
  });

  it("is nothing when the branch is clean or unchecked", () => {
    expect(conflictView(stateWith({ check: { base: "origin/main", ...shas, state: "clean" } }), "a")).toBeNull();
    expect(conflictView(stateWith({}), "a")).toBeNull();
  });

  it("is nothing on an older Host that doesn't check conflicts", () => {
    expect(conflictView(stateWith({ check: conflicts(["a"]), capabilities: [] }), "a")).toBeNull();
  });

  it("shows the conflicts without a resolve on a Host that can't resolve them", () => {
    const view = conflictView(stateWith({ check: conflicts(["a"]), capabilities: ["workspace.conflicts"] }), "a");
    expect(view).toMatchObject({ line: "1 file conflicts with main", resolve: null });
  });

  it("shows a root's conflicts without a resolve: no session runs there", () => {
    const view = conflictView(stateWith({ check: conflicts(["a"]), workspace: { ...root, workspace_id: "r2" } }), "r2");
    expect(view).toMatchObject({ line: "1 file conflicts with main", resolve: null });
  });

  it("resolves in the session shown when it is in the worktree, else the newest", () => {
    const state = stateWith({ check: conflicts(["a"]), sessions: [session("new"), session("old")] });
    expect(conflictView(state, "a", "old")?.resolve).toEqual({ state: "ready", session_id: "old" });
    expect(conflictView(state, "a", "elsewhere")?.resolve).toEqual({ state: "ready", session_id: "new" });
    expect(conflictView(state, "a")?.resolve).toEqual({ state: "ready", session_id: "new" });
  });

  it("asks for a chat first when the worktree has none", () => {
    expect(conflictView(stateWith({ check: conflicts(["a"]), sessions: [] }), "a")?.resolve).toEqual({
      state: "blocked",
      reason: "Start a chat here first, then resolve conflicts.",
    });
  });

  it("waits while any agent in the worktree is working", () => {
    const busy = stateWith({ check: conflicts(["a"]), sessions: [session("s1"), session("s2", { live: true, status: "in_progress" })] });
    expect(conflictView(busy, "a", "s1")?.resolve).toEqual({
      state: "blocked",
      reason: "An agent is working here. Wait for it or stop it, then resolve conflicts.",
    });
    // A live view is fresher than the summary.
    const idle = { ...busy, sessions: { s2: { ...initialSessionView("s2"), attach: "live" as const, agentStatus: "connected" as const } } };
    expect(conflictView(idle, "a", "s1")?.resolve).toEqual({ state: "ready", session_id: "s1" });
  });

  it("follows the session resolving, even once the conflicts have cleared", () => {
    const busy = [session("s1", { live: true, status: "in_progress" })];
    expect(conflictView(stateWith({ check: conflicts(["a"]), sessions: busy, resolving: "s1" }), "a")).toEqual({
      line: "1 file conflicts with main",
      files: ["a"],
      resolve: { state: "resolving", session_id: "s1" },
    });
    expect(conflictView(stateWith({ check: null, sessions: busy, resolving: "s1" }), "a")).toEqual({
      line: null,
      files: [],
      resolve: { state: "resolving", session_id: "s1" },
    });
  });
});

describe("resolve wording", () => {
  it("labels the button by what it does", () => {
    expect(resolveLabel({ state: "ready", session_id: "s1" }, true)).toBe("Merge conflicts");
    expect(resolveLabel({ state: "ready", session_id: "s1" }, false)).toBe("Resolve conflicts");
    expect(resolveLabel({ state: "blocked", reason: "x" }, true)).toBe("Merge conflicts");
    expect(resolveLabel({ state: "resolving", session_id: "s1" }, true)).toBe("Resolving conflicts…");
  });

  it("says why the Host refused, or nothing for codes it doesn't know", () => {
    const e = (code: string) => new ZwiimClientError(code, code, 409);
    expect(resolveRefusalText(e("turn_in_progress"))).toBe(
      "An agent is working here. Wait for it or stop it, then resolve conflicts.",
    );
    expect(resolveRefusalText(e("worktree_busy"))).toBe(
      "An agent is working here. Wait for it or stop it, then resolve conflicts.",
    );
    expect(resolveRefusalText(e("no_conflicts"))).toBe("Nothing to resolve: the branch merges cleanly now.");
    expect(resolveRefusalText(e("root_workspace"))).toBe("Conflicts are resolved in a worktree, not the main checkout.");
    expect(resolveRefusalText(e("not_found"))).toBe("That chat is gone. Open one here, then resolve conflicts.");
    expect(resolveRefusalText(e("network"))).toBeNull();
    expect(resolveRefusalText(new Error("x"))).toBeNull();
  });
});
