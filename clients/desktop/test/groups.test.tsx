import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  type LayoutEntry,
  type SessionSummary,
  type WorkspaceInfo,
  type WorkspaceLayout,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { sidebarFolds } from "@/lib/prefs";
import { Sidebar, type SidebarProps } from "@/panels/Sidebar";
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
const worktree = (id: string, parent: string): WorkspaceInfo =>
  repo(id, { kind: "worktree", parent_workspace_id: parent, branch: `zwiim/${id}` });
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
  t.layout = layout;
  return t;
}

async function mount(t: FakeTransport, overrides: Partial<SidebarProps> = {}) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  const props: SidebarProps = {
    workspaceId: null,
    onSelectWorkspace: vi.fn(),
    onSessionStarted: vi.fn(),
    pickFolder: vi.fn(async () => null),
    remote: false,
    ...overrides,
  };
  client.start();
  await tick();
  const ui = (p: SidebarProps) => (
    <HostProvider value={controller}>
      <Sidebar {...p} />
    </HostProvider>
  );
  const view = render(ui(props));
  await tick();
  return { rerender: (next: Partial<SidebarProps>) => view.rerender(ui({ ...props, ...next })) };
}

const rows = () =>
  [...screen.getByTestId("workspace-tree").querySelectorAll<HTMLElement>("[data-testid]")].map((e) =>
    e.dataset.testid!.replace("workspace-", ""),
  );
const groupRow = (name: string) =>
  screen.getAllByRole("button").find((b) => b.title === name && b.hasAttribute("aria-expanded"))!;
const openNewGroup = async () => {
  fireEvent.click(screen.getByTitle("Add"));
  fireEvent.click(within(screen.getByRole("menu", { name: "Add" })).getByRole("menuitem", { name: "New group" }));
  await tick();
};
const groupMenu = async (name: string, item: string) => {
  fireEvent.click(screen.getByTitle("Group actions", { exact: true }));
  await tick();
  fireEvent.click(within(screen.getByRole("menu", { name: `${name} actions` })).getByRole("menuitem", { name: item }));
  await tick();
};

beforeEach(() => localStorage.clear());
afterEach(() => cleanup());

