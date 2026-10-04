import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { busyDetails, dirtyDetails, ZwiimClientError } from "../src/client/errors";
import { HostClient } from "../src/client/host-client";
import { reduceHost, initialHostState, type HostState } from "../src/state/store";
import { buildWorkspaceTree, selectWorkspaceTree } from "../src/state/workspaces";
import { LocalTransport } from "../src/transport/local";
import type { SessionSummary, WorkspaceInfo } from "../src/types/protocol";
import { FakeTransport, flush } from "./fakes";

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
const wt = (id: string, extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  workspace_id: id,
  name: id,
  alias: null,
  kind: "worktree",
  parent_workspace_id: "root",
  branch: `zwiim/${id}`,
  base_branch: "main",
  base_commit: "3f9e".padEnd(40, "0"),
  removed: false,
  is_git_repo: true,
  display_path: null,
  ...extra,
});
const session = (id: string, workspace_id: string, extra: Partial<SessionSummary> = {}): SessionSummary => ({
  session_id: id,
  workspace_id,
  title: null,
  agent: "claude_code",
  status: "pending_review",
  live: false,
  created_at: "2026-09-24T00:00:00Z",
  updated_at: "2026-09-24T00:00:00Z",
  ...extra,
});
const hostError = (code: string, details?: Record<string, unknown>) =>
  new ZwiimClientError(code, code, 409, details);

function stateWith(workspaces: WorkspaceInfo[], sessions: Record<string, SessionSummary[]>): HostState {
  let s = reduceHost(initialHostState(), { type: "workspaces", workspaces });
  for (const [workspace_id, list] of Object.entries(sessions)) {
    s = reduceHost(s, { type: "sessions", workspace_id, sessions: list });
  }
  return s;
}

function setup() {
  const transport = new FakeTransport();
  const client = new HostClient({ transport, random: () => 0.5 });
  const controller = new HostController(client);
  return { transport, client, controller, store: controller.store };
}

async function goLive(t: ReturnType<typeof setup>) {
  t.client.start();
  await flush();
  t.transport.channel.open();
  await flush();
}

describe("workspace tree", () => {
  it("nests worktrees under their root with sessions and working counts", () => {
    const s = stateWith([root, wt("a"), wt("b")], {
      root: [session("s0", "root")],
      a: [
        session("s1", "a", { live: true, status: "in_progress" }),
        session("s2", "a", { live: true, status: "pending_review" }),
        session("s3", "a", { live: false, status: "in_progress" }),
      ],
    });
    const tree = buildWorkspaceTree(s);
    expect(tree.map((n) => n.workspace.workspace_id)).toEqual(["root"]);
    const [a, b] = tree[0]!.worktrees;
    expect(tree[0]!.sessions?.map((x) => x.session_id)).toEqual(["s0"]);
    expect([a!.workspace.workspace_id, a!.sessions?.length, a!.working]).toEqual(["a", 3, 1]);
    // Not loaded yet is not the same as empty.
    expect([b!.workspace.workspace_id, b!.sessions, b!.working]).toEqual(["b", null, 0]);
  });

  it("prefers an attached session's live status over the polled list", () => {
    let s = stateWith([root], { root: [session("s1", "root", { live: true, status: "pending_review" })] });
    expect(buildWorkspaceTree(s)[0]!.working).toBe(0);
    s = reduceHost(s, {
      type: "session_frame",
      session_id: "s1",
      frame: {
        type: "snapshot",
        epoch: "e",
        seq: 1,
        snapshot: {
          status: "prompting",
          live_message: null,
          tool_calls: [],
          pending_request: null,
          pending_user_message: null,
          last_error: null,
          usage: null,
          options: null,
          commands: [],
        },
      },
    });
    expect(buildWorkspaceTree(s)[0]!.working).toBe(1);
  });

  it("keeps a removed worktree and an orphan visible, and memoizes", () => {
    const s = stateWith([root, wt("gone", { removed: true, branch: null }), wt("orphan", { parent_workspace_id: "x" })], {});
    const tree = selectWorkspaceTree(s);
    expect(tree.map((n) => n.workspace.workspace_id)).toEqual(["root", "orphan"]);
    expect(tree[0]!.worktrees[0]!.workspace.removed).toBe(true);
    expect(selectWorkspaceTree({ ...s, connection: { ...s.connection } })).toBe(tree);
  });

  it("drops session lists of workspaces the host stopped listing", () => {
    let s = stateWith([root, wt("a")], { root: [], a: [session("s1", "a")] });
    s = reduceHost(s, { type: "workspaces", workspaces: [root] });
    expect(Object.keys(s.sessionsByWorkspace)).toEqual(["root"]);
  });
});

