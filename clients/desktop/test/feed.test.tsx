import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type SessionSummary, type WorkspaceInfo } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { DiffPanel } from "@/panels/DiffPanel";
import { Sidebar, type SidebarProps } from "@/panels/Sidebar";
import { TerminalPanel } from "@/panels/TerminalPanel";
import { FakeTransport, tick } from "./fakes";

// jsdom cannot render xterm; the panel's terminal list is what is under test.
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    loadAddon() {}
    open() {}
    focus() {}
    reset() {}
    write() {}
    dispose() {}
    onData() {
      return { dispose() {} };
    }
    onResize() {
      return { dispose() {} };
    }
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
globalThis.ResizeObserver ??= class {
  observe() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

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
};
const worktree = (id: string, extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  workspace_id: id,
  name: `repo-${id}`,
  alias: null,
  kind: "worktree",
  parent_workspace_id: "root",
  branch: `zwiim/${id}`,
  base_branch: "main",
  base_commit: "3f9e1a2".padEnd(40, "b"),
  removed: false,
  ...extra,
});
const session = (id: string, workspace_id: string, extra: Partial<SessionSummary> = {}): SessionSummary => ({
  session_id: id,
  workspace_id,
  title: `task ${id}`,
  agent: "claude_code",
  status: "pending_review",
  live: false,
  created_at: "2026-09-24T00:00:00Z",
  updated_at: "2026-09-24T00:00:00Z",
  ...extra,
});
const LIST_OPS = ["workspace.list", "session.list", "terminal.list"];

async function mount(t: FakeTransport, ui: React.ReactNode) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(<HostProvider value={controller}>{ui}</HostProvider>);
  await tick();
  return controller;
}

const sidebarProps = (): SidebarProps => ({
  workspaceId: null,
  sessionId: null,
  onSelectWorkspace: vi.fn(),
  onSelectSession: vi.fn(),
  pickFolder: vi.fn(async () => null),
});

describe("workspace tree on the host feed", () => {
  it("does not poll the host", async () => {
    vi.useFakeTimers();
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    await mount(t, <Sidebar {...sidebarProps()} />);
    expect(screen.getByTestId("workspace-root")).toBeTruthy();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    expect(t.calls.filter((c) => LIST_OPS.includes(c.op))).toEqual([]);
  });

  it("shows another client's worktree, session and working status", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    await mount(t, <Sidebar {...sidebarProps()} />);
    expect(screen.queryByTestId("workspace-a1b2c3d4")).toBeNull();

    await t.feedEvent("workspace.changed", { workspace: worktree("a1b2c3d4") });
    await t.feedEvent("session.changed", { session: session("s1", "a1b2c3d4") });
    const row = () => screen.getByTestId("workspace-a1b2c3d4").textContent!;
    expect(row()).toContain("zwiim/a1b2c3d4");
    expect(row()).toContain("task s1");
    expect(row()).not.toContain("working ");

    await t.feedEvent("session.changed", { session: session("s1", "a1b2c3d4", { live: true, status: "in_progress" }) });
    await t.feedEvent("session.changed", { session: session("s2", "a1b2c3d4", { live: true, status: "in_progress" }) });
    expect(row()).toContain("2 working");
    await t.feedEvent("session.changed", { session: session("s1", "a1b2c3d4", { live: false, status: "pending_review" }) });
    expect(row()).toContain("1 working");
    expect(t.calls.filter((c) => LIST_OPS.includes(c.op))).toEqual([]);
  });

  it("follows branch changes and worktree removal", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root, worktree("a1b2c3d4"), worktree("b2c3d4e5")];
    t.handlers["session.list"] = (p) => (p.workspace_id === "a1b2c3d4" ? [session("s1", "a1b2c3d4")] : []);
    await mount(t, <Sidebar {...sidebarProps()} />);

    await t.feedEvent("workspace.changed", { workspace: { ...root, branch: "feature" } });
    expect(screen.getByTestId("workspace-root").textContent).toContain("feature");

    // Removed with sessions: stays, marked removed. Without: disappears.
    await t.feedEvent("workspace.changed", { workspace: worktree("a1b2c3d4", { removed: true, branch: null }) });
    await t.feedEvent("workspace.removed", { workspace_id: "b2c3d4e5" });
    const gone = screen.getByTestId("workspace-a1b2c3d4");
    expect(gone.textContent).toContain("removed");
    expect(gone.textContent).toContain("task s1");
    expect(gone.querySelector('[title="Remove worktree"]')).toBeNull();
    expect(screen.queryByTestId("workspace-b2c3d4e5")).toBeNull();
  });
});

