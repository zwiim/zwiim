import { act, cleanup, createEvent, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  type LayoutEntry,
  type WorkspaceInfo,
  type WorkspaceLayout,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { MOVE_SETTLE_MS } from "@/lib/layout-moves";
import { rememberSidebarFolds, sidebarFolds } from "@/lib/prefs";
import { Sidebar } from "@/panels/Sidebar";
import { FakeTransport, hostHello, tick } from "./fakes";

const repo = (id: string, extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
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
const worktree = (id: string, parent: string, extra: Partial<WorkspaceInfo> = {}) =>
  repo(id, { kind: "worktree", parent_workspace_id: parent, branch: id, ...extra });
const member = (id: string, worktrees: string[] = []) => ({ workspace_id: id, worktrees });
const flat = (id: string, worktrees: string[] = []): LayoutEntry => ({ kind: "workspace", ...member(id, worktrees) });
const group = (id: string, name: string, ...workspaces: string[]): LayoutEntry => ({
  kind: "group",
  group_id: id,
  name,
  workspaces: workspaces.map((w) => member(w)),
});

function host(layout: WorkspaceLayout | undefined, workspaces: WorkspaceInfo[] = [repo("api"), repo("web"), repo("cli")]) {
  const t = new FakeTransport();
  t.handlers["workspace.list"] = () => workspaces;
  t.handlers["workspace.layout_set"] = () => ({});
  t.layout = layout;
  return t;
}

async function mount(t: FakeTransport) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <Sidebar
        workspaceId={null}
        onSelectWorkspace={vi.fn()}
        onSessionStarted={vi.fn()}
        pickFolder={vi.fn(async () => null)}
        remote={false}
      />
    </HostProvider>,
  );
  await tick();
}

const rows = () =>
  [...screen.getByTestId("workspace-tree").querySelectorAll<HTMLElement>("[data-testid]")].map((e) =>
    e.dataset.testid!.replace("workspace-", ""),
  );
const repoRow = (id: string) => screen.getByTestId(`workspace-${id}`).firstElementChild as HTMLElement;
const groupRow = (id: string) => screen.getByTestId(`group-${id}`).firstElementChild as HTMLElement;
const sent = (t: FakeTransport) =>
  t.calls.filter((c) => c.op === "workspace.layout_set").map((c) => (c.params as WorkspaceLayout).entries);
const offline = async (t: FakeTransport) => {
  await act(async () => t.handlersForEvents!.onClose({ code: 1006, reason: "" }));
  await tick();
};

const dataTransfer = () => ({ setData: vi.fn(), effectAllowed: "", dropEffect: "", types: [] });

/** Rows measure 100 tall from 0, so `y` is a percent of the row's height. */
function fireAt(type: "dragOver" | "drop", el: Element, y: number) {
  const event = createEvent[type](el, { dataTransfer: dataTransfer() });
  Object.defineProperty(event, "clientY", { value: y });
  fireEvent(el, event);
  return event;
}
function drag(el: Element) {
  const data = dataTransfer();
  fireEvent.dragStart(el, { dataTransfer: data });
  return data;
}
async function dragTo(from: Element, to: Element, y: number) {
  drag(from);
  fireAt("dragOver", to, y);
  fireAt("drop", to, y);
  fireEvent.dragEnd(from);
  await tick();
}

const menu = (id: string, name: string) => {
  fireEvent.click(within(screen.getByTestId(`workspace-${id}`)).getAllByRole("button", { name: `${name} actions` })[0]!);
  return within(screen.getByRole("menu", { name: `${name} actions` }));
};
const pick = async (id: string, item: string) => {
  fireEvent.click(menu(id, id).getByRole("menuitem", { name: item }));
  await tick();
};
const item = (m: ReturnType<typeof menu>, name: string) => m.getByRole("menuitem", { name }) as HTMLButtonElement;

