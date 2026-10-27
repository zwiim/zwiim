import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import { initialHostState, reduceHost, type HostState } from "../src/state/store";
import { selectWorkspaceTree } from "../src/state/workspaces";
import type {
  CloneJob,
  ConflictCheck,
  HostFeedEvent,
  HostSnapshot,
  KeepAwakeStatus,
  PrState,
  SessionSummary,
  TerminalInfo,
  WorkspaceInfo,
} from "../src/types/protocol";
import { FakeTransport, flush } from "./fakes";

const ws = (id: string, extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  workspace_id: id,
  name: id,
  alias: null,
  kind: id === "root" ? "root" : "worktree",
  parent_workspace_id: id === "root" ? null : "root",
  branch: id === "root" ? "main" : `zwiim/${id}`,
  base_branch: id === "root" ? null : "main",
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
  ...extra,
});
const session = (id: string, workspace_id: string, created: number, extra: Partial<SessionSummary> = {}): SessionSummary => ({
  session_id: id,
  workspace_id,
  title: null,
  agent: "claude_code",
  status: "pending_review",
  live: false,
  created_at: `2026-09-24T00:00:0${created}Z`,
  updated_at: `2026-09-24T00:00:0${created}Z`,
  ...extra,
});
const term = (id: string, title = "zsh", workspace_id: string | null = null): TerminalInfo => ({ terminal_id: id, title, workspace_id });

const snap = (parts: Partial<HostSnapshot> = {}): HostSnapshot => ({
  epoch: "e1",
  seq: 0,
  workspaces: [],
  sessions: [],
  terminals: [],
  ...parts,
});

let seq = 0;
function apply(state: HostState, ...events: Omit<HostFeedEvent, "seq">[]): HostState {
  return events.reduce(
    (s, e) => reduceHost(s, { type: "host_event", event: { ...e, seq: ++seq } as HostFeedEvent }),
    state,
  );
}
const fromSnapshot = (parts: Partial<HostSnapshot>, base = initialHostState()) =>
  reduceHost(base, { type: "host_snapshot", snapshot: snap(parts) });
const pr = (number: number, extra: Partial<PrState> = {}): PrState => ({
  number,
  url: `https://github.com/o/r/pull/${number}`,
  title: "Add thing",
  state: "open",
  base: "main",
  head: "zwiim/a",
  head_sha: "abc123",
  merged_at: null,
  checks: { passed: 0, failed: 0, pending: 1, skipped: 0, items: [] },
  merge: { allowed: true, status: "unstable", reason: null, method: "squash", methods: ["squash"] },
  ...extra,
});
const ids = (list: { session_id: string }[] | undefined) => list?.map((s) => s.session_id);
function cloneJob(over: Partial<CloneJob> = {}): CloneJob {
  return {
    clone_id: "c_1",
    owner: "octo",
    name: "app",
    state: "cloning",
    phase: "receiving",
    percent: 40,
    workspace_id: null,
    worktree_id: null,
    error: null,
    ...over,
  };
}