describe("workspace groups in the sidebar", () => {
  it("lists groups and ungrouped rows in one mixed order, with a count and no icon", async () => {
    await mount(host({ entries: [flat("cli"), group("g1", "backend", "api", "web")] }));
    expect(rows()).toEqual(["cli", "group-g1", "api", "web"]);
    const row = groupRow("backend");
    expect(row.textContent).toBe("backend2");
    expect(row.title).toBe("backend");
    expect(screen.getByTestId("group-g1").contains(screen.getByTestId("workspace-api"))).toBe(true);
    expect(screen.getByTestId("workspace-cli").closest('[data-testid^="group-"]')).toBeNull();
  });

  it("stays flat on a Host from before groups, with no way to add one", async () => {
    const t = host(undefined);
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "workspace.layout") });
    await mount(t);
    expect(rows()).toEqual(["api", "web", "cli"]);
    fireEvent.click(screen.getByTitle("Add"));
    expect(within(screen.getByRole("menu", { name: "Add" })).queryByRole("menuitem", { name: "New group" })).toBeNull();
  });

  it("follows another device's layout", async () => {
    const t = host({ entries: [flat("api"), flat("web"), flat("cli")] });
    await mount(t);
    await t.feedEvent("workspace.layout", { layout: { entries: [group("g1", "backend", "web"), flat("api"), flat("cli")] } });
    expect(rows()).toEqual(["group-g1", "web", "api", "cli"]);
  });

  it("folds on the device, rolling up working and selection", async () => {
    const t = host({ entries: [group("g1", "backend", "api")] }, [repo("api"), worktree("wt", "api")]);
    const running: SessionSummary = {
      session_id: "s1",
      workspace_id: null,
      title: "t",
      agent: "claude_code",
      status: "in_progress",
      live: true,
      created_at: "",
      updated_at: "",
    };
    t.handlers["session.list"] = (p) => (p.workspace_id === "wt" ? [running] : []);
    const { rerender } = await mount(t, { workspaceId: "wt" });
    expect(groupRow("backend").textContent).not.toContain("working");

    fireEvent.click(groupRow("backend"));
    expect(screen.queryByTestId("workspace-api")).toBeNull();
    expect(groupRow("backend").getAttribute("aria-expanded")).toBe("false");
    expect(groupRow("backend").textContent).toContain("1 working");
    expect(groupRow("backend").parentElement!.className).toContain("bg-sidebar-accent");
    expect(sidebarFolds().groups).toEqual(["g1"]);

    // Folding around the selection stays folded; a new selection inside opens it.
    rerender({ workspaceId: "api" });
    await tick();
    expect(screen.getByTestId("workspace-api")).toBeTruthy();
    expect(sidebarFolds().groups).toEqual([]);
  });

  it("opens a folded group when the selected worktree turns up inside it", async () => {
    const t = host({ entries: [group("g1", "backend", "api")] }, [repo("api")]);
    const { rerender } = await mount(t);
    fireEvent.click(groupRow("backend"));
    expect(sidebarFolds().groups).toEqual(["g1"]);

    // The worktree is selected as soon as it is made; the feed lists it after.
    rerender({ workspaceId: "wt" });
    await tick();
    expect(sidebarFolds().groups).toEqual(["g1"]);

    await t.feedEvent("workspace.changed", { workspace: worktree("wt", "api") });
    await t.feedEvent("workspace.layout", {
      layout: { entries: [{ kind: "group", group_id: "g1", name: "backend", workspaces: [member("api", ["wt"])] }] },
    });
    expect(sidebarFolds().groups).toEqual([]);
    expect(screen.getByTestId("workspace-api")).toBeTruthy();
  });

  it("creates a group from a draft row at the end of the list", async () => {
    const t = host({ entries: [flat("api")] }, [repo("api")]);
    t.handlers["workspace.group_create"] = (p) => ({ group_id: "g9", name: p.name });
    await mount(t);
    await openNewGroup();
    const input = screen.getByRole("textbox", { name: "New group name" }) as HTMLInputElement;
    expect(document.activeElement).toBe(input);
    fireEvent.change(input, { target: { value: "  backend " } });
    fireEvent.keyDown(input, { key: "Enter" });
    await tick();
    expect(t.calls.find((c) => c.op === "workspace.group_create")?.params).toEqual({ name: "backend" });
    expect(screen.queryByRole("textbox")).toBeNull();
    await t.feedEvent("workspace.layout", { layout: { entries: [flat("api"), group("g9", "backend")] } });
    expect(rows()).toEqual(["api", "group-g9"]);
    expect(screen.getByText("Drag here, or use Move to group")).toBeTruthy();
  });

  it("cancels a draft on Esc or an empty name, writing nothing", async () => {
    const t = host({ entries: [flat("api")] });
    await mount(t);
    await openNewGroup();
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Escape" });
    expect(screen.queryByRole("textbox")).toBeNull();
    await openNewGroup();
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "   " } });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    await tick();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(t.calls.some((c) => c.op === "workspace.group_create")).toBe(false);
  });

  it("keeps a refused name to fix, showing the Host's error", async () => {
    const t = host({ entries: [] });
    t.handlers["workspace.group_create"] = () => {
      throw new Error("too long");
    };
    await mount(t);
    await openNewGroup();
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "x" } });
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" });
    await tick();
    expect(screen.getByRole("textbox")).toBeTruthy();
    expect(screen.getByText(/too long/)).toBeTruthy();
  });

  it("renames in place", async () => {
    const t = host({ entries: [group("g1", "backend", "api")] });
    t.handlers["workspace.group_rename"] = (p) => ({ group_id: p.group_id, name: p.name });
    await mount(t);
    await groupMenu("backend", "Rename");
    const input = screen.getByRole("textbox", { name: "Rename backend" });
    fireEvent.change(input, { target: { value: "servers" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await tick();
    expect(t.calls.find((c) => c.op === "workspace.group_rename")?.params).toEqual({ group_id: "g1", name: "servers" });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(document.activeElement).toBe(groupRow("backend"));
  });

  it("deletes an empty group at once", async () => {
    const t = host({ entries: [group("g1", "backend")] });
    t.handlers["workspace.group_delete"] = () => ({});
    await mount(t);
    await groupMenu("backend", "Delete group");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(t.calls.find((c) => c.op === "workspace.group_delete")?.params).toEqual({ group_id: "g1" });
  });

  it("asks in the row before deleting a group with members", async () => {
    const t = host({ entries: [group("g1", "backend", "api", "web")] });
    t.handlers["workspace.group_delete"] = () => ({});
    await mount(t);
    await groupMenu("backend", "Delete group");
    const card = within(screen.getByRole("alertdialog", { name: "Delete backend" }));
    expect(screen.getByRole("alertdialog").textContent).toContain(
      "Delete backend? Its 2 repositories move to the top level, in place. Nothing on disk changes.",
    );
    expect(document.activeElement).toBe(card.getByRole("button", { name: "Cancel" }));
    expect(t.calls.some((c) => c.op === "workspace.group_delete")).toBe(false);
    fireEvent.click(card.getByRole("button", { name: "Delete" }));
    await tick();
    expect(t.calls.find((c) => c.op === "workspace.group_delete")?.params).toEqual({ group_id: "g1" });
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("disables adding and the group menu offline", async () => {
    const t = host({ entries: [group("g1", "backend")] });
    await mount(t);
    await act(async () => t.handlersForEvents!.onClose({ code: 1006, reason: "" }));
    await tick();
    expect((screen.getByTitle("Add") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTitle("Group actions") as HTMLButtonElement).disabled).toBe(true);
  });
});
