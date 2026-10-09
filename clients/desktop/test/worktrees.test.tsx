import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
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
import { FakeTransport, hostHello, tick } from "./fakes";

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
  is_git_repo: true,
  display_path: null,
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
  is_git_repo: true,
  display_path: null,
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
    onSelectWorkspace: vi.fn(),
    onSessionStarted: vi.fn(),
    pickFolder: vi.fn(async () => null),
    remote: false,
    ...overrides,
  };
}

/** A Host from before publishing: + adds the picked folder as it is (the publish flow is in publish.test). */
function beforePublishing(t: FakeTransport) {
  t.handlers["host.hello"] = () =>
    hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "workspace.publish") });
  return t;
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
  it("shows root → worktrees with branch, base, and working counts, but not sessions", async () => {
    const { t } = treeHost();
    await mount(t, () => <Sidebar {...sidebarProps()} />);
    const tree = screen.getByTestId("workspace-tree").textContent!;
    expect(tree).toContain("repo");
    expect(tree).toContain("main");
    const wt = screen.getByTestId("workspace-a1b2c3d4").textContent!;
    expect(wt).toContain("zwiim/a1b2c3d4");
    expect(screen.getByTitle("base main@3f9e1a2").textContent).toContain("zwiim/a1b2c3d4");
    expect(wt).toContain("1 working");
    // Sessions are tabs beside the conversation, not sidebar rows.
    expect(wt).not.toContain("task s1");
    expect(screen.queryByText("New session")).toBeNull();
    // Nested under the root, not beside it.
    expect(screen.getByTestId("workspace-root").contains(screen.getByTestId("workspace-a1b2c3d4"))).toBe(true);
    // Removed worktrees fold under one row until asked for.
    expect(screen.queryByTestId("workspace-gone0000")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "1 removed" }));
    const gone = screen.getByTestId("workspace-gone0000");
    expect(gone.textContent).toContain("repo-gone0000");
    expect(gone.querySelector('[title="Remove worktree"]')).toBeNull();
    expect(screen.getAllByTitle("Remove worktree")).toHaveLength(1);
  });

  it("offers no worktrees for a plain folder, and leaves it out of the repository list", async () => {
    const { t } = treeHost();
    // Its click selects it as it always did (a root's click opens a worktree since, in publish.test).
    beforePublishing(t);
    const plain: WorkspaceInfo = { ...root, workspace_id: "plain", name: "notes", is_git_repo: false, branch: null };
    t.handlers["workspace.list"] = () => [root, worktree("a1b2c3d4"), plain];
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    const row = screen.getByTestId("workspace-plain");
    expect(row.textContent).toContain("notes");
    expect(row.querySelector('[title="New worktree"]')).toBeNull();
    expect(screen.getAllByTitle("New worktree")).toHaveLength(1);
    fireEvent.click(screen.getByText("notes"));
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("plain");

    fireEvent.click(screen.getByTitle("New worktree"));
    await tick();
    const repos = within(screen.getByRole("dialog", { name: "New worktree" })).getByLabelText("Repository") as HTMLSelectElement;
    expect([...repos.options].map((o) => o.value)).toEqual(["root"]);
  });

  it("selects a worktree", async () => {
    const { t } = treeHost();
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    fireEvent.click(screen.getByText("zwiim/a1b2c3d4"));
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("a1b2c3d4");
  });

  it("creates a worktree from the dialog without a task, and selects it", async () => {
    const { t } = treeHost();
    t.handlers["workspace.create_worktree"] = () => worktree("9f8e7d6c");
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    fireEvent.click(screen.getByTitle("New worktree"));
    await tick();
    const dialog = screen.getByRole("dialog", { name: "New worktree" });
    expect((within(dialog).getByLabelText("Repository") as HTMLSelectElement).value).toBe("root");
    // Nothing is created until asked.
    expect(t.calls.some((c) => c.op === "workspace.create_worktree")).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await tick();
    expect(t.calls.find((c) => c.op === "workspace.create_worktree")?.params).toEqual({ workspace_id: "root" });
    expect(t.calls.some((c) => c.op === "session.start")).toBe(false);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("9f8e7d6c");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByTestId("workspace-9f8e7d6c").textContent).toContain("zwiim/9f8e7d6c");
    expect(t.calls.some((c) => c.op === "session.start")).toBe(false);
  });
});