beforeEach(() => {
  localStorage.clear();
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({ top: 0, height: 100 } as DOMRect);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("dragging in the sidebar", () => {
  it("starts on the row alone, carrying data WebKit needs", async () => {
    await mount(host({ entries: [flat("api", ["wt"])] }, [repo("api"), worktree("wt", "api")]));
    const row = repoRow("api");
    expect(row.getAttribute("draggable")).toBe("true");
    expect(row.contains(screen.getByTestId("workspace-wt"))).toBe(false);
    const data = drag(row);
    expect(data.setData).toHaveBeenCalledWith("text/plain", "api");
    expect(data.effectAllowed).toBe("move");
  });

  it("drops into the middle of a group, outlined, with no brand fill", async () => {
    const t = host({ entries: [group("g1", "backend", "api"), flat("web"), flat("cli")] });
    await mount(t);
    drag(repoRow("cli"));
    expect(fireAt("dragOver", groupRow("g1"), 50).defaultPrevented).toBe(true);
    expect(groupRow("g1").className).toContain("ring-ring");
    expect(groupRow("g1").className).not.toContain("brand");
    fireAt("drop", groupRow("g1"), 50);
    await tick();
    expect(sent(t)).toEqual([[
      { kind: "group", group_id: "g1", workspaces: [member("api"), member("cli")] },
      flat("web"),
    ]]);
    expect(rows()).toEqual(["group-g1", "api", "cli", "web"]);
    expect(screen.getByRole("status").textContent).toContain("Moved cli to backend");
  });

  it("inserts above in the top quarter and below in the bottom quarter of a group", async () => {
    const t = host({ entries: [flat("web"), group("g1", "backend"), flat("cli")] }, [repo("web"), repo("cli")]);
    await mount(t);
    drag(repoRow("cli"));
    fireAt("dragOver", groupRow("g1"), 10);
    expect(screen.getByTestId("group-g1").querySelector('[data-drop="before"]')).toBeTruthy();
    fireAt("drop", groupRow("g1"), 10);
    await tick();
    expect(rows()).toEqual(["web", "cli", "group-g1"]);
    await t.feedEvent("workspace.layout", { layout: { entries: [flat("web"), flat("cli"), group("g1", "backend")] } });

    await dragTo(repoRow("web"), groupRow("g1"), 90);
    expect(rows()).toEqual(["cli", "group-g1", "web"]);
  });

  it("reads the bottom quarter of an open group as the top of its members", async () => {
    const t = host({ entries: [group("g1", "backend", "api"), flat("cli")] }, [repo("api"), repo("cli")]);
    await mount(t);
    drag(repoRow("cli"));
    fireAt("dragOver", groupRow("g1"), 90);
    expect(screen.getByTestId("workspace-api").querySelector('[data-drop="before"]')).toBeTruthy();
    fireAt("drop", groupRow("g1"), 90);
    await tick();
    expect(rows()).toEqual(["group-g1", "cli", "api"]);
  });

  it("drops out of a group onto the zone under the list", async () => {
    const t = host({ entries: [group("g1", "backend", "api"), flat("web")] }, [repo("api"), repo("web")]);
    await mount(t);
    const end = document.querySelector("[data-layout-end]")!;
    drag(repoRow("api"));
    fireAt("dragOver", end, 0);
    expect(end.querySelector("[data-drop]")).toBeTruthy();
    fireAt("drop", end, 0);
    await tick();
    expect(rows()).toEqual(["group-g1", "web", "api"]);
    expect(screen.getByRole("status").textContent).toContain("Moved api out of backend");
  });

  it("reorders groups at the top level, passing over their members", async () => {
    const t = host({ entries: [group("g1", "backend", "api"), group("g2", "web", "web")] }, [repo("api"), repo("web")]);
    await mount(t);
    await dragTo(groupRow("g2"), repoRow("api"), 10);
    expect(rows()).toEqual(["group-g2", "web", "group-g1", "api"]);
  });

  it("reorders worktrees in their repository and nowhere else", async () => {
    const t = host(
      { entries: [flat("api", ["w1", "w2"]), flat("web", ["w3"])] },
      [repo("api"), worktree("w1", "api"), worktree("w2", "api"), repo("web"), worktree("w3", "web")],
    );
    await mount(t);
    drag(repoRow("w2"));
    expect(fireAt("dragOver", repoRow("w3"), 10).defaultPrevented).toBe(false);
    expect(fireAt("dragOver", repoRow("web"), 10).defaultPrevented).toBe(false);
    expect(document.querySelector("[data-drop]")).toBeNull();
    fireAt("drop", repoRow("w3"), 10);
    await tick();
    expect(sent(t)).toEqual([]);

    await dragTo(repoRow("w2"), repoRow("w1"), 10);
    expect(rows().filter((r) => r.startsWith("w") && r !== "web")).toEqual(["w2", "w1", "w3"]);
  });

  it("works out the move at the drop, on the layout another device just changed", async () => {
    const t = host({ entries: [flat("api"), flat("web"), flat("cli")] });
    await mount(t);
    drag(repoRow("api"));
    const elsewhere = { entries: [flat("cli"), flat("web"), flat("api")] };
    await t.feedEvent("workspace.layout", { layout: elsewhere });
    fireAt("drop", repoRow("web"), 10);
    await tick();
    expect(sent(t)).toEqual([[flat("cli"), flat("api"), flat("web")]]);
  });

  it("drops on what is left when the group went away mid-drag", async () => {
    const t = host({ entries: [group("g1", "backend", "web"), flat("api")] }, [repo("api"), repo("web")]);
    await mount(t);
    drag(repoRow("api"));
    await t.feedEvent("workspace.layout", { layout: { entries: [flat("web"), flat("api")] } });
    expect(screen.queryByTestId("group-g1")).toBeNull();
    fireAt("drop", repoRow("web"), 10);
    await tick();
    expect(sent(t)).toEqual([[flat("api"), flat("web")]]);
  });

  it("writes nothing when the row dragged went away", async () => {
    const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
    await mount(t);
    drag(repoRow("api"));
    await t.feedEvent("workspace.layout", { layout: { entries: [flat("web")] } });
    fireAt("drop", repoRow("web"), 10);
    await tick();
    expect(sent(t)).toEqual([]);
  });

  it("unfolds a folded group on a drop", async () => {
    rememberSidebarFolds({ ...sidebarFolds(), groups: ["g1"] });
    const t = host({ entries: [group("g1", "backend"), flat("api")] }, [repo("api")]);
    await mount(t);
    expect(rows()).toEqual(["group-g1", "api"]);
    await dragTo(repoRow("api"), groupRow("g1"), 50);
    expect(sidebarFolds().groups).toEqual([]);
    expect(rows()).toEqual(["group-g1", "api"]);
    expect(screen.getByTestId("group-g1").contains(screen.getByTestId("workspace-api"))).toBe(true);
  });

  it("leaves archived worktrees and other drags alone", async () => {
    const t = host(
      { entries: [flat("api", ["old"]), flat("web")] },
      [repo("api"), worktree("old", "api", { removed: true }), repo("web")],
    );
    await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "1 archived" }));
    expect(repoRow("old").getAttribute("draggable")).toBeNull();
    // A file dragged in from the OS: no row started it.
    expect(fireAt("dragOver", repoRow("web"), 10).defaultPrevented).toBe(false);
  });

  it("cancels on Esc, which the browser ends with dragend and no drop", async () => {
    const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
    await mount(t);
    const row = repoRow("api");
    drag(row);
    fireAt("dragOver", repoRow("web"), 90);
    expect(document.querySelector("[data-drop]")).toBeTruthy();
    fireEvent.dragEnd(row);
    await tick();
    expect(document.querySelector("[data-drop]")).toBeNull();
    expect(sent(t)).toEqual([]);
  });

  it("cancels when the Host goes offline, and rows stop dragging", async () => {
    const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
    await mount(t);
    drag(repoRow("api"));
    fireAt("dragOver", repoRow("web"), 90);
    expect(document.querySelector("[data-drop]")).toBeTruthy();
    await offline(t);
    expect(document.querySelector("[data-drop]")).toBeNull();
    expect(repoRow("api").getAttribute("draggable")).toBe("false");
    expect(fireAt("dragOver", repoRow("web"), 90).defaultPrevented).toBe(false);
    fireAt("drop", repoRow("web"), 90);
    await tick();
    expect(sent(t)).toEqual([]);
  });

  it("holds rows still while a move is on its way", async () => {
    const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
    await mount(t);
    await dragTo(repoRow("web"), repoRow("api"), 10);
    expect(rows()).toEqual(["web", "api"]);
    expect(repoRow("api").getAttribute("draggable")).toBe("false");
    await t.feedEvent("workspace.layout", { layout: { entries: [flat("web"), flat("api")] } });
    expect(repoRow("api").getAttribute("draggable")).toBe("true");
  });

  it("shows the Host's layout again when none arrives in time", async () => {
    const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
    await mount(t);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    await dragTo(repoRow("web"), repoRow("api"), 10);
    expect(rows()).toEqual(["web", "api"]);
    await act(async () => void vi.advanceTimersByTime(MOVE_SETTLE_MS));
    expect(rows()).toEqual(["api", "web"]);
  });

  describe("a write on its way", () => {
    const three = () => host({ entries: [flat("api"), flat("web"), flat("cli")] });
    const slowly = (t: FakeTransport) => {
      let answer = () => {};
      t.handlers["workspace.layout_set"] = () => new Promise((resolve) => (answer = () => resolve({})));
      return async () => {
        answer();
        await tick();
      };
    };

    it("stays through a layout that is not its own", async () => {
      const t = three();
      const answer = slowly(t);
      await mount(t);
      await dragTo(repoRow("cli"), repoRow("api"), 10);
      await t.feedEvent("workspace.layout", { layout: { entries: [flat("api", ["x"]), flat("web"), flat("cli")] } });
      expect(rows()).toEqual(["cli", "api", "web"]);
      expect(repoRow("web").getAttribute("draggable")).toBe("false");
      await pick("web", "Move up");
      expect(sent(t)).toHaveLength(1);
      await answer();
    });

    it("stays however long the Host takes to answer", async () => {
      const t = three();
      const answer = slowly(t);
      await mount(t);
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      await dragTo(repoRow("cli"), repoRow("api"), 10);
      await act(async () => void vi.advanceTimersByTime(MOVE_SETTLE_MS));
      expect(rows()).toEqual(["cli", "api", "web"]);
      expect(repoRow("web").getAttribute("draggable")).toBe("false");
      await answer();
    });

    it("is kept by the next move when the feed has not brought it", async () => {
      const t = three();
      await mount(t);
      vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
      await dragTo(repoRow("cli"), repoRow("api"), 10);
      await act(async () => void vi.advanceTimersByTime(MOVE_SETTLE_MS));
      await dragTo(repoRow("web"), repoRow("api"), 10);
      expect(sent(t)[1]).toEqual([flat("cli"), flat("web"), flat("api")]);
    });

    it("frees the rows at once when its layout beats the answer", async () => {
      const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
      t.handlers["workspace.layout_set"] = async () => {
        await t.feedEvent("workspace.layout", { layout: { entries: [flat("web"), flat("api")] } });
        return {};
      };
      await mount(t);
      await dragTo(repoRow("web"), repoRow("api"), 10);
      expect(rows()).toEqual(["web", "api"]);
      expect(repoRow("api").getAttribute("draggable")).toBe("true");
    });
  });

  it("reverts a refused move and says why", async () => {
    const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
    t.handlers["workspace.layout_set"] = () => {
      throw new Error("layout refused");
    };
    await mount(t);
    await dragTo(repoRow("web"), repoRow("api"), 10);
    expect(rows()).toEqual(["api", "web"]);
    expect(screen.getByRole("alert").textContent).toContain("layout refused");
  });

  it("is off on a Host from before groups", async () => {
    const t = host(undefined);
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "workspace.layout") });
    await mount(t);
    expect(repoRow("api").getAttribute("draggable")).toBe("false");
    expect(document.querySelector("[data-layout-end]")).toBeNull();
  });
});

