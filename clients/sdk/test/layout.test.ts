import { describe, expect, it } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import { groupOf, moveInLayout, placeOf, stepTarget, type LayoutRef, type LayoutTarget } from "../src/state/layout";
import { initialHostState, reduceHost, type HostState } from "../src/state/store";
import {
  buildSidebarTree,
  buildWorkspaceTree,
  openTarget,
  selectSidebarTree,
  selectWorkspaceTree,
} from "../src/state/workspaces";
import type { LayoutEntry, SessionSummary, WorkspaceInfo, WorkspaceLayout } from "../src/types/protocol";
import { FakeTransport, flush } from "./fakes";

const repo = (id: string, worktrees: string[] = []): LayoutEntry => ({ kind: "workspace", workspace_id: id, worktrees });
const group = (id: string, ...members: string[]): LayoutEntry => ({
  kind: "group",
  group_id: id,
  name: id,
  workspaces: members.map((workspace_id) => ({ workspace_id, worktrees: [] })),
});
const layoutOf = (...entries: LayoutEntry[]): WorkspaceLayout => ({ entries });

const w = (id: string): LayoutRef => ({ kind: "workspace", id });
const g = (id: string): LayoutRef => ({ kind: "group", id });
const t = (id: string): LayoutRef => ({ kind: "worktree", id });

/** A layout as a short string: `a`, `[g: a b]`, `a(x y)`. */
function shape(layout: WorkspaceLayout | null): string | null {
  if (!layout) return null;
  const member = (id: string, wts: string[]) => (wts.length ? `${id}(${wts.join(" ")})` : id);
  return layout.entries
    .map((e) =>
      e.kind === "workspace"
        ? member(e.workspace_id, e.worktrees)
        : `[${e.group_id}: ${e.workspaces.map((m) => member(m.workspace_id, m.worktrees)).join(" ")}]`.replace(": ]", ":]"),
    )
    .join(" ");
}