describe("diff on the host feed", () => {
  it("refetches the open workspace's diff and file patch on workspace.diff_changed", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root, worktree("a1b2c3d4")];
    let files = ["a.txt"];
    let version = 1;
    t.handlers["git.diff"] = (p) => ({
      base_commit: null,
      truncated: false,
      files: (p.file ? [p.file] : files).map((path: string) => ({
        path,
        change: "modified",
        additions: version,
        deletions: 0,
        binary: false,
        too_large: false,
        untracked: false,
      })),
      ...(p.file ? { patch: `+v${version}` } : {}),
    });
    await mount(t, <DiffPanel workspace={worktree("a1b2c3d4")} />);
    fireEvent.click(screen.getByText("a.txt"));
    await tick();
    expect(screen.getByTestId("diff-patch").textContent).toContain("+v1");
    const diffCalls = () => t.calls.filter((c) => c.op === "git.diff").length;
    const before = diffCalls();

    // Another workspace's change is not ours to fetch.
    await t.feedEvent("workspace.diff_changed", { workspace_id: "root" });
    await tick();
    expect(diffCalls()).toBe(before);

    files = ["a.txt", "b.txt"];
    version = 2;
    await t.feedEvent("workspace.diff_changed", { workspace_id: "a1b2c3d4" });
    await tick();
    expect(screen.getByTestId("diff-files").textContent).toContain("b.txt");
    expect(screen.getByTestId("diff-patch").textContent).toContain("+v2");
    expect(t.calls.filter((c) => c.op === "git.diff").slice(before).map((c) => c.params)).toEqual([
      { workspace_id: "a1b2c3d4" },
      { workspace_id: "a1b2c3d4", file: "a.txt" },
    ]);

    // The open file was committed away: the selection closes.
    files = ["b.txt"];
    await t.feedEvent("workspace.diff_changed", { workspace_id: "a1b2c3d4" });
    await tick();
    expect(screen.getByTestId("diff-files").textContent).not.toContain("a.txt");
    expect(screen.getByTestId("diff-patch").textContent).toContain("Select a file");
  });
});

describe("terminals on the host feed", () => {
  it("shows terminals another client opens and removes those that end", async () => {
    const t = new FakeTransport();
    t.handlers["terminal.snapshot"] = (p) => ({ terminal_id: p.terminal_id, alive: true, data: "", seq: 0 });
    await mount(t, <TerminalPanel workspaceId="w1" />);
    expect(screen.getByText("No terminals in this workspace. Open one with +.")).toBeTruthy();

    await t.feedEvent("terminal.changed", { terminal: { terminal_id: "t1", title: "zsh", workspace_id: "w1" } });
    await t.feedEvent("terminal.changed", { terminal: { terminal_id: "t2", title: "bash", workspace_id: "w1" } });
    // Another workspace's terminal is not this workspace's.
    await t.feedEvent("terminal.changed", { terminal: { terminal_id: "t3", title: "fish", workspace_id: "w2" } });
    await tick();
    expect(screen.getByRole("tab", { name: "zsh" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "bash" })).toBeTruthy();
    expect(screen.queryByText("fish")).toBeNull();

    // t2 is not on screen: gone. t1 is: kept as ended until closed.
    await t.feedEvent("terminal.removed", { terminal_id: "t2" });
    await t.feedEvent("terminal.removed", { terminal_id: "t1" });
    expect(screen.queryByRole("tab", { name: /bash/ })).toBeNull();
    expect(screen.getByRole("tab", { name: "zsh (exited)" })).toBeTruthy();
    fireEvent.click(screen.getByTitle("Close"));
    await tick();
    expect(screen.queryByRole("tab", { name: /zsh/ })).toBeNull();
    expect(t.calls.filter((c) => LIST_OPS.includes(c.op))).toEqual([]);
  });

  it("kills a terminal only after confirming", async () => {
    const t = new FakeTransport();
    t.handlers["terminal.snapshot"] = (p) => ({ terminal_id: p.terminal_id, alive: true, data: "", seq: 0 });
    t.handlers["terminal.kill"] = () => ({});
    await mount(t, <TerminalPanel workspaceId="w1" />);
    await t.feedEvent("terminal.changed", { terminal: { terminal_id: "t1", title: "zsh", workspace_id: "w1" } });
    await tick();
    fireEvent.click(screen.getByLabelText("Kill zsh"));
    expect(screen.getByTestId("confirm-kill").textContent).toContain("Kill zsh?");
    fireEvent.click(screen.getByText("Keep"));
    expect(screen.queryByTestId("confirm-kill")).toBeNull();
    expect(t.calls.some((c) => c.op === "terminal.kill")).toBe(false);
    fireEvent.click(screen.getByLabelText("Kill zsh"));
    fireEvent.click(screen.getByRole("button", { name: "Kill" }));
    await tick();
    expect(t.calls.filter((c) => c.op === "terminal.kill").map((c) => c.params)).toEqual([{ terminal_id: "t1" }]);
  });
});