describe("worktree operations", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("creates a worktree from the host's answer, as a mutating request", async () => {
    const t = setup();
    t.transport.handlers["workspace.create_worktree"] = () => wt("a");
    const created = await t.controller.createWorktree("root");
    expect(created.branch).toBe("zwiim/a");
    const req = t.transport.requests.at(-1)!;
    expect([req.op, req.params, req.options]).toEqual([
      "workspace.create_worktree",
      { workspace_id: "root" },
      { mutating: true },
    ]);
    expect(t.store.getState().workspaces).toEqual([created]);
    expect(t.store.getState().sessionsByWorkspace.a).toEqual([]);

    await t.controller.createWorktree("root", { base: "origin/main" });
    expect(t.transport.requests.at(-1)!.params).toEqual({ workspace_id: "root", base: "origin/main" });
  });

  it("continues a worktree after its merged PR, as a mutating request, and shows the new branch at once", async () => {
    const t = setup();
    t.store.dispatch({ type: "workspaces", workspaces: [root, wt("a")] });
    const next = wt("a", { branch: "zwiim/b", base_branch: "origin/main" });
    t.transport.handlers["workspace.continue"] = () => next;
    expect(await t.controller.continueWorkspace("a", 71)).toEqual(next);
    const req = t.transport.requests.at(-1)!;
    expect([req.op, req.params, req.options]).toEqual([
      "workspace.continue",
      { workspace_id: "a", number: 71 },
      { mutating: true },
    ]);
    expect(t.store.getState().workspaces).toEqual([root, next]);
  });

  it("surfaces busy and dirty refusals with their details", async () => {
    const t = setup();
    t.transport.handlers["workspace.remove_worktree"] = (p) => {
      throw p.force
        ? hostError("workspace_busy", { live_sessions: 2, terminals: 1, session_ids: ["s1", "s2"] })
        : hostError("workspace_dirty", { changes: 3 });
    };
    const dirty = await t.controller.removeWorktree("a").catch((e: ZwiimClientError) => e);
    expect(dirtyDetails(dirty)).toEqual({ changes: 3 });
    expect(busyDetails(dirty)).toBeNull();
    const busy = await t.controller.removeWorktree("a", { force: true }).catch((e: ZwiimClientError) => e);
    expect(busyDetails(busy)).toEqual({ live_sessions: 2, terminals: 1, session_ids: ["s1", "s2"] });
    expect(t.transport.requests.map((r) => r.params)).toEqual([
      { workspace_id: "a" },
      { workspace_id: "a", force: true },
    ]);
    expect(t.transport.requests.every((r) => r.options?.mutating)).toBe(true);
    expect(t.transport.count("workspace.list")).toBe(0);
  });

  it("stops a session's agent once, as a mutation, and never retries an unknown outcome", async () => {
    const t = setup();
    t.transport.handlers["session.stop"] = () => ({});
    await t.controller.stopSession("s1");
    expect(t.transport.requests.at(-1)).toEqual({
      op: "session.stop",
      params: { session_id: "s1" },
      options: { mutating: true },
    });
    t.transport.handlers["session.stop"] = () => {
      throw new ZwiimClientError("unknown_outcome", "response lost");
    };
    await expect(t.controller.stopSession("s2")).rejects.toMatchObject({ code: "unknown_outcome" });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(t.transport.requests.filter((r) => r.op === "session.stop").map((r) => r.params)).toEqual([
      { session_id: "s1" },
      { session_id: "s2" },
    ]);
  });

  it("follows removal through the feed: a worktree with sessions stays, removed", async () => {
    const t = setup();
    t.transport.handlers["workspace.list"] = () => [root, wt("a"), wt("b")];
    t.transport.handlers["session.list"] = (p) => (p.workspace_id === "a" ? [session("s1", "a")] : []);
    t.transport.handlers["workspace.remove_worktree"] = (p) => {
      if (p.workspace_id === "a") {
        t.transport.feedEvent("workspace.changed", { workspace: wt("a", { removed: true, branch: null }) });
        return { workspace_id: "a", parent_workspace_id: "root", kept_sessions: 1 };
      }
      t.transport.feedEvent("workspace.removed", { workspace_id: "b" });
      return { workspace_id: "b", parent_workspace_id: "root", kept_sessions: 0 };
    };
    await goLive(t);
    const listsBefore = t.transport.requests.length;
    expect((await t.controller.removeWorktree("a", { force: true })).kept_sessions).toBe(1);
    await t.controller.removeWorktree("b");
    const nodes = selectWorkspaceTree(t.store.getState())[0]!.worktrees;
    expect(nodes.map((n) => [n.workspace.workspace_id, n.workspace.removed, n.sessions?.map((s) => s.session_id)])).toEqual([
      ["a", true, ["s1"]],
    ]);
    expect(t.transport.requests.length - listsBefore).toBe(2);
  });

  it("deletes a session once, following the feed", async () => {
    const t = setup();
    t.transport.handlers["workspace.list"] = () => [root, wt("a")];
    t.transport.handlers["session.list"] = (p) => (p.workspace_id === "a" ? [session("s1", "a"), session("s2", "a")] : []);
    t.transport.handlers["session.delete"] = (p) => {
      if (p.session_id === "s2") throw hostError("turn_in_progress");
      t.transport.feedEvent("session.removed", { session_id: "s1", workspace_id: "a" });
      return {};
    };
    await goLive(t);
    await t.controller.deleteSession("s1");
    const busy = await t.controller.deleteSession("s2").catch((e: ZwiimClientError) => e);
    expect((busy as ZwiimClientError).code).toBe("turn_in_progress");
    expect(t.store.getState().sessionsByWorkspace.a!.map((s) => s.session_id)).toEqual(["s2"]);
    const deletes = t.transport.requests.filter((r) => r.op === "session.delete");
    expect(deletes.map((r) => [r.params, r.options?.mutating])).toEqual([
      [{ session_id: "s1" }, true],
      [{ session_id: "s2" }, true],
    ]);
  });

  it("loads a diff, and one file's patch", async () => {
    const t = setup();
    t.transport.handlers["git.diff"] = (p) => ({
      base_commit: "abc",
      files: [{ path: "a.txt", change: "added", additions: 1, deletions: 0, binary: false, too_large: false, untracked: true }],
      truncated: false,
      ...(p.file ? { patch: "+x\n" } : {}),
    });
    expect((await t.controller.loadDiff("a")).files[0]!.untracked).toBe(true);
    expect((await t.controller.loadDiff("a", "a.txt")).patch).toBe("+x\n");
    expect(t.transport.requests.map((r) => [r.params, r.options?.mutating ?? false])).toEqual([
      [{ workspace_id: "a" }, false],
      [{ workspace_id: "a", file: "a.txt" }, false],
    ]);
  });

  it("rejects a send in a removed worktree without resending or re-listing", async () => {
    const t = setup();
    for (const op of ["session.send", "session.start"] as const) {
      t.transport.handlers[op] = () => {
        throw new ZwiimClientError("workspace_removed", "The workspace was removed", 410);
      };
    }
    await expect(
      t.controller.sendMessage({ session_id: "s1", blocks: [{ type: "text", text: "hi" }] }),
    ).rejects.toMatchObject({ code: "workspace_removed" });
    await expect(
      t.controller.startSession({ workspace_id: "a", agent: "claude_code", blocks: [] }),
    ).rejects.toMatchObject({ code: "workspace_removed" });
    await flush();
    expect(t.transport.count("session.send")).toBe(1);
    expect(t.transport.count("workspace.list")).toBe(0);
    expect(t.store.getState().sessions.s1).toBeUndefined();
  });

  it("rebuilds the whole tree from the host on reconnect", async () => {
    const t = setup();
    let working = true;
    t.transport.handlers["workspace.list"] = () => [root, wt("a"), wt("b")];
    t.transport.handlers["session.list"] = (p) =>
      p.workspace_id === "a" ? [session("s1", "a", { live: working, status: "in_progress" })] : [];
    await goLive(t);
    expect(selectWorkspaceTree(t.store.getState())[0]!.worktrees.map((n) => n.working)).toEqual([1, 0]);

    working = false;
    t.transport.channel.drop();
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    await flush();
    expect(t.transport.count("session.list")).toBe(0);
    expect(selectWorkspaceTree(t.store.getState())[0]!.worktrees.map((n) => n.working)).toEqual([0, 0]);
  });
});

describe("LocalTransport error details", () => {
  it("passes the host's details through, and only objects", async () => {
    const reply = (details: unknown) =>
      vi.fn(async () =>
        new Response(JSON.stringify({ error: { code: "workspace_busy", message: "busy", details } }), {
          status: 409,
        }),
      );
    let t = new LocalTransport({ url: "http://h", token: "t", fetch: reply({ live_sessions: 2, terminals: 1 }) });
    const fail = (tr: LocalTransport) =>
      tr.request("workspace.remove_worktree", { workspace_id: "a" }).then(
        () => null,
        (e: ZwiimClientError) => e,
      );
    const e = (await fail(t))!;
    // A host that names no sessions reads as none named.
    expect([e.code, e.status, busyDetails(e)]).toEqual([
      "workspace_busy",
      409,
      { live_sessions: 2, terminals: 1, session_ids: [] },
    ]);
    t = new LocalTransport({ url: "http://h", token: "t", fetch: reply([1]) });
    const e2 = (await fail(t))!;
    expect(e2.details).toBeUndefined();
  });
});