describe("host feed reducers", () => {
  it("a snapshot replaces workspaces, sessions and terminals", () => {
    let s = fromSnapshot({
      workspaces: [ws("root"), ws("a")],
      sessions: [session("s2", "root", 2), session("s1", "root", 1), session("s3", "a", 3)],
      terminals: [term("t1"), term("t2")],
    });
    s = reduceHost(s, { type: "terminal", terminal_id: "t1", patch: { attached: true } });
    s = reduceHost(s, { type: "terminal", terminal_id: "t2", patch: { attached: true } });
    s = reduceHost(s, { type: "session_frame", session_id: "s1", frame: { type: "attaching" } });
    const diffBefore = s.diffRevision.root;

    s = fromSnapshot(
      {
        workspaces: [ws("root"), ws("b")],
        sessions: [session("s1", "root", 1, { title: "renamed" })],
        terminals: [term("t1", "bash"), term("t3")],
      },
      s,
    );
    expect(s.workspaces?.map((w) => w.workspace_id)).toEqual(["root", "b"]);
    // Every listed workspace has a session list, empty or not; unlisted ones are gone.
    expect(s.sessionsByWorkspace).toEqual({ root: [session("s1", "root", 1, { title: "renamed" })], b: [] });
    expect(s.terminals).toEqual({
      t1: { terminal_id: "t1", title: "bash", workspace_id: null, alive: true, attached: true },
      t2: { terminal_id: "t2", title: "zsh", workspace_id: null, alive: false, attached: true },
      t3: { terminal_id: "t3", title: "zsh", workspace_id: null, alive: true, attached: false },
    });
    // Open-session views are UI state, not part of the feed.
    expect(s.sessions.s1?.attach).toBe("attaching");
    // Anything missed while away counts as a possible diff change.
    expect(s.diffRevision.root).toBe(diffBefore! + 1);
    expect(s.diffRevision.b).toBe(1);
  });

  it("workspace.changed upserts; a new workspace starts with no sessions", () => {
    let s = fromSnapshot({ workspaces: [ws("root")], sessions: [session("s1", "root", 1)] });
    s = apply(
      s,
      { type: "workspace.changed", payload: { workspace: ws("root", { branch: "dev" }) } },
      { type: "workspace.changed", payload: { workspace: ws("a") } },
    );
    expect(s.workspaces?.map((w) => [w.workspace_id, w.branch])).toEqual([
      ["root", "dev"],
      ["a", "zwiim/a"],
    ]);
    expect(s.sessionsByWorkspace).toEqual({ root: [session("s1", "root", 1)], a: [] });
    s = apply(s, { type: "workspace.changed", payload: { workspace: ws("a", { removed: true, branch: null }) } });
    expect(s.workspaces?.[1]).toMatchObject({ removed: true, branch: null });
  });

  it("workspace.removed drops the workspace and its sessions", () => {
    let s = fromSnapshot({ workspaces: [ws("root"), ws("a")], sessions: [session("s1", "a", 1)] });
    s = apply(s, { type: "workspace.removed", payload: { workspace_id: "a" } });
    expect(s.workspaces?.map((w) => w.workspace_id)).toEqual(["root"]);
    expect(s.sessionsByWorkspace).toEqual({ root: [] });
  });

  it("pull requests come from the snapshot and follow workspace.pr_changed", () => {
    let s = fromSnapshot({
      workspaces: [ws("root"), ws("a"), ws("b")],
      prs: { a: { pr: pr(7), error: null } },
    });
    expect(s.prs.a?.pr?.number).toBe(7);

    const settled = pr(7, { checks: { passed: 2, failed: 0, pending: 0, skipped: 0, items: [] } });
    s = apply(
      s,
      { type: "workspace.pr_changed", payload: { workspace_id: "a", pr: settled, error: null } },
      {
        type: "workspace.pr_changed",
        payload: { workspace_id: "b", pr: null, error: { code: "gh_unauthenticated", message: "Sign in to gh" } },
      },
    );
    expect(s.prs.a?.pr?.checks.passed).toBe(2);
    expect(s.prs.b).toEqual({ pr: null, error: { code: "gh_unauthenticated", message: "Sign in to gh" } });

    const merged = pr(7, { state: "merged", merged_at: "2026-10-03T00:00:00Z" });
    s = apply(s, { type: "workspace.pr_changed", payload: { workspace_id: "a", pr: merged, error: null } });
    expect(s.prs.a?.pr?.state).toBe("merged");

    // No pull request and no error: the entry goes.
    s = apply(s, { type: "workspace.pr_changed", payload: { workspace_id: "a", pr: null, error: null } });
    expect("a" in s.prs).toBe(false);
    s = apply(s, { type: "workspace.removed", payload: { workspace_id: "b" } });
    expect(s.prs).toEqual({});

    // A new snapshot replaces them all; a Host without PRs has none.
    s = fromSnapshot({ workspaces: [ws("root")], prs: { root: { pr: pr(9), error: null } } }, s);
    expect(Object.keys(s.prs)).toEqual(["root"]);
    s = fromSnapshot({ workspaces: [ws("root")] }, s);
    expect(s.prs).toEqual({});
  });

  it("store_keeps_conflicts_from_snapshot_and_event", () => {
    const check = (state: ConflictCheck["state"], files: string[]): ConflictCheck =>
      state === "clean"
        ? { base: "origin/main", base_sha: "b1", head_sha: "h1", state }
        : state === "conflicts"
          ? { base: "origin/main", base_sha: "b1", head_sha: "h1", state, files }
          : { base: "origin/main", base_sha: "b1", head_sha: "h1", state, operation: "merge", files };
    let s = fromSnapshot({
      workspaces: [ws("root"), ws("a"), ws("b")],
      conflicts: { a: check("conflicts", ["src/x.ts"]) },
    });
    expect(s.conflicts.a).toMatchObject({ state: "conflicts", files: ["src/x.ts"] });

    s = apply(
      s,
      { type: "workspace.conflicts_changed", payload: { workspace_id: "a", conflicts: check("in_progress", ["src/x.ts"]) } },
      { type: "workspace.conflicts_changed", payload: { workspace_id: "b", conflicts: check("clean", []) } },
    );
    expect(s.conflicts.a).toMatchObject({ state: "in_progress", operation: "merge" });
    expect(s.conflicts.b?.state).toBe("clean");

    // Nothing to compare with: the entry goes.
    s = apply(s, { type: "workspace.conflicts_changed", payload: { workspace_id: "a", conflicts: null } });
    expect("a" in s.conflicts).toBe(false);
    s = apply(s, { type: "workspace.removed", payload: { workspace_id: "b" } });
    expect(s.conflicts).toEqual({});

    // A Host without conflict checks has none.
    s = fromSnapshot({ workspaces: [ws("root")], conflicts: { root: check("clean", []) } }, s);
    expect(Object.keys(s.conflicts)).toEqual(["root"]);
    s = fromSnapshot({ workspaces: [ws("root")] }, s);
    expect(s.conflicts).toEqual({});
  });

  it("workspace.diff_changed invalidates only that workspace's diff", () => {
    const s0 = fromSnapshot({ workspaces: [ws("root"), ws("a")] });
    const s = apply(s0, { type: "workspace.diff_changed", payload: { workspace_id: "a" } });
    expect(s.diffRevision).toEqual({ root: s0.diffRevision.root, a: s0.diffRevision.a! + 1 });
    // Invalidation only: the tree is untouched.
    expect([s.workspaces, s.sessionsByWorkspace]).toEqual([s0.workspaces, s0.sessionsByWorkspace]);
    expect(s.workspaces).toBe(s0.workspaces);
  });

  it("session.changed inserts newest first, updates in place, and moves between workspaces", () => {
    let s = fromSnapshot({
      workspaces: [ws("root"), ws("a")],
      sessions: [session("s3", "root", 3), session("s1", "root", 1), session("s9", "a", 9)],
    });
    s = apply(
      s,
      { type: "session.changed", payload: { session: session("s2", "root", 2) } },
      { type: "session.changed", payload: { session: session("s0", "root", 0) } },
      { type: "session.changed", payload: { session: session("s4", "root", 4, { live: true, status: "in_progress" }) } },
    );
    expect(ids(s.sessionsByWorkspace.root)).toEqual(["s4", "s3", "s2", "s1", "s0"]);
    expect(selectWorkspaceTree(s)[0]!.working).toBe(1);

    s = apply(s, { type: "session.changed", payload: { session: session("s4", "root", 4, { title: "t", status: "pending_review" }) } });
    expect(s.sessionsByWorkspace.root?.[0]).toMatchObject({ session_id: "s4", title: "t", status: "pending_review" });
    expect(selectWorkspaceTree(s)[0]!.working).toBe(0);

    // A legacy reparent: the worktree's session now belongs to the root.
    s = apply(s, { type: "session.changed", payload: { session: session("s9", "root", 9) } });
    expect(ids(s.sessionsByWorkspace.root)).toEqual(["s9", "s4", "s3", "s2", "s1", "s0"]);
    expect(s.sessionsByWorkspace.a).toEqual([]);
  });

  it("session.removed removes the session", () => {
    let s = fromSnapshot({ workspaces: [ws("root")], sessions: [session("s2", "root", 2), session("s1", "root", 1)] });
    s = apply(s, { type: "session.removed", payload: { session_id: "s2", workspace_id: "root" } });
    expect(ids(s.sessionsByWorkspace.root)).toEqual(["s1"]);
  });

  it("terminal.changed adds or retitles; terminal.removed ends or drops", () => {
    let s = fromSnapshot({ terminals: [term("t1")] });
    s = reduceHost(s, { type: "terminal", terminal_id: "t1", patch: { attached: true } });
    s = apply(
      s,
      { type: "terminal.changed", payload: { terminal: term("t2") } },
      { type: "terminal.changed", payload: { terminal: term("t1", "bash") } },
    );
    expect(s.terminals.t1).toEqual({ terminal_id: "t1", title: "bash", workspace_id: null, alive: true, attached: true });
    expect(s.terminals.t2).toEqual({ terminal_id: "t2", title: "zsh", workspace_id: null, alive: true, attached: false });
    s = apply(
      s,
      { type: "terminal.removed", payload: { terminal_id: "t1" } },
      { type: "terminal.removed", payload: { terminal_id: "t2" } },
    );
    // Still on screen: kept, marked ended. Not on screen: gone.
    expect(s.terminals).toEqual({ t1: { terminal_id: "t1", title: "bash", workspace_id: null, alive: false, attached: true } });
    expect(apply(s, { type: "terminal.removed", payload: { terminal_id: "nope" } })).toBe(s);
  });

  it("snapshot plus events matches a fresh snapshot of the final host state", () => {
    let s = fromSnapshot({
      workspaces: [ws("root"), ws("a")],
      sessions: [session("s1", "a", 1)],
      terminals: [term("t1")],
    });
    s = apply(
      s,
      { type: "workspace.changed", payload: { workspace: ws("b") } },
      { type: "session.changed", payload: { session: session("s2", "b", 2) } },
      { type: "session.changed", payload: { session: session("s1", "root", 1) } },
      { type: "workspace.removed", payload: { workspace_id: "a" } },
      { type: "workspace.changed", payload: { workspace: ws("root", { branch: "dev" }) } },
      { type: "terminal.removed", payload: { terminal_id: "t1" } },
      { type: "terminal.changed", payload: { terminal: term("t2") } },
    );
    const fresh = fromSnapshot({
      workspaces: [ws("root", { branch: "dev" }), ws("b")],
      sessions: [session("s1", "root", 1), session("s2", "b", 2)],
      terminals: [term("t2")],
    });
    expect([s.workspaces, s.sessionsByWorkspace, s.terminals]).toEqual([
      fresh.workspaces,
      fresh.sessionsByWorkspace,
      fresh.terminals,
    ]);
  });
});