describe("moveInLayout", () => {
  const base = layoutOf(repo("a", ["x", "y", "z"]), group("g", "b", "c"), group("h"), repo("d"));

  const moves: [string, LayoutRef, LayoutTarget, string | null][] = [
    ["a repository into a group", w("a"), { at: "into", group_id: "g" }, "[g: b c a(x y z)] [h:] d"],
    ["a repository into an empty group", w("d"), { at: "into", group_id: "h" }, "a(x y z) [g: b c] [h: d]"],
    ["a repository out of a group", w("b"), { at: "end" }, "a(x y z) [g: c] [h:] d b"],
    ["a repository between groups", w("b"), { at: "into", group_id: "h" }, "a(x y z) [g: c] [h: b] d"],
    ["a repository before a member", w("d"), { at: "before", ref: w("c") }, "a(x y z) [g: b d c] [h:]"],
    ["a repository after a member", w("d"), { at: "after", ref: w("c") }, "a(x y z) [g: b c d] [h:]"],
    ["a member out next to a group", w("c"), { at: "before", ref: g("g") }, "a(x y z) c [g: b] [h:] d"],
    ["a member reordered", w("c"), { at: "before", ref: w("b") }, "a(x y z) [g: c b] [h:] d"],
    ["a repository reordered at the top", w("d"), { at: "before", ref: w("a") }, "d a(x y z) [g: b c] [h:]"],
    ["a group reordered", g("h"), { at: "before", ref: g("g") }, "a(x y z) [h:] [g: b c] d"],
    ["a group to the end", g("g"), { at: "end" }, "a(x y z) [h:] d [g: b c]"],
    ["a group next to a repository", g("g"), { at: "after", ref: w("d") }, "a(x y z) [h:] d [g: b c]"],
    ["a worktree reordered", t("z"), { at: "before", ref: t("x") }, "a(z x y) [g: b c] [h:] d"],
    ["a worktree after another", t("x"), { at: "after", ref: t("y") }, "a(y x z) [g: b c] [h:] d"],
    // Refused, so nothing is ever sent.
    ["a group into a group", g("g"), { at: "into", group_id: "h" }, null],
    ["a group among members", g("h"), { at: "before", ref: w("b") }, null],
    ["a worktree to the top level", t("x"), { at: "end" }, null],
    ["a worktree into a group", t("x"), { at: "into", group_id: "g" }, null],
    ["a worktree beside a repository", t("x"), { at: "before", ref: w("d") }, null],
    ["a repository among worktrees", w("d"), { at: "before", ref: t("x") }, null],
    ["onto itself", w("a"), { at: "before", ref: w("a") }, null],
    ["to the place it is in", w("a"), { at: "before", ref: g("g") }, null],
    ["into the group it is in, already last", w("c"), { at: "into", group_id: "g" }, null],
    ["to the end it is at", w("d"), { at: "end" }, null],
    ["a group to the end it is at", g("h"), { at: "after", ref: g("g") }, null],
    ["into a group that is gone", w("a"), { at: "into", group_id: "gone" }, null],
    ["a repository that is gone", w("gone"), { at: "end" }, null],
    ["next to a repository that is gone", w("a"), { at: "after", ref: w("gone") }, null],
  ];

  it.each(moves)("%s", (_, source, target, expected) => {
    expect(shape(moveInLayout(base, source, target))).toBe(expected);
  });

  it("leaves the layout it was given alone", () => {
    const before = JSON.stringify(base);
    moveInLayout(base, w("b"), { at: "into", group_id: "h" });
    expect(JSON.stringify(base)).toBe(before);
  });

  it("keeps a repository's worktrees and a group's name through a move", () => {
    const next = moveInLayout(base, w("a"), { at: "end" })!;
    expect(next.entries.at(-1)).toEqual(repo("a", ["x", "y", "z"]));
    expect(next.entries[0]).toMatchObject({ kind: "group", name: "g" });
  });

  it("moves a worktree inside a repository in a group", () => {
    const inside = layoutOf({
      kind: "group",
      group_id: "g",
      name: "g",
      workspaces: [{ workspace_id: "a", worktrees: ["x", "y"] }],
    });
    expect(shape(moveInLayout(inside, t("y"), { at: "before", ref: t("x") }))).toBe("[g: a(y x)]");
  });

  it("refuses a worktree moved under another repository", () => {
    const two = layoutOf(repo("a", ["x"]), repo("b", ["y"]));
    expect(moveInLayout(two, t("x"), { at: "before", ref: t("y") })).toBeNull();
  });

  it("keeps a worktree whose repository is gone out of every group", () => {
    const alone: LayoutRef = { ...w("o"), standalone: true };
    const orphan = layoutOf(repo("o"), group("g", "b"), repo("d"));
    expect(moveInLayout(orphan, alone, { at: "into", group_id: "g" })).toBeNull();
    expect(moveInLayout(orphan, alone, { at: "before", ref: w("b") })).toBeNull();
    expect(moveInLayout(orphan, alone, { at: "after", ref: w("b") })).toBeNull();
    expect(shape(moveInLayout(orphan, alone, { at: "after", ref: w("d") }))).toBe("[g: b] d o");
    expect(shape(moveInLayout(orphan, alone, { at: "after", ref: g("g") }))).toBe("[g: b] o d");
  });
});

