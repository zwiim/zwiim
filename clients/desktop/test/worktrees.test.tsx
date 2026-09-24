import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  HostClient,
  HostController,
  ZwiimClientError,
  type SessionSummary,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { DiffPanel, DiffView } from "@/panels/DiffPanel";
import { SessionPanel } from "@/panels/SessionPanel";
import { busyMessage, Sidebar, type SidebarProps } from "@/panels/Sidebar";
import { FakeTransport, tick } from "./fakes";

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const BASE = "3f9e1a2".padEnd(40, "b");
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
  base_commit: BASE,
  removed: false,
  ...extra,
});
const session = (id: string, agent: string, extra: Partial<SessionSummary> = {}): SessionSummary => ({
  session_id: id,
  workspace_id: null,
  title: `task ${id}`,
  agent,
  status: "pending_review",
  live: false,
  created_at: "",
  updated_at: "",
  ...extra,
});

async function mount(t: FakeTransport, ui: (c: HostController) => React.ReactNode) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(<HostProvider value={controller}>{ui(controller)}</HostProvider>);
  await tick();
  return controller;
}

function sidebarProps(overrides: Partial<SidebarProps> = {}): SidebarProps {
  return {
    workspaceId: null,
    sessionId: null,
    onSelectWorkspace: vi.fn(),
    onSelectSession: vi.fn(),
    pickFolder: vi.fn(async () => null),
    ...overrides,
  };
}

/** A host with a root, a working worktree, and a removed worktree that kept a session. */
function treeHost() {
  const t = new FakeTransport();
  const sessions: Record<string, SessionSummary[]> = {
    root: [session("s0", "claude_code")],
    a1b2c3d4: [
      session("s1", "claude_code", { live: true, status: "in_progress" }),
      session("s2", "codex", { live: true }),
    ],
    gone0000: [session("s3", "claude_code")],
  };
  t.handlers["workspace.list"] = () => [
    root,
    worktree("a1b2c3d4"),
    worktree("gone0000", { removed: true, branch: null }),
  ];
  t.handlers["session.list"] = (p) => sessions[p.workspace_id] ?? [];
  return { t, sessions };
}

describe("workspace tree", () => {
  it("shows root → worktrees → sessions with branch, base, and working counts", async () => {
    const { t } = treeHost();
    await mount(t, () => <Sidebar {...sidebarProps()} />);
    const tree = screen.getByTestId("workspace-tree").textContent!;
    expect(tree).toContain("repo");
    expect(tree).toContain("main");
    const wt = screen.getByTestId("workspace-a1b2c3d4").textContent!;
    expect(wt).toContain("zwiim/a1b2c3d4");
    expect(wt).toContain("base main@3f9e1a2");
    expect(wt).toContain("1 working");
    // Each session says whether its agent is working; the agent is in the tooltip.
    const rows = [...screen.getByTestId("workspace-a1b2c3d4").querySelectorAll("button[title]")].map((b) => b.getAttribute("title"));
    expect(rows).toContain("Claude Code · working");
    expect(rows).toContain("Codex · idle");
    // Nested under the root, not beside it.
    expect(screen.getByTestId("workspace-root").contains(screen.getByTestId("workspace-a1b2c3d4"))).toBe(true);
    const gone = screen.getByTestId("workspace-gone0000");
    expect(gone.textContent).toContain("repo-gone0000");
    expect(gone.textContent).toContain("removed");
    expect(gone.textContent).toContain("task s3");
    expect(gone.querySelector('[title="Remove worktree"]')).toBeNull();
    expect(screen.getAllByTitle("Remove worktree")).toHaveLength(1);
  });

  it("selects a session under its worktree", async () => {
    const { t } = treeHost();
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    fireEvent.click(screen.getByText("task s2"));
    expect(props.onSelectSession).toHaveBeenCalledWith("a1b2c3d4", "s2");
  });

  it("creates a worktree explicitly and selects it", async () => {
    const { t } = treeHost();
    t.handlers["workspace.create_worktree"] = () => worktree("9f8e7d6c");
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    fireEvent.click(screen.getByTitle("New worktree"));
    await tick();
    expect(t.calls.find((c) => c.op === "workspace.create_worktree")?.params).toEqual({ workspace_id: "root" });
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("9f8e7d6c");
    expect(screen.getByTestId("workspace-9f8e7d6c").textContent).toContain("zwiim/9f8e7d6c");
    expect(t.calls.some((c) => c.op === "session.start")).toBe(false);
  });
});