describe("folding", () => {
  it("folds every workspace under the header, and adds one with +", async () => {
    const { t } = treeHost();
    beforePublishing(t);
    t.handlers["workspace.add"] = () => ({ ...root, workspace_id: "new", name: "new" });
    const props = sidebarProps({ pickFolder: vi.fn(async () => "/tmp/new") });
    await mount(t, () => <Sidebar {...props} />);
    const header = screen.getByRole("button", { name: "Workspaces" });
    expect(header.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(header);
    expect(screen.queryByTestId("workspace-tree")).toBeNull();
    expect(JSON.parse(window.localStorage.getItem("zwiim.desktop.folds")!).workspaces).toBe(true);
    expect(screen.queryByText("Add workspace")).toBeNull();
    // Adding unfolds, so the new workspace shows.
    fireEvent.click(screen.getByTitle("Add workspace"));
    await tick();
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("new");
    expect(screen.getByTestId("workspace-tree")).toBeTruthy();
  });

  it("asks before adding a large plain folder, and adds it anyway when confirmed", async () => {
    const { t } = treeHost();
    beforePublishing(t);
    t.handlers["workspace.add"] = (p: { allow_large?: boolean }) => {
      if (!p.allow_large) throw new ZwiimClientError("large_folder", "big", 409, { files: 5001, bytes: 10, at_least: true });
      return { ...root, workspace_id: "big", name: "big" };
    };
    const props = sidebarProps({ pickFolder: vi.fn(async () => "/tmp/big") });
    await mount(t, () => <Sidebar {...props} />);

    fireEvent.click(screen.getByTitle("Add workspace"));
    await tick();
    const card = screen.getByRole("alertdialog");
    expect(card.textContent).toContain("Large folder");
    expect(card.textContent).toContain("more than 5,000 files");
    fireEvent.click(within(card).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(props.onSelectWorkspace).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTitle("Add workspace"));
    await tick();
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Add anyway" }));
    await tick();
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("big");
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(t.calls.filter((c) => c.op === "workspace.add").map((c) => c.params)).toEqual([
      { path: "/tmp/big" },
      { path: "/tmp/big" },
      { path: "/tmp/big", allow_large: true },
    ]);
  });

  it("keeps the large-folder card while Add anyway is under way", async () => {
    const { t } = treeHost();
    beforePublishing(t);
    let finish!: () => void;
    t.handlers["workspace.add"] = (p: { allow_large?: boolean }) => {
      if (!p.allow_large) throw new ZwiimClientError("large_folder", "big", 409, { files: 5001, bytes: 10, at_least: true });
      return new Promise((resolve) => {
        finish = () => resolve({ ...root, workspace_id: "big", name: "big" });
      });
    };
    const props = sidebarProps({ pickFolder: vi.fn(async () => "/tmp/big") });
    await mount(t, () => <Sidebar {...props} />);

    fireEvent.click(screen.getByTitle("Add workspace"));
    await tick();
    const card = screen.getByRole("alertdialog");
    fireEvent.click(within(card).getByRole("button", { name: "Add anyway" }));
    await tick();
    const cancel = within(card).getByRole("button", { name: "Cancel" }) as HTMLButtonElement;
    expect(cancel.disabled).toBe(true);
    fireEvent.click(cancel);
    expect(screen.getByRole("alertdialog")).toBeTruthy();

    finish();
    await tick();
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("big");
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("folds a repository's worktrees, keeping their work and selection on its row", async () => {
    const { t } = treeHost();
    await mount(t, () => <Sidebar {...sidebarProps({ workspaceId: "a1b2c3d4" })} />);
    fireEvent.click(screen.getByTitle("Hide worktrees"));
    expect(screen.queryByTestId("workspace-a1b2c3d4")).toBeNull();
    const row = screen.getByTestId("workspace-root");
    expect(row.textContent).toContain("1 working");
    expect(row.firstElementChild!.className).toContain("bg-sidebar-accent");
    expect(JSON.parse(window.localStorage.getItem("zwiim.desktop.folds")!).repositories).toEqual(["root"]);
    cleanup();

    // Remembered across launches.
    await mount(t, () => <Sidebar {...sidebarProps()} />);
    expect(screen.queryByTestId("workspace-a1b2c3d4")).toBeNull();
    fireEvent.click(screen.getByTitle("Show worktrees"));
    expect(screen.getByTestId("workspace-a1b2c3d4")).toBeTruthy();
  });

  it("opens the removed worktrees when one of them is selected", async () => {
    const { t } = treeHost();
    await mount(t, () => <Sidebar {...sidebarProps({ workspaceId: "gone0000" })} />);
    expect(screen.getByTestId("workspace-gone0000")).toBeTruthy();
    expect(screen.getByRole("button", { name: "1 removed" }).getAttribute("aria-expanded")).toBe("true");
  });
});

describe("earlier sessions in the repository folder", () => {
  const earlier = (n: string) => screen.queryByRole("button", { name: n });

  it("folds a root's own sessions under one row beneath its worktrees, which selects the root", async () => {
    const { t, sessions } = treeHost();
    sessions.root = [session("s0", "claude_code"), session("s9", "codex")];
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    const row = earlier("2 earlier sessions")!;
    // Nested under the repository, after its worktrees, styled as the removed row.
    expect(screen.getByTestId("workspace-root").contains(row)).toBe(true);
    expect(screen.getByTestId("workspace-a1b2c3d4").compareDocumentPosition(row)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    );
    expect(row.className).toBe(screen.getByRole("button", { name: "1 removed" }).className);
    expect(row.querySelector("svg.lucide-history")).toBeTruthy();
    fireEvent.click(row);
    expect(props.onSelectWorkspace).toHaveBeenCalledTimes(1);
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("root");
    // Selecting it reads nothing and makes no worktree.
    expect(t.calls.some((c) => c.op === "workspace.inspect" || c.op === "workspace.create_worktree")).toBe(false);
  });

  it("marks the row, not the repository, as the selection", async () => {
    const { t } = treeHost();
    await mount(t, () => <Sidebar {...sidebarProps({ workspaceId: "root" })} />);
    expect(earlier("1 earlier session")!.getAttribute("aria-current")).toBe("true");
    expect(screen.getAllByRole("button", { current: true })).toHaveLength(1);
  });

  it("shows the row for a root with sessions and no worktree", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    t.handlers["session.list"] = (p) => (p.workspace_id === "root" ? [session("s0", "claude_code")] : []);
    await mount(t, () => <Sidebar {...sidebarProps()} />);
    expect(screen.getByTestId("workspace-root").contains(earlier("1 earlier session"))).toBe(true);
  });

  it("shows no row for a root without sessions of its own", async () => {
    const { t, sessions } = treeHost();
    sessions.root = [];
    await mount(t, () => <Sidebar {...sidebarProps()} />);
    expect(screen.getByTestId("workspace-tree").textContent).not.toContain("earlier session");
  });

  it("shows no row on a Host from before publishing", async () => {
    const { t } = treeHost();
    beforePublishing(t);
    await mount(t, () => <Sidebar {...sidebarProps({ workspaceId: "root" })} />);
    expect(screen.getByTestId("workspace-tree").textContent).not.toContain("earlier session");
  });

  it("replaces a root session's composer with Continue in a new worktree", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    t.handlers["session.list"] = (p) => (p.workspace_id === "root" ? [session("s0", "claude_code")] : []);
    const onContinue = vi.fn();
    await mount(t, () => (
      <SessionPanel workspaceId="root" sessionId="s0" onSessionStarted={() => {}} onContinue={onContinue} />
    ));
    expect(screen.getByText("This session ran in the repository folder.")).toBeTruthy();
    expect(screen.queryByPlaceholderText("Send a message")).toBeNull();
    expect(screen.queryByTitle("Send")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Continue in a new worktree" }));
    expect(onContinue).toHaveBeenCalledTimes(1);
    cleanup();

    // With no session open, nothing can be drafted there either.
    await mount(t, () => (
      <SessionPanel workspaceId="root" sessionId={null} onSessionStarted={() => {}} onContinue={onContinue} />
    ));
    expect(screen.queryByPlaceholderText(/^Describe the task/)).toBeNull();
    expect(screen.getByRole("button", { name: "Continue in a new worktree" })).toBeTruthy();
  });

  it("keeps a root session's composer on a Host from before publishing", async () => {
    const t = beforePublishing(new FakeTransport());
    t.handlers["workspace.list"] = () => [root];
    t.handlers["session.list"] = (p) => (p.workspace_id === "root" ? [session("s0", "claude_code")] : []);
    await mount(t, () => <SessionPanel workspaceId="root" sessionId="s0" onSessionStarted={() => {}} />);
    expect(screen.getByPlaceholderText("Send a message")).toBeTruthy();
    expect(screen.queryByText("This session ran in the repository folder.")).toBeNull();
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
    expect(busyMessage({ live_sessions: 1, terminals: 0, session_ids: ["s1"] })).toBe("1 agent running — stop it to remove.");
    expect(busyMessage({ live_sessions: 0, terminals: 2, session_ids: [] })).toBe("2 terminals open — stop them to remove.");
  });
});

describe("diff", () => {
  it("lists changed files with counts and indicators, and shows a file's patch", async () => {
    const t = new FakeTransport();
    t.handlers["git.diff"] = (p) => ({
      base_commit: BASE,
      truncated: false,
      files: [
        {
          path: "src/a.rs",
          change: "modified",
          additions: 2,
          deletions: 1,
          binary: false,
          too_large: false,
          untracked: false,
        },
        {
          path: "new.txt",
          change: "added",
          additions: 1,
          deletions: 0,
          binary: false,
          too_large: false,
          untracked: true,
        },
        {
          path: "logo.png",
          change: "added",
          additions: null,
          deletions: null,
          binary: true,
          too_large: false,
          untracked: true,
        },
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
    expect(screen.getByTestId("diff-patch").textContent).toContain("Binary file — not shown");
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
  it("a new worktree with a task creates it, then starts the session there", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    t.handlers["workspace.create_worktree"] = () => worktree("9f8e7d6c");
    t.handlers["session.start"] = () => ({ session_id: "s9" });
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    fireEvent.click(screen.getByTitle("New worktree"));
    await tick();
    const dialog = screen.getByRole("dialog", { name: "New worktree" });
    fireEvent.change(within(dialog).getByLabelText("What do you want to work on?"), { target: { value: "go" } });
    expect((within(dialog).getByLabelText("Agent") as HTMLSelectElement).value).toBe("claude_code");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await tick();
    expect(
      t.calls.map((c) => c.op).filter((op) => op.startsWith("workspace.create") || op === "session.start"),
    ).toEqual(["workspace.create_worktree", "session.start"]);
    expect(t.calls.find((c) => c.op === "session.start")?.params).toMatchObject({
      workspace_id: "9f8e7d6c",
      agent: "claude_code",
      blocks: [{ type: "text", text: "go" }],
    });
    expect(props.onSelectWorkspace).toHaveBeenCalledWith("9f8e7d6c");
    expect(props.onSessionStarted).toHaveBeenCalledWith("9f8e7d6c", "s9");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("starts from a chosen base branch, in plan mode, with files uploaded into the new worktree", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    t.handlers["git.branches"] = () => ({
      local: ["main", "feature"],
      remote: ["origin/main"],
      default: "origin/main",
    });
    t.handlers["agent.list"] = () => [
      {
        agent: "claude_code",
        name: "Claude Code",
        available: true,
        reason: null,
        support: "supported",
        options: {
          model: { current: "sonnet", values: [{ value: "sonnet", label: "Sonnet", description: null }] },
          effort: null,
          mode: { current: "normal", label: "Manual", available: ["normal", "plan", "unattended"] },
        },
      },
    ];
    t.handlers["workspace.create_worktree"] = () => worktree("9f8e7d6c");
    t.handlers["attachments.upload"] = (p) => ({
      attachment_id: p.name === "shot.png" ? "img00001" : "pdf00001",
      name: p.name,
      size: 4,
      mime_type: "application/octet-stream",
    });
    t.handlers["session.start"] = () => ({ session_id: "s9" });
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    fireEvent.click(screen.getByTitle("New worktree"));
    await tick();
    const dialog = screen.getByRole("dialog", { name: "New worktree" });
    // The remote's default branch is preselected; the mode menu is not offered here.
    expect(within(dialog).getByLabelText("Base branch: origin/main")).toBeTruthy();
    expect(within(dialog).queryByRole("button", { name: /^Mode:/ })).toBeNull();
    fireEvent.click(within(dialog).getByLabelText("Base branch: origin/main"));
    expect(
      within(within(dialog).getByRole("listbox"))
        .getAllByRole("option")
        .map((o) => o.textContent),
    ).toEqual(["origin/maindefault", "main", "feature"]);
    fireEvent.change(within(dialog).getByLabelText("Search branches"), { target: { value: "feat" } });
    fireEvent.keyDown(within(dialog).getByLabelText("Search branches"), { key: "Enter" });
    expect(within(dialog).getByLabelText("Base branch: feature")).toBeTruthy();

    fireEvent.click(within(dialog).getByTitle("More options"));
    fireEvent.click(within(dialog).getByRole("menuitemcheckbox", { name: /Plan mode/ }));
    expect(within(dialog).getByTestId("plan-mode").textContent).toContain("Plan mode");
    const png = new File([new Uint8Array([137, 80, 78, 71])], "shot.png", { type: "image/png" });
    const pdf = new File([new Uint8Array([37, 80, 68, 70])], "spec.pdf", { type: "application/pdf" });
    fireEvent.change(within(dialog).getByTestId("attachment-input"), { target: { files: [png, pdf] } });
    await within(dialog).findByLabelText("Remove spec.pdf");
    // Nothing uploads before there is a worktree to hold it.
    expect(t.calls.some((c) => c.op === "attachments.upload")).toBe(false);

    fireEvent.change(within(dialog).getByLabelText("What do you want to work on?"), { target: { value: "plan it" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(t.calls.some((c) => c.op === "session.start")).toBe(true));
    expect(t.calls.find((c) => c.op === "workspace.create_worktree")?.params).toEqual({
      workspace_id: "root",
      base: "feature",
    });
    expect(t.calls.filter((c) => c.op === "attachments.upload").map((c) => c.params)).toEqual([
      { workspace_id: "9f8e7d6c", name: "shot.png", data: "iVBORw==" },
      { workspace_id: "9f8e7d6c", name: "spec.pdf", data: "JVBERg==" },
    ]);
    expect(t.calls.find((c) => c.op === "session.start")?.params).toMatchObject({
      workspace_id: "9f8e7d6c",
      blocks: [
        { type: "text", text: "plan it" },
        { type: "attachment", attachment_id: "img00001" },
        { type: "attachment", attachment_id: "pdf00001" },
      ],
      options: { model: "sonnet", mode: "plan" },
    });
  });

  it("offers the agent's slash commands in the task box; Escape closes the menu, not the dialog", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    t.handlers["git.branches"] = () => ({ local: ["main"], remote: [], default: "main" });
    t.handlers["agent.list"] = () => [
      {
        agent: "claude_code",
        name: "Claude Code",
        available: true,
        reason: null,
        support: "supported",
        options: null,
        commands: [{ name: "review", description: "Review a pull request", hint: null }],
      },
    ];
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    fireEvent.click(screen.getByTitle("New worktree"));
    await tick();
    const dialog = screen.getByRole("dialog", { name: "New worktree" });
    const box = within(dialog).getByLabelText("What do you want to work on?") as HTMLTextAreaElement;
    fireEvent.change(box, { target: { value: "/re" } });
    expect(within(dialog).getByRole("listbox", { name: "Commands" }).textContent).toContain("/review");
    fireEvent.keyDown(box, { key: "Escape" });
    expect(within(dialog).queryByRole("listbox", { name: "Commands" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "New worktree" })).toBeTruthy();
    fireEvent.change(box, { target: { value: "" } });
    fireEvent.change(box, { target: { value: "/" } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("/review ");
    expect(t.calls.some((c) => c.op === "workspace.create_worktree")).toBe(false);
  });

  it("keeps a created worktree when its session fails to start, and retries only the start", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    t.handlers["workspace.create_worktree"] = () => worktree("9f8e7d6c");
    let fail = true;
    t.handlers["session.start"] = () => {
      if (fail) throw new ZwiimClientError("agent_not_installed", "missing", 400);
      return { session_id: "s9" };
    };
    const props = sidebarProps();
    await mount(t, () => <Sidebar {...props} />);
    fireEvent.click(screen.getByTitle("New worktree"));
    await tick();
    const dialog = screen.getByRole("dialog", { name: "New worktree" });
    fireEvent.change(within(dialog).getByLabelText("What do you want to work on?"), { target: { value: "go" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await tick();
    expect(within(dialog).getByRole("alert").textContent).toContain(
      "The worktree was created, but the session did not start",
    );
    expect((within(dialog).getByLabelText("Repository") as HTMLSelectElement).disabled).toBe(true);
    fail = false;
    fireEvent.click(within(dialog).getByRole("button", { name: "Start session" }));
    await tick();
    expect(t.calls.filter((c) => c.op === "workspace.create_worktree")).toHaveLength(1);
    expect(t.calls.filter((c) => c.op === "session.start")).toHaveLength(2);
    expect(props.onSessionStarted).toHaveBeenCalledWith("9f8e7d6c", "s9");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keeps a removed worktree's session readable but never sendable", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root, worktree("gone0000", { removed: true, branch: null })];
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        {
          turn_id: "t1",
          role: "user",
          blocks: [{ type: "text", text: "old question" }],
          timestamp: "",
          completed_at: null,
          duration_ms: null,
          model: null,
          usage: null,
        },
      ],
    });
    await mount(t, () => <SessionPanel workspaceId="gone0000" sessionId="s3" onSessionStarted={() => {}} />);
    expect(screen.getByTestId("transcript").textContent).toContain("old question");
    const composer = screen.getByPlaceholderText(
      "This worktree was removed — the session is read-only",
    ) as HTMLTextAreaElement;
    expect(composer.disabled).toBe(true);
    expect((screen.getByTitle("Send") as HTMLButtonElement).disabled).toBe(true);
    expect(t.calls.some((c) => c.op === "session.send")).toBe(false);
    cleanup();

    await mount(t, () => <SessionPanel workspaceId="gone0000" sessionId={null} onSessionStarted={() => {}} />);
    expect(screen.getByText(/no new work can start here/)).toBeTruthy();
    expect(screen.queryByPlaceholderText(/^Describe the task/)).toBeNull();
  });

  it("shows a send rejected with workspace_removed", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root, worktree("a1b2c3d4")];
    t.handlers["session.send"] = () => {
      throw new ZwiimClientError("workspace_removed", "The workspace was removed", 410);
    };
    await mount(t, () => <SessionPanel workspaceId="a1b2c3d4" sessionId="s1" onSessionStarted={() => {}} />);
    fireEvent.change(screen.getByPlaceholderText(/^Send a message/), { target: { value: "more" } });
    fireEvent.click(screen.getByTitle("Send"));
    await tick();
    expect(screen.getByRole("alert").textContent).toBe("The workspace was removed");
  });
});