describe("stepTarget", () => {
  const base = layoutOf(repo("a", ["x", "y"]), group("g", "b", "c"), repo("d"));

  it("hops a group whole, and stays in the container", () => {
    const down = stepTarget(base, w("a"), 1)!;
    expect(shape(moveInLayout(base, w("a"), down))).toBe("[g: b c] a(x y) d");
    const up = stepTarget(base, w("d"), -1)!;
    expect(shape(moveInLayout(base, w("d"), up))).toBe("a(x y) d [g: b c]");
  });

  it("moves a member inside its group and a worktree inside its repository", () => {
    expect(shape(moveInLayout(base, w("c"), stepTarget(base, w("c"), -1)!))).toBe("a(x y) [g: c b] d");
    expect(shape(moveInLayout(base, t("y"), stepTarget(base, t("y"), -1)!))).toBe("a(y x) [g: b c] d");
  });

  it("is null at the ends", () => {
    expect(stepTarget(base, w("a"), -1)).toBeNull();
    expect(stepTarget(base, w("d"), 1)).toBeNull();
    expect(stepTarget(base, w("b"), -1)).toBeNull();
    expect(stepTarget(base, w("c"), 1)).toBeNull();
    expect(stepTarget(base, t("x"), -1)).toBeNull();
    expect(stepTarget(base, t("y"), 1)).toBeNull();
    expect(stepTarget(base, w("gone"), 1)).toBeNull();
  });
});

describe("groupOf", () => {
  const base = layoutOf(group("g1", "a"), repo("b", ["x"]));

  it("names the group holding a repository", () => {
    expect(groupOf(base, w("a"))).toBe("g1");
  });

  it("is null at the top level or for something gone", () => {
    expect(groupOf(base, w("b"))).toBeNull();
    expect(groupOf(base, g("g1"))).toBeNull();
    expect(groupOf(base, w("gone"))).toBeNull();
  });
});

describe("placeOf", () => {
  const base = layoutOf(repo("a", ["x", "y"]), group("g", "b", "c"), repo("d"));

  it.each<[string, LayoutRef, LayoutTarget]>([
    ["a top level repository", w("a"), { at: "into", group_id: "g" }],
    ["the last repository", w("d"), { at: "before", ref: w("a") }],
    ["a member", w("b"), { at: "end" }],
    ["the last member", w("c"), { at: "end" }],
    ["a group", g("g"), { at: "end" }],
    ["a worktree", t("x"), { at: "after", ref: t("y") }],
    ["the last worktree", t("y"), { at: "before", ref: t("x") }],
  ])("undoes a move of %s", (_, source, target) => {
    const back = placeOf(base, source)!;
    const moved = moveInLayout(base, source, target)!;
    expect(shape(moveInLayout(moved, source, back))).toBe(shape(base));
  });

  it("is null for what is gone", () => {
    expect(placeOf(base, w("gone"))).toBeNull();
  });
});

const info = (id: string, extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  workspace_id: id,
  name: id,
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
  ...extra,
});
const worktree = (id: string, parent: string) =>
  info(id, { kind: "worktree", parent_workspace_id: parent, branch: `zwiim/${id}` });

const stateWith = (workspaces: WorkspaceInfo[], layout: WorkspaceLayout | null): HostState => ({
  ...initialHostState(),
  workspaces,
  layout,
});

const names = (s: Pick<HostState, "workspaces" | "sessionsByWorkspace" | "sessions" | "layout">) =>
  buildSidebarTree(s).map((e) =>
    e.kind === "group"
      ? `[${e.group.name}: ${e.repos.map((r) => r.workspace.workspace_id).join(" ")}]`
      : e.node.workspace.workspace_id,
  );