describe("clones", () => {
  it("come from the snapshot and follow clone events", () => {
    let s = fromSnapshot({ clones: [cloneJob()] });
    expect(s.clones["c_1"]!.percent).toBe(40);
    s = apply(s, { type: "clone.changed", payload: { clone: cloneJob({ state: "done", workspace_id: "w1", worktree_id: "w2" }) } });
    expect(s.clones["c_1"]!.state).toBe("done");
    s = apply(s, { type: "clone.removed", payload: { clone_id: "c_1" } });
    expect(s.clones).toEqual({});
  });

  it("default to none for a Host without clones", () => {
    expect(fromSnapshot({}).clones).toEqual({});
  });
});

describe("keep awake", () => {
  const keepAwake: KeepAwakeStatus = {
    supported: true,
    mode: "always",
    on_battery: false,
    lid_closed: { support: "no_lid", enabled: false },
    active: true,
    blocked_by: null,
    warnings: [],
    power: { source: "ac", battery_percent: null },
  };

  it("keeps the Mac's keep-awake status from the snapshot and the feed", () => {
    expect(fromSnapshot({}).keepAwake).toBeNull();
    let s = fromSnapshot({ keep_awake: keepAwake });
    expect(s.keepAwake).toEqual(keepAwake);
    const later = { ...keepAwake, active: false, blocked_by: "on_battery" as const };
    s = apply(s, { type: "host.keep_awake_changed", payload: later });
    expect(s.keepAwake).toEqual(later);
  });
});