describe("Undo", () => {
  it("puts a move back on the latest layout, keeping later work", async () => {
    const t = host({ entries: [group("g1", "backend", "web"), flat("api"), flat("cli")] });
    await mount(t);
    await dragTo(repoRow("api"), groupRow("g1"), 50);
    await t.feedEvent("workspace.layout", { layout: { entries: [group("g1", "backend", "web", "api"), flat("cli")] } });
    // Another device adds a group after the move.
    await t.feedEvent("workspace.layout", {
      layout: { entries: [group("g1", "backend", "web", "api"), flat("cli"), group("g2", "tools")] },
    });
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await tick();
    expect(sent(t)[1]).toEqual([
      { kind: "group", group_id: "g1", workspaces: [member("web")] },
      flat("api"),
      flat("cli"),
      { kind: "group", group_id: "g2", workspaces: [] },
    ]);
    expect(screen.queryByRole("button", { name: "Undo" })).toBeNull();
    expect(document.activeElement).toBe(within(screen.getByTestId("workspace-api")).getAllByRole("button")[0]);
  });

  it("says the group is gone when the move can't go back", async () => {
    const t = host({ entries: [group("g1", "backend", "api"), flat("web")] }, [repo("api"), repo("web")]);
    await mount(t);
    await dragTo(repoRow("api"), repoRow("web"), 90);
    await t.feedEvent("workspace.layout", { layout: { entries: [flat("web"), flat("api")] } });
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await tick();
    expect(sent(t)).toHaveLength(1);
    expect(screen.getByRole("status").textContent).toContain("backend was deleted on another device. api stayed put.");
  });

  describe("of New group...", () => {
    const moveToNew = async (t: FakeTransport) => {
      t.handlers["workspace.group_create"] = (p) => ({ group_id: "g9", name: p.name });
      t.handlers["workspace.group_delete"] = () => ({});
      await mount(t);
      await pick("api", "New group...");
      const input = screen.getByRole("textbox", { name: "New group name" });
      fireEvent.change(input, { target: { value: "backend" } });
      fireEvent.keyDown(input, { key: "Enter" });
      await tick();
    };

    it("takes away the group made for the move", async () => {
      const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
      await moveToNew(t);
      await t.feedEvent("workspace.layout", { layout: { entries: [flat("web"), group("g9", "backend", "api")] } });
      fireEvent.click(screen.getByRole("button", { name: "Undo" }));
      await tick();
      expect(sent(t)[1]).toEqual([flat("api"), flat("web"), { kind: "group", group_id: "g9", workspaces: [] }]);
      expect(t.calls.filter((c) => c.op !== "host.hello").at(-1)).toEqual({
        op: "workspace.group_delete",
        params: { group_id: "g9" },
      });
    });

    it("keeps the group when something else moved in", async () => {
      const t = host({ entries: [flat("api"), flat("web"), flat("cli")] });
      await moveToNew(t);
      await t.feedEvent("workspace.layout", { layout: { entries: [flat("web"), group("g9", "backend", "api", "cli")] } });
      fireEvent.click(screen.getByRole("button", { name: "Undo" }));
      await tick();
      expect(sent(t)).toHaveLength(2);
      expect(t.calls.some((c) => c.op === "workspace.group_delete")).toBe(false);
    });
  });
});