describe("buildSidebarTree", () => {
  const rows = [info("a"), info("b"), info("c"), worktree("x", "a"), worktree("y", "a")];

  it("stays flat without a layout", () => {
    expect(names(stateWith(rows, null))).toEqual(["a", "b", "c"]);
  });

  it("follows the layout, and the flat tree follows it too", () => {
    const s = stateWith(rows, layoutOf(repo("c"), group("backend", "b", "a")));
    expect(names(s)).toEqual(["c", "[backend: b a]"]);
    expect(buildWorkspaceTree(s).map((n) => n.workspace.workspace_id)).toEqual(["c", "b", "a"]);
  });

  it("orders worktrees by the layout", () => {
    const s = stateWith(rows, layoutOf(repo("a", ["y", "x"]), repo("b"), repo("c")));
    const tree = buildWorkspaceTree(s);
    expect(tree[0]!.worktrees.map((n) => n.workspace.workspace_id)).toEqual(["y", "x"]);
  });

  it("keeps an empty group", () => {
    const s = stateWith(rows, layoutOf(group("empty"), repo("a"), repo("b"), repo("c")));
    expect(names(s)).toEqual(["[empty: ]", "a", "b", "c"]);
  });

  it("puts a listed root the layout has not named yet last", () => {
    const s = stateWith([...rows, info("new")], layoutOf(repo("c"), repo("a"), repo("b")));
    expect(names(s)).toEqual(["c", "a", "b", "new"]);
  });

  it("puts a listed worktree the layout has not named yet last in its root", () => {
    const s = stateWith([...rows, worktree("new", "a")], layoutOf(repo("a", ["y", "x"]), repo("b"), repo("c")));
    const tree = buildWorkspaceTree(s);
    expect(tree[0]!.worktrees.map((n) => n.workspace.workspace_id)).toEqual(["y", "x", "new"]);
  });

  it("drops what the layout names and the list no longer has", () => {
    const s = stateWith([info("a")], layoutOf(group("g", "gone"), repo("a", ["alsoGone"]), repo("b")));
    expect(names(s)).toEqual(["[g: ]", "a"]);
  });

  it("rolls working agents up to the group, worktrees included", () => {
    const base = stateWith(rows, layoutOf(group("g", "a"), repo("b"), repo("c")));
    const running = (session_id: string, workspace_id: string) =>
      ({ session_id, workspace_id, live: true, status: "in_progress" }) as SessionSummary;
    const s = { ...base, sessionsByWorkspace: { a: [running("s1", "a")], x: [running("s2", "x")] } };
    expect(buildSidebarTree(s)[0]).toMatchObject({ kind: "group", working: 2 });
  });

  it("does not move where a repository opens", () => {
    const rows = [info("a"), worktree("old", "a"), worktree("recent", "a")];
    const s = stateWith(rows, layoutOf(repo("a", ["recent", "old"])));
    expect(openTarget(s, "a")).toEqual(openTarget(stateWith(rows, null), "a"));
  });
});

describe("the layout in the store", () => {
  const snapshot = (layout?: WorkspaceLayout) => ({
    type: "host_snapshot" as const,
    snapshot: { workspaces: [info("a")], sessions: [], terminals: [], ...(layout ? { layout } : {}) },
  });

  it("starts empty", () => {
    expect(initialHostState().layout).toBeNull();
  });

  it("comes from the snapshot", () => {
    const layout = layoutOf(repo("a"));
    expect(reduceHost(initialHostState(), snapshot(layout) as never).layout).toEqual(layout);
  });

  it("stays null on an older Host", () => {
    expect(reduceHost(initialHostState(), snapshot() as never).layout).toBeNull();
  });

  it("clears when a newer Host is replaced by an older one", () => {
    const s = reduceHost(initialHostState(), snapshot(layoutOf(repo("a"))) as never);
    expect(reduceHost(s, snapshot() as never).layout).toBeNull();
  });

  it("follows workspace.layout", () => {
    const layout = layoutOf(group("g", "a"));
    const s = reduceHost(initialHostState(), {
      type: "host_event",
      event: { type: "workspace.layout", payload: { layout } },
    } as never);
    expect(s.layout).toEqual(layout);
  });

  it("keeps the same trees until an input changes, then rebuilds", () => {
    const s = stateWith([info("a"), info("b")], layoutOf(repo("a"), repo("b")));
    const tree = selectWorkspaceTree(s);
    const sidebar = selectSidebarTree(s);
    expect(selectWorkspaceTree({ ...s })).toBe(tree);
    expect(selectSidebarTree({ ...s })).toBe(sidebar);
    const moved = { ...s, layout: layoutOf(repo("b"), repo("a")) };
    expect(selectWorkspaceTree(moved).map((n) => n.workspace.workspace_id)).toEqual(["b", "a"]);
    expect(selectSidebarTree(moved)).not.toBe(sidebar);
  });
});