describe("controller on the host feed", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  async function live() {
    const transport = new FakeTransport();
    transport.handlers["workspace.list"] = () => [ws("root")];
    const client = new HostClient({ transport, random: () => 0.5 });
    const controller = new HostController(client);
    client.start();
    await flush();
    transport.channel.open();
    await flush();
    return { transport, controller, store: controller.store };
  }

  it("applies another client's changes with no list calls", async () => {
    const t = await live();
    t.transport.feedEvent("workspace.changed", { workspace: ws("a") });
    t.transport.feedEvent("session.changed", { session: session("s1", "a", 1, { live: true, status: "in_progress" }) });
    t.transport.feedEvent("terminal.changed", { terminal: term("t1") });
    const tree = selectWorkspaceTree(t.store.getState());
    expect(tree[0]!.worktrees.map((n) => [n.workspace.workspace_id, ids(n.sessions ?? []), n.working])).toEqual([
      ["a", ["s1"], 1],
    ]);
    expect(Object.keys(t.store.getState().terminals)).toEqual(["t1"]);
    t.transport.feedEvent("terminal.removed", { terminal_id: "t1" });
    expect(t.store.getState().terminals).toEqual({});
    expect(t.transport.requests.map((r) => r.op)).toEqual(["host.hello", "agent.list"]);
  });

  it("invalidates a diff on workspace.diff_changed", async () => {
    const t = await live();
    const before = t.store.getState().diffRevision.root!;
    t.transport.feedEvent("workspace.diff_changed", { workspace_id: "root" });
    expect(t.store.getState().diffRevision.root).toBe(before + 1);
    expect(t.transport.count("git.diff")).toBe(0);
  });

  it("mutation responses and feed events agree in either order", async () => {
    const t = await live();
    t.transport.handlers["workspace.create_worktree"] = () => {
      // The feed can outrun the response, including another client's session.
      t.transport.feedEvent("workspace.changed", { workspace: ws("a") });
      t.transport.feedEvent("session.changed", { session: session("s1", "a", 1) });
      return ws("a");
    };
    await t.controller.createWorktree("root");
    expect(ids(t.store.getState().sessionsByWorkspace.a)).toEqual(["s1"]);

    t.transport.handlers["terminal.spawn"] = () => ({ terminal_id: "t1" });
    await t.controller.spawnTerminal("root");
    t.transport.feedEvent("terminal.changed", { terminal: term("t1") });
    expect(t.store.getState().terminals.t1).toMatchObject({ title: "zsh", alive: true });
    t.transport.handlers["session.start"] = () => ({ session_id: "s2" });
    await t.controller.startSession({ workspace_id: "root", agent: "claude_code", blocks: [] });
    for (const op of ["workspace.list", "session.list", "terminal.list"] as const) {
      expect(t.transport.count(op)).toBe(0);
    }
  });
});
