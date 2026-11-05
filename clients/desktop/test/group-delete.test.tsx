import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type WorkspaceInfo, type WorkspaceLayout } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { Sidebar } from "@/panels/Sidebar";
import { FakeTransport, tick } from "./fakes";

const repo = (id: string): WorkspaceInfo => ({
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
});
const member = (id: string) => ({ workspace_id: id, worktrees: [] });

const LAYOUT: WorkspaceLayout = {
  entries: [
    { kind: "group", group_id: "g1", name: "backend", workspaces: [member("api")] },
    { kind: "workspace", ...member("cli") },
  ],
};

async function mount() {
  const t = new FakeTransport();
  t.handlers["workspace.list"] = () => [repo("api"), repo("cli")];
  t.handlers["workspace.layout_set"] = () => ({});
  t.handlers["workspace.group_delete"] = () => ({});
  t.layout = LAYOUT;
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
  return t;
}

const openDelete = async () => {
  fireEvent.click(screen.getByTitle("Group actions", { exact: true }));
  await tick();
  fireEvent.click(screen.getByRole("menuitem", { name: "Delete group" }));
  await tick();
};

beforeEach(() => localStorage.clear());
afterEach(() => cleanup());

describe("deleting a group", () => {
  it("closes the confirm on Esc, deleting nothing, and returns focus to the row", async () => {
    const t = await mount();
    await openDelete();
    expect(screen.getByRole("alertdialog", { name: "Delete backend" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    await tick();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(t.calls.some((c) => c.op === "workspace.group_delete")).toBe(false);
    expect(document.activeElement?.getAttribute("title")).toBe("backend");
  });

  it("clears the note of a move into it, whose Undo has nowhere left to go back to", async () => {
    await mount();
    fireEvent.click(within(screen.getByTestId("workspace-cli")).getAllByRole("button", { name: "cli actions" })[0]);
    await tick();
    fireEvent.click(screen.getByRole("menuitem", { name: "backend" }));
    await tick();
    expect(screen.getByRole("status").textContent).toContain("Moved cli to backend");
    await openDelete();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await tick();
    expect(screen.queryByRole("status")).toBeNull();
  });
});