describe("controller", () => {
  function setup() {
    const transport = new FakeTransport();
    const controller = new HostController(new HostClient({ transport, random: () => 0.5 }));
    return { transport, controller };
  }

  async function connected() {
    const t = setup();
    t.controller.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    return t;
  }

  function capture(t: ReturnType<typeof setup>, op: keyof FakeTransport["handlers"], result: unknown = {}) {
    const calls: unknown[] = [];
    t.transport.handlers[op] = (params: unknown) => {
      calls.push(params);
      return result;
    };
    return calls;
  }

  it("sends a layout without group names", async () => {
    const t = await connected();
    const calls = capture(t, "workspace.layout_set");
    await t.controller.setWorkspaceLayout(layoutOf(group("g", "a"), repo("b", ["x"])));
    await flush();
    expect(calls).toEqual([
      {
        entries: [
          { kind: "group", group_id: "g", workspaces: [{ workspace_id: "a", worktrees: [] }] },
          { kind: "workspace", workspace_id: "b", worktrees: ["x"] },
        ],
      },
    ]);
  });

  it("builds on an answered write until the feed's next layout", async () => {
    const t = await connected();
    capture(t, "workspace.layout_set");
    t.transport.feedEvent("workspace.layout", { layout: layoutOf(repo("a"), repo("b")) });
    await flush();

    await t.controller.setWorkspaceLayout(layoutOf(repo("b"), repo("a")));
    expect(shape(t.controller.store.getState().layout)).toBe("a b");
    expect(shape(t.controller.latestLayout())).toBe("b a");

    t.transport.feedEvent("workspace.layout", { layout: layoutOf(repo("b"), repo("a"), repo("c")) });
    await flush();
    expect(shape(t.controller.latestLayout())).toBe("b a c");
  });

  it("keeps an answered write over a layout that landed while it was on its way", async () => {
    const t = await connected();
    t.transport.feedEvent("workspace.layout", { layout: layoutOf(repo("a"), repo("b")) });
    await flush();
    t.transport.handlers["workspace.layout_set"] = () => {
      t.transport.feedEvent("workspace.layout", { layout: layoutOf(repo("a", ["x"]), repo("b")) });
      return {};
    };

    await t.controller.setWorkspaceLayout(layoutOf(repo("b"), repo("a")));
    expect(shape(t.controller.latestLayout())).toBe("b a");
  });

  it("builds on the Host's layout after a refused write", async () => {
    const t = await connected();
    t.transport.feedEvent("workspace.layout", { layout: layoutOf(repo("a"), repo("b")) });
    await flush();
    t.transport.handlers["workspace.layout_set"] = () => {
      throw new Error("refused");
    };

    await expect(t.controller.setWorkspaceLayout(layoutOf(repo("b"), repo("a")))).rejects.toThrow();
    expect(shape(t.controller.latestLayout())).toBe("a b");
  });

  it("creates, renames and deletes groups", async () => {
    const t = await connected();
    const made = capture(t, "workspace.group_create", { group_id: "g", name: "backend" });
    const renamed = capture(t, "workspace.group_rename", { group_id: "g", name: "api" });
    const deleted = capture(t, "workspace.group_delete");
    expect(await t.controller.createWorkspaceGroup("backend")).toEqual({ group_id: "g", name: "backend" });
    expect(await t.controller.renameWorkspaceGroup("g", "api")).toEqual({ group_id: "g", name: "api" });
    await t.controller.deleteWorkspaceGroup("g");
    expect(made).toEqual([{ name: "backend" }]);
    expect(renamed).toEqual([{ group_id: "g", name: "api" }]);
    expect(deleted).toEqual([{ group_id: "g" }]);
  });
});