describe("worktree removal", () => {
  it("confirms, then confirms the loss of dirty changes before forcing", async () => {
    const { t } = treeHost();
    const params: unknown[] = [];
    t.handlers["workspace.remove_worktree"] = (p) => {
      params.push(p);
      if (!p.force) throw new ZwiimClientError("workspace_dirty", "dirty", 409, { changes: 3 });
      return { workspace_id: p.workspace_id, parent_workspace_id: "root", kept_sessions: 2 };
    };
    await mount(t, () => <Sidebar {...sidebarProps()} />);
    fireEvent.click(screen.getByTitle("Remove worktree"));
    const dialog = screen.getByRole("alertdialog");
    expect(dialog.textContent).toContain("Branch zwiim/a1b2c3d4 will be kept.");
    expect(params).toEqual([]);

    fireEvent.click(screen.getByText("Remove"));
    await tick();
    expect(screen.getByRole("alertdialog").textContent).toContain(
      "3 uncommitted changes will be lost. Branch zwiim/a1b2c3d4 will be kept.",
    );
    fireEvent.click(screen.getByText("Discard and remove"));
    await tick();
    expect(params).toEqual([{ workspace_id: "a1b2c3d4" }, { workspace_id: "a1b2c3d4", force: true }]);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("reports a busy worktree and never forces past it", async () => {
    const { t } = treeHost();
    const params: unknown[] = [];
    t.handlers["workspace.remove_worktree"] = (p) => {
      params.push(p);
      throw new ZwiimClientError("workspace_busy", "busy", 409, { live_sessions: 2, terminals: 1 });
    };
    await mount(t, () => <Sidebar {...sidebarProps()} />);
    fireEvent.click(screen.getByTitle("Remove worktree"));
    fireEvent.click(screen.getByText("Remove"));
    await tick();
    expect(screen.getByRole("alertdialog").textContent).toContain(
      "2 agents running, 1 terminal open — stop them to remove.",
    );
    fireEvent.click(screen.getByText("Try again"));
    await tick();
    expect(params).toEqual([{ workspace_id: "a1b2c3d4" }, { workspace_id: "a1b2c3d4" }]);
  });

  it("phrases busy counts", () => {
    expect(busyMessage({ live_sessions: 1, terminals: 0 })).toBe("1 agent running — stop it to remove.");
    expect(busyMessage({ live_sessions: 0, terminals: 2 })).toBe("2 terminals open — stop them to remove.");
  });
});

describe("diff", () => {
  it("lists changed files with counts and indicators, and shows a file's patch", async () => {
    const t = new FakeTransport();
    t.handlers["git.diff"] = (p) => ({
      base_commit: BASE,
      truncated: false,
      files: [
        { path: "src/a.rs", change: "modified", additions: 2, deletions: 1, binary: false, too_large: false, untracked: false },
        { path: "new.txt", change: "added", additions: 1, deletions: 0, binary: false, too_large: false, untracked: true },
        { path: "logo.png", change: "added", additions: null, deletions: null, binary: true, too_large: false, untracked: true },
      ],
      ...(p.file === "src/a.rs" ? { patch: "@@ -1 +1,2 @@\n-old\n+new\n+more" } : {}),
      ...(p.file === "logo.png" ? { patch: null } : {}),
    });
    const onOpen = vi.fn();
    await mount(t, () => <DiffPanel workspace={worktree("a1b2c3d4")} selected={null} onOpen={onOpen} />);
    expect(screen.getByText("Against main@3f9e1a2")).toBeTruthy();
    const files = screen.getByTestId("diff-files").textContent!;
    expect(files).toContain("src/a.rs");
    expect(files).toContain("+2");
    expect(files).toContain("−1");
    expect(files).toContain("untracked");
    expect(files).toContain("binary");
    // The list only lists: a file's patch opens in its own tab.
    fireEvent.click(screen.getByText("src/a.rs"));
    expect(onOpen).toHaveBeenCalledWith("src/a.rs");
    expect(screen.queryByTestId("diff-patch")).toBeNull();
    cleanup();

    await mount(t, () => <DiffView workspace={worktree("a1b2c3d4")} path="src/a.rs" />);
    const patch = screen.getByTestId("diff-patch").textContent!;
    expect(patch).toContain("-old");
    expect(patch).toContain("+more");
    cleanup();
    await mount(t, () => <DiffView workspace={worktree("a1b2c3d4")} path="logo.png" />);
    expect(screen.getByTestId("diff-patch").textContent).toContain("Binary file — content not shown.");
    expect(t.calls.filter((c) => c.op === "git.diff").map((c) => c.params)).toEqual([
      { workspace_id: "a1b2c3d4" },
      { workspace_id: "a1b2c3d4", file: "src/a.rs" },
      { workspace_id: "a1b2c3d4", file: "logo.png" },
    ]);
  });

  it("labels the root's diff as uncommitted changes", async () => {
    const t = new FakeTransport();
    t.handlers["git.diff"] = () => ({ base_commit: BASE, truncated: false, files: [] });
    await mount(t, () => <DiffPanel workspace={root} selected={null} onOpen={() => {}} />);
    expect(screen.getByText("Uncommitted changes against HEAD")).toBeTruthy();
    expect(screen.getByText("No changes")).toBeTruthy();
  });
});

describe("sessions and removed worktrees", () => {
  it("New worktree + session chains create then start in the new worktree", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    t.handlers["workspace.create_worktree"] = () => worktree("9f8e7d6c");
    t.handlers["session.start"] = () => ({ session_id: "s9" });
    const created = vi.fn();
    const started = vi.fn();
    await mount(t, () => (
      <SessionPanel workspaceId="root" sessionId={null} onSessionStarted={started} onWorkspaceCreated={created} />
    ));
    fireEvent.click(screen.getByLabelText("In a new worktree"));
    fireEvent.change(screen.getByPlaceholderText("Describe the task…"), { target: { value: "go" } });
    fireEvent.click(screen.getByTitle("Send"));
    await tick();
    expect(t.calls.map((c) => c.op).filter((op) => op.startsWith("workspace.create") || op === "session.start")).toEqual([
      "workspace.create_worktree",
      "session.start",
    ]);
    expect(t.calls.find((c) => c.op === "session.start")?.params).toMatchObject({ workspace_id: "9f8e7d6c" });
    expect(created).toHaveBeenCalledWith("9f8e7d6c");
    expect(started).toHaveBeenCalledWith("s9");
  });

  it("keeps a removed worktree's session readable but never sendable", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root, worktree("gone0000", { removed: true, branch: null })];
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        { turn_id: "t1", role: "user", blocks: [{ type: "text", text: "old question" }], timestamp: "", completed_at: null, duration_ms: null, model: null, usage: null },
      ],
    });
    await mount(t, () => <SessionPanel workspaceId="gone0000" sessionId="s3" onSessionStarted={() => {}} />);
    expect(screen.getByTestId("transcript").textContent).toContain("old question");
    const composer = screen.getByPlaceholderText("This worktree was removed — the session is read-only") as HTMLTextAreaElement;
    expect(composer.disabled).toBe(true);
    expect((screen.getByTitle("Send") as HTMLButtonElement).disabled).toBe(true);
    expect(t.calls.some((c) => c.op === "session.send")).toBe(false);
    cleanup();

    await mount(t, () => <SessionPanel workspaceId="gone0000" sessionId={null} onSessionStarted={() => {}} />);
    expect(screen.getByText(/no new work can start here/)).toBeTruthy();
    expect(screen.queryByPlaceholderText("Describe the task…")).toBeNull();
  });

  it("shows a send rejected with workspace_removed", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root, worktree("a1b2c3d4")];
    t.handlers["session.send"] = () => {
      throw new ZwiimClientError("workspace_removed", "The workspace was removed", 410);
    };
    await mount(t, () => <SessionPanel workspaceId="a1b2c3d4" sessionId="s1" onSessionStarted={() => {}} />);
    fireEvent.change(screen.getByPlaceholderText("Send a message"), { target: { value: "more" } });
    fireEvent.click(screen.getByTitle("Send"));
    await tick();
    expect(screen.getByRole("alert").textContent).toBe("The workspace was removed");
  });
});