describe("moving from a menu", () => {
  it("moves a repository to a group and back to no group, focus following", async () => {
    const t = host({ entries: [group("g1", "backend", "web"), flat("api")] }, [repo("api"), repo("web")]);
    await mount(t);
    const m = menu("api", "api");
    expect(item(m, "No group").disabled).toBe(true);
    fireEvent.click(item(m, "backend"));
    await tick();
    expect(sent(t)[0]).toEqual([{ kind: "group", group_id: "g1", workspaces: [member("web"), member("api")] }]);
    expect(document.activeElement?.getAttribute("data-row")).toBe("api");
    await t.feedEvent("workspace.layout", { layout: { entries: [group("g1", "backend", "web", "api")] } });

    const again = menu("api", "api");
    expect(item(again, "backend").disabled).toBe(true);
    fireEvent.click(item(again, "No group"));
    await tick();
    expect(sent(t)[1]).toEqual([{ kind: "group", group_id: "g1", workspaces: [member("web")] }, flat("api")]);
  });

  it("offers no group to a worktree whose repository is gone", async () => {
    const alone = repo("wt", { kind: "worktree" });
    const t = host({ entries: [group("g1", "backend", "web"), flat("wt")] }, [repo("web"), alone]);
    await mount(t);
    const m = menu("wt", "wt");
    expect(m.queryByRole("group", { name: "Move to group" })).toBeNull();
    expect(item(m, "Move up").disabled).toBe(false);
    fireEvent.click(document.body);

    drag(repoRow("wt"));
    expect(fireAt("dragOver", groupRow("g1"), 50).defaultPrevented).toBe(false);
    expect(fireAt("dragOver", repoRow("web"), 10).defaultPrevented).toBe(false);
  });

  it("offers moves on a plain folder, which has no other actions", async () => {
    const t = host({ entries: [group("g1", "backend"), flat("notes")] }, [repo("notes", { is_git_repo: false })]);
    await mount(t);
    await pick("notes", "backend");
    expect(sent(t)).toHaveLength(1);
  });

  it("makes a new group, then moves into it", async () => {
    const t = host({ entries: [flat("api")] }, [repo("api")]);
    t.handlers["workspace.group_create"] = (p) => ({ group_id: "g9", name: p.name });
    await mount(t);
    await pick("api", "New group...");
    const input = screen.getByRole("textbox", { name: "New group name" });
    fireEvent.change(input, { target: { value: "backend" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await tick();
    expect(sent(t)).toEqual([[{ kind: "group", group_id: "g9", workspaces: [member("api")] }]]);
    expect(screen.getByTestId("group-g9").contains(screen.getByTestId("workspace-api"))).toBe(true);
    expect(screen.getByRole("status").textContent).toContain("Moved api to backend");
  });

  it("keeps a new group when the move into it fails", async () => {
    const t = host({ entries: [flat("api")] }, [repo("api")]);
    t.handlers["workspace.group_create"] = (p) => ({ group_id: "g9", name: p.name });
    t.handlers["workspace.layout_set"] = () => {
      throw new Error("no move");
    };
    await mount(t);
    await pick("api", "New group...");
    const input = screen.getByRole("textbox", { name: "New group name" });
    fireEvent.change(input, { target: { value: "backend" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await tick();
    expect(t.calls.some((c) => c.op === "workspace.group_delete")).toBe(false);
    expect(screen.getByRole("alert").textContent).toContain("no move");
  });

  describe("New group... around another move", () => {
    const typeName = async (name: string) => {
      await pick("api", "New group...");
      const input = screen.getByRole("textbox", { name: "New group name" });
      fireEvent.change(input, { target: { value: name } });
      return input;
    };
    const later = (t: FakeTransport, op: keyof FakeTransport["handlers"], answer: unknown) => {
      let go = () => {};
      t.handlers[op] = () => new Promise((resolve) => (go = () => resolve(answer)));
      return async () => {
        go();
        await tick();
      };
    };

    it("makes no group while a move is on its way, and keeps the name", async () => {
      const t = host({ entries: [flat("api"), flat("web"), flat("cli")] });
      const answer = later(t, "workspace.layout_set", {});
      await mount(t);
      const input = await typeName("backend");
      await dragTo(repoRow("cli"), repoRow("web"), 10);
      fireEvent.keyDown(input, { key: "Enter" });
      await tick();
      expect(t.calls.some((c) => c.op === "workspace.group_create")).toBe(false);
      expect((screen.getByRole("textbox", { name: "New group name" }) as HTMLInputElement).value).toBe("backend");
      expect(screen.getByText("Wait for the move on its way, then try again.")).toBeTruthy();
      await answer();
    });

    it("says so when a move started during the create leaves the group empty", async () => {
      const t = host({ entries: [flat("api"), flat("web"), flat("cli")] });
      const made = later(t, "workspace.group_create", { group_id: "g9", name: "backend" });
      const answer = later(t, "workspace.layout_set", {});
      await mount(t);
      const input = await typeName("backend");
      fireEvent.keyDown(input, { key: "Enter" });
      await tick();
      await dragTo(repoRow("cli"), repoRow("web"), 10);
      await made();
      expect(sent(t)).toHaveLength(1);
      expect(screen.getByRole("status").textContent).toContain("api stayed put. backend is empty.");
      await answer();
    });
  });

  it("steps within the container, hops a group whole, and disables at the ends", async () => {
    const t = host({ entries: [flat("api"), group("g1", "backend", "web"), flat("cli")] });
    await mount(t);
    const first = menu("api", "api");
    expect(item(first, "Move up").disabled).toBe(true);
    fireEvent.click(item(first, "Move down"));
    await tick();
    expect(sent(t)[0]).toEqual([
      { kind: "group", group_id: "g1", workspaces: [member("web")] },
      flat("api"),
      flat("cli"),
    ]);
    expect(document.activeElement?.getAttribute("data-row")).toBe("api");
    await t.feedEvent("workspace.layout", { layout: { entries: [group("g1", "backend", "web"), flat("api"), flat("cli")] } });

    const inside = menu("web", "web");
    expect(item(inside, "Move up").disabled).toBe(true);
    expect(item(inside, "Move down").disabled).toBe(true);
  });

  it("moves a group from its menu", async () => {
    const t = host({ entries: [flat("api"), group("g1", "backend", "web")] }, [repo("api"), repo("web")]);
    await mount(t);
    fireEvent.click(screen.getByTitle("Group actions"));
    const m = within(screen.getByRole("menu", { name: "backend actions" }));
    expect(item(m, "Move down").disabled).toBe(true);
    fireEvent.click(item(m, "Move up"));
    await tick();
    expect(rows()).toEqual(["group-g1", "web", "api"]);
    expect(document.activeElement?.getAttribute("data-row")).toBe("g1");
  });

  it("moves a worktree inside its repository", async () => {
    const t = host({ entries: [flat("api", ["w1", "w2"])] }, [repo("api"), worktree("w1", "api"), worktree("w2", "api")]);
    await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "w1 actions" }));
    const m = within(screen.getByRole("menu", { name: "w1 actions" }));
    expect(item(m, "Move up").disabled).toBe(true);
    fireEvent.click(item(m, "Move down"));
    await tick();
    expect(sent(t)).toEqual([[flat("api", ["w2", "w1"])]]);
    expect(document.activeElement?.getAttribute("data-row")).toBe("w1");
  });

  it("disables moves offline", async () => {
    const t = host({ entries: [flat("api"), flat("web")] }, [repo("api"), repo("web")]);
    await mount(t);
    const open = menu("api", "api");
    expect(item(open, "Move down").disabled).toBe(false);
    fireEvent.click(item(open, "Move down"));
    await tick();
    await t.feedEvent("workspace.layout", { layout: { entries: [flat("web"), flat("api")] } });
    await offline(t);
    fireEvent.click(screen.getByRole("button", { name: "Undo" }));
    await tick();
    expect(sent(t)).toHaveLength(1);
  });
});
