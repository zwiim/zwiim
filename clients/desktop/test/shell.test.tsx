import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { StrictMode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, CLIENT_REVISION, HostClient, HostController, ZwiimClientError, type WorkspaceInfo } from "@zwiim/sdk";
import { Workbench } from "@/App";
import { bootHost, type BootPhase } from "@/lib/boot";
import type { DiscoveredHost, NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberSelection } from "@/lib/prefs";
import { SessionPanel, agentLabel, defaultAgent, offeredAgents } from "@/panels/SessionPanel";
import { ConnectionStatus } from "@/panels/ConnectionStatus";
import { UpdateCard } from "@/panels/UpdateCard";
import pkg from "../package.json";
import { FakeTransport, hostHello, tick } from "./fakes";

const NO_UPDATES = { desktop: null, host: null, setDesktop: () => {}, hint: false, checks: 0, check: async () => false };

afterEach(cleanup);

const host: DiscoveredHost = { host_id: "h1", url: "http://127.0.0.1:1", token: "t", pid: 7, running: true };

function bridge(overrides: Partial<NativeBridge> = {}): NativeBridge {
  return {
    discover: vi.fn(async () => host),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: false, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u1", email: "me@example.com" })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
    ...overrides,
  };
}

const ws = (id: string): WorkspaceInfo => ({
  workspace_id: id,
  name: `repo-${id}`,
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

async function mount(t: FakeTransport, ui: React.ReactNode) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(<HostProvider value={controller}>{ui}</HostProvider>);
  await tick();
  return controller;
}

describe("compatibility", () => {
  it("introduces this app, and blocks a host that needs a newer one", async () => {
    const t = new FakeTransport();
    const hellos: unknown[] = [];
    t.handlers["host.hello"] = (p: unknown) => {
      hellos.push(p);
      return hostHello({ revision: CLIENT_REVISION + 1, min_client_revision: CLIENT_REVISION + 1 });
    };
    const c = (await bootHost(bridge(), () => {}, { makeTransport: () => t }))!;
    await tick();
    expect(hellos.at(-1)).toMatchObject({ client: { app: "desktop", version: pkg.version, revision: CLIENT_REVISION } });
    expect(c.client.state).toMatchObject({ status: "incompatible", compat: { update: "client" } });
    render(
      <HostProvider value={c}>
        <ConnectionStatus />
        <UpdateCard bridge={bridge()} updates={NO_UPDATES} remote={false} onSeeChanges={() => {}} />
      </HostProvider>,
    );
    // The sidebar's status stays quiet; the update card says what to update.
    expect(screen.queryByTestId("connection-status")).toBeNull();
    expect(screen.getByTestId("update-card").textContent).toContain("App update required");
    expect(screen.getByTestId("update-detail").textContent).toBe(
      "This host needs a newer version of Zwiim. Update this app to continue.",
    );
    await expect(c.client.request("workspace.list", {})).rejects.toMatchObject({ code: "update_required" });
    c.dispose();
  });

  it("names an older local host as the side to restart", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () => hostHello({ revision: 0 });
    const c = (await bootHost(bridge(), () => {}, { makeTransport: () => t }))!;
    await tick();
    render(
      <HostProvider value={c}>
        <ConnectionStatus />
        <UpdateCard bridge={bridge()} updates={NO_UPDATES} remote={false} onSeeChanges={() => {}} />
      </HostProvider>,
    );
    expect(screen.queryByTestId("connection-status")).toBeNull();
    expect(screen.getByTestId("update-card").textContent).toContain("Host update required");
    expect(screen.getByTestId("update-card").textContent).toContain("older than this app");
    c.dispose();
  });
});

describe("boot with a corrupt discovery file", () => {
  it("fails with the problem instead of waiting forever", async () => {
    const b = bridge({
      discover: vi.fn(async () => {
        throw new Error("Discovery file is corrupt");
      }),
    });
    const phases: BootPhase[] = [];
    const c = await bootHost(b, (p) => phases.push(p), { pollMs: 1, startTimeoutMs: 20 });
    expect(c).toBeNull();
    expect(b.startHost).toHaveBeenCalledTimes(1);
    expect(phases.at(-1)).toEqual({
      phase: "failed",
      message: "The host did not start in time. Discovery file is corrupt",
    });
  });

  it("connects once a started host rewrites the file", async () => {
    let n = 0;
    const b = bridge({
      discover: vi.fn(async () => {
        if (n++ === 0) throw new Error("Discovery file is corrupt");
        return host;
      }),
    });
    const phases: BootPhase["phase"][] = [];
    const c = await bootHost(b, (p) => phases.push(p.phase), { pollMs: 1, makeTransport: () => new FakeTransport() });
    expect(phases).toEqual(["discovering", "starting", "ready"]);
    c?.dispose();
  });
});

describe("agent picker", () => {
  it("offers only usable agents and labels the others", async () => {
    const t = new FakeTransport();
    const c = await mount(t, <SessionPanel workspaceId="w1" sessionId={null} onSessionStarted={() => {}} />);
    const select = screen.getByRole("combobox") as HTMLSelectElement;
    expect(select.value).toBe("claude_code");
    const codex = [...select.options].find((o) => o.value === "codex")!;
    expect(codex.disabled).toBe(true);
    expect(codex.textContent).toBe("Codex — not installed");
    c.dispose();
  });

  it("says so when no agent can run, and checks again on request", async () => {
    const t = new FakeTransport();
    t.handlers["agent.list"] = () => [
      { agent: "codex", name: "Codex", available: false, reason: "not_installed", options: null },
    ];
    const c = await mount(t, <SessionPanel workspaceId="w1" sessionId={null} onSessionStarted={() => {}} />);
    expect(screen.getByTestId("no-agents")).toBeTruthy();
    expect(screen.queryByRole("combobox")).toBeNull();
    const before = t.calls.filter((x) => x.op === "agent.list").length;
    fireEvent.click(screen.getByText(/check again/i));
    await tick();
    expect(t.calls.filter((x) => x.op === "agent.list").length).toBe(before + 1);
    c.dispose();
  });
});

describe("workbench", () => {
  /** Root workspaces a click selects: a Host from before publishing (a root's click opens a worktree since, in publish.test). */
  function workbenchHost() {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "workspace.publish") });
    t.handlers["workspace.list"] = () => [ws("w1"), ws("w2")];
    t.handlers["session.list"] = (p) =>
      p.workspace_id === "w2"
        ? [
            {
              session_id: "s9",
              workspace_id: null,
              title: "old task",
              agent: "claude_code",
              status: "pending_review",
              live: false,
              created_at: "",
              updated_at: "",
            },
          ]
        : [];
    t.handlers["workspace.tree"] = () => [{ name: "a.txt", path: "a.txt", kind: "file" }];
    t.handlers["files.read"] = (p) => ({
      path: p.path,
      content: "hello from a",
      etag: "e",
      truncated: false,
      binary: false,
      size: 12,
    });
    return t;
  }

  it("shows the session, workspace views and terminal side by side", async () => {
    const t = workbenchHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    const panel = screen.getByTestId("workspace-panel");
    expect(screen.getByPlaceholderText(/^Describe the task/)).toBeTruthy();
    expect(panel.querySelector('[aria-label="Terminals"]')).toBeTruthy();
    expect(screen.getByTestId("file-tree")).toBeTruthy();
    c.dispose();
  });

  it("keeps panels mounted across tabs", async () => {
    const t = workbenchHost();
    t.handlers["git.status"] = () => [];
    t.handlers["git.log"] = () => ({ entries: [] });
    const c = await mount(t, <Workbench bridge={bridge()} />);
    fireEvent.click(screen.getByText("a.txt"));
    await tick();
    expect(screen.getByText("hello from a")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Git" }));
    await tick();
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    await tick();
    expect(screen.getByText("hello from a")).toBeTruthy();
    expect(t.calls.filter((x) => x.op === "workspace.tree").length).toBe(1);
    c.dispose();
  });

  it("opens a changed file as a tab beside the session and closes it back to the session", async () => {
    const t = workbenchHost();
    t.handlers["git.diff"] = (p) => ({
      base_commit: null,
      truncated: false,
      files: [
        {
          path: "src/a.txt",
          change: "modified",
          additions: 1,
          deletions: 0,
          binary: false,
          too_large: false,
          untracked: false,
        },
      ],
      ...(p.file ? { patch: "@@ -1 +1 @@\n-old\n+new" } : {}),
    });
    const c = await mount(t, <Workbench bridge={bridge()} />);
    fireEvent.click(screen.getByRole("button", { name: "Diff" }));
    await tick();
    fireEvent.click(screen.getByText("src/a.txt"));
    await tick();
    const tabs = screen.getByRole("navigation", { name: "Open tabs" });
    expect(tabs.textContent).toContain("New session");
    expect(tabs.textContent).toContain("a.txt");
    expect(screen.getByTestId("diff-patch").textContent).toContain("+new");
    expect(screen.getByTestId("diff-view").parentElement!.getAttribute("data-active")).toBe("true");
    // Opening it again reuses the tab.
    fireEvent.click(within(screen.getByTestId("diff-files")).getByText("src/a.txt"));
    expect(screen.getAllByLabelText("Close a.txt")).toHaveLength(1);
    fireEvent.click(screen.getByLabelText("Close a.txt"));
    expect(screen.queryByTestId("diff-view")).toBeNull();
    expect(screen.getByRole("navigation", { name: "Open tabs" }).textContent).not.toContain("a.txt");
    expect(screen.getByPlaceholderText(/^Describe the task/)).toBeTruthy();
    c.dispose();
  });

  it("opens a file linked in the conversation as a tab beside it", async () => {
    const t = workbenchHost();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [
        {
          turn_id: "t1",
          role: "assistant",
          blocks: [{ type: "text", text: "Wrote [a.txt](a.txt)." }],
          timestamp: "",
          completed_at: null,
          duration_ms: null,
          model: null,
          usage: null,
        },
      ],
    });
    const c = await mount(t, <Workbench bridge={bridge()} />);
    fireEvent.click(screen.getByText("repo-w2"));
    await tick();
    fireEvent.click(screen.getByRole("link", { name: "a.txt" }));
    await tick();
    expect(screen.getByRole("navigation", { name: "Open tabs" }).textContent).toContain("a.txt");
    expect(screen.getByText("hello from a")).toBeTruthy();
    expect(t.calls.find((x) => x.op === "files.read")?.params).toEqual({ workspace_id: "w2", path: "a.txt" });
    c.dispose();
  });

  it("shows a workspace's sessions as tabs, and + starts another there", async () => {
    const t = workbenchHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    const tabs = () => screen.getByRole("navigation", { name: "Open tabs" });
    fireEvent.click(screen.getByText("repo-w2"));
    await tick();
    // A workspace opens at its newest session.
    expect(within(tabs()).getByRole("button", { name: "old task" }).getAttribute("aria-current")).toBe("page");
    expect(t.sent).toContainEqual(expect.objectContaining({ op: "session.subscribe", session_id: "s9" }));
    fireEvent.click(within(tabs()).getByRole("button", { name: "New session" }));
    expect(screen.getByPlaceholderText(/^Describe the task/)).toBeTruthy();
    expect(tabs().textContent).toContain("old task");
    fireEvent.click(within(tabs()).getByRole("button", { name: "old task" }));
    expect(screen.queryByPlaceholderText(/^Describe the task/)).toBeNull();
    // Another workspace has its own tabs; coming back returns to the session shown.
    fireEvent.click(screen.getByText("repo-w1"));
    expect(tabs().textContent).not.toContain("old task");
    fireEvent.click(screen.getByText("repo-w2"));
    expect(within(tabs()).getByRole("button", { name: "old task" }).getAttribute("aria-current")).toBe("page");
    c.dispose();
  });

  it("keeps a root's sessions writable on a Host from before publishing", async () => {
    const t = workbenchHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    fireEvent.click(screen.getByText("repo-w2"));
    await tick();
    expect(screen.getByPlaceholderText("Send a message")).toBeTruthy();
    expect(screen.queryByTestId("root-banner")).toBeNull();
    expect(screen.queryByText("This session ran in the repository folder.")).toBeNull();
    expect(screen.getByTestId("sidebar").textContent).not.toContain("earlier session");
    expect(screen.getByTestId("workspace-panel")).toBeTruthy();
    c.dispose();
  });

  it("deletes a session from its tab, then shows the tab beside it", async () => {
    const t = workbenchHost();
    const session = (session_id: string, title: string) => ({
      session_id,
      workspace_id: null,
      title,
      agent: "claude_code",
      status: "pending_review",
      live: false,
      created_at: "",
      updated_at: "",
    });
    t.handlers["session.list"] = (p) =>
      p.workspace_id === "w2" ? [session("s10", "new task"), session("s9", "old task")] : [];
    let busy = true;
    t.handlers["session.delete"] = () => {
      if (busy) throw new ZwiimClientError("turn_in_progress", "working", 409);
      return {};
    };
    const c = await mount(t, <Workbench bridge={bridge()} />);
    const tabs = () => screen.getByRole("navigation", { name: "Open tabs" });
    fireEvent.click(screen.getByText("repo-w2"));
    await tick();
    expect(within(tabs()).getByRole("button", { name: "new task" }).getAttribute("aria-current")).toBe("page");

    // Cancel leaves it; a working agent refuses, with nothing removed.
    fireEvent.click(within(tabs()).getByRole("button", { name: "Delete new task" }));
    fireEvent.click(within(screen.getByRole("alertdialog", { name: "Delete session" })).getByText("Cancel"));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    fireEvent.click(within(tabs()).getByRole("button", { name: "Delete new task" }));
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    await tick();
    expect(screen.getByRole("alertdialog").textContent).toContain("The agent is still working");
    expect(within(tabs()).getByRole("button", { name: "new task" })).toBeTruthy();

    busy = false;
    fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete" }));
    await tick();
    await t.feedEvent("session.removed", { session_id: "s10", workspace_id: "w2" });
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(within(tabs()).queryByRole("button", { name: "new task" })).toBeNull();
    expect(within(tabs()).getByRole("button", { name: "old task" }).getAttribute("aria-current")).toBe("page");
    expect(t.calls.filter((x) => x.op === "session.delete").map((x) => x.params)).toEqual([
      { session_id: "s10" },
      { session_id: "s10" },
    ]);
    c.dispose();
  });

  it("collapses the workspace panel without losing its state, and remembers it", async () => {
    const t = workbenchHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    fireEvent.click(screen.getByText("a.txt"));
    await tick();
    fireEvent.click(screen.getByTitle("Hide panel"));
    expect(screen.getByTestId("workspace-panel").getAttribute("aria-hidden")).toBe("true");
    expect(JSON.parse(window.localStorage.getItem("zwiim.desktop.panel")!).collapsed).toBe(true);
    fireEvent.click(screen.getByTitle("Show panel"));
    expect(screen.getByText("hello from a")).toBeTruthy();
    fireEvent.click(screen.getByTitle("Hide terminal"));
    expect(JSON.parse(window.localStorage.getItem("zwiim.desktop.panel")!).terminalCollapsed).toBe(true);
    expect(screen.getByTitle("Show terminal")).toBeTruthy();
    fireEvent.click(screen.getByTitle("Show terminal"));
    expect(t.calls.filter((x) => x.op === "workspace.tree").length).toBe(1);
    c.dispose();
  });

  it("restores the last workspace and session, ignoring ones the host no longer has", async () => {
    rememberSelection("h1", { workspaceId: "w2", sessionId: "s9" });
    const t = workbenchHost();
    let c = await mount(t, <Workbench bridge={bridge()} />);
    expect(t.sent).toContainEqual(expect.objectContaining({ op: "session.subscribe", session_id: "s9" }));
    c.dispose();
    cleanup();

    rememberSelection("h1", { workspaceId: "gone", sessionId: "s9" });
    const t2 = workbenchHost();
    c = await mount(t2, <Workbench bridge={bridge()} />);
    expect(t2.sent.some((m) => m.op === "session.subscribe")).toBe(false);
    expect(JSON.parse(window.localStorage.getItem("zwiim.desktop.last.h1")!)).toEqual({
      workspaceId: "w1",
      sessionId: null,
    });
    c.dispose();
  });

  it("collapses the sidebar and remembers it", async () => {
    const t = workbenchHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    fireEvent.click(screen.getByTitle("Hide sidebar"));
    expect(screen.queryByTestId("sidebar")).toBeNull();
    expect(JSON.parse(window.localStorage.getItem("zwiim.desktop.sidebar")!).collapsed).toBe(true);
    fireEvent.click(screen.getByTitle("Show sidebar"));
    expect(screen.getByTestId("sidebar")).toBeTruthy();
    c.dispose();
  });

  it("disables changes while the host is unreachable", async () => {
    vi.useFakeTimers();
    try {
      const t = workbenchHost();
      const c = await mount(t, <Workbench bridge={bridge()} />);
      expect(screen.queryByTestId("offline-banner")).toBeNull();
      const send = () => screen.getByPlaceholderText(/^Describe the task/) as HTMLTextAreaElement;
      expect(send().disabled).toBe(false);
      await act(async () => t.handlersForEvents!.onClose({ code: 1006, reason: "" }));
      await tick();
      expect(screen.getByTestId("offline-banner").textContent).toContain("Changes are disabled");
      expect(send().disabled).toBe(true);
      expect((screen.getByTitle("Add workspace") as HTMLButtonElement).disabled).toBe(true);
      c.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("start on a current Host", () => {
  const worktree = (id: string, root: string): WorkspaceInfo => ({
    ...ws(id),
    kind: "worktree",
    parent_workspace_id: root,
    branch: `zwiim/${id}`,
  });
  const session = (id: string, updated_at: string) => ({
    session_id: id,
    workspace_id: null,
    title: `task ${id}`,
    agent: "claude_code",
    status: "pending_review",
    live: false,
    created_at: "",
    updated_at,
  });

  /**
   * A Host with publishing (every capability), by default root r1 with no
   * worktree and root r2 with worktrees t1 and t2 (t2's session prompted last).
   */
  function currentHost(workspaces: WorkspaceInfo[] = [ws("r1"), ws("r2"), worktree("t1", "r2"), worktree("t2", "r2")]) {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => workspaces;
    t.handlers["session.list"] = (p) =>
      p.workspace_id === "t1"
        ? [session("s1", "2026-10-01T00:00:00Z")]
        : p.workspace_id === "t2"
          ? [session("s2", "2026-10-02T00:00:00Z")]
          : p.workspace_id === "r2"
            ? [session("s0", "2026-10-03T00:00:00Z")]
            : [];
    t.handlers["workspace.tree"] = () => [];
    t.handlers["git.status"] = () => ({ branch: null, upstream: null, ahead: 0, behind: 0, files: [] });
    return t;
  }

  const shown = () => JSON.parse(window.localStorage.getItem("zwiim.desktop.last.h1")!);
  const selected = () => screen.queryAllByRole("button", { current: true }).map((b) => b.textContent);
  /** Nothing the start does creates a worktree or reads a root to make one. */
  const created = (t: FakeTransport) =>
    t.calls.filter((x) => x.op === "workspace.create_worktree" || x.op === "workspace.inspect");

  it("replaces a remembered root by its open target worktree", async () => {
    rememberSelection("h1", { workspaceId: "r2", sessionId: "s0" });
    const t = currentHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    expect(shown()).toEqual({ workspaceId: "t2", sessionId: "s2" });
    expect(selected()).toEqual(["zwiim/t2"]);
    expect(created(t)).toEqual([]);
    c.dispose();
  });

  it("shows nothing for a remembered root without a worktree", async () => {
    rememberSelection("h1", { workspaceId: "r1", sessionId: null });
    const t = currentHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    expect(shown()).toEqual({ workspaceId: null, sessionId: null });
    expect(selected()).toEqual([]);
    expect(screen.getByText("Pick a repository to open a worktree.")).toBeTruthy();
    expect(screen.queryByText("No workspace yet")).toBeNull();
    // Another client's change does not pick one either.
    await t.feedEvent("workspace.changed", { workspace: ws("r3") });
    expect(selected()).toEqual([]);
    expect(created(t)).toEqual([]);
    c.dispose();
  });

  it("with nothing remembered, opens the first root that has a worktree, never a root", async () => {
    const t = currentHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    expect(shown()).toEqual({ workspaceId: "t2", sessionId: "s2" });
    expect(selected()).toEqual(["zwiim/t2"]);
    expect(created(t)).toEqual([]);
    c.dispose();
  });

  it("restores a remembered worktree and session as before", async () => {
    rememberSelection("h1", { workspaceId: "t1", sessionId: "s1" });
    const t = currentHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    expect(shown()).toEqual({ workspaceId: "t1", sessionId: "s1" });
    expect(t.sent).toContainEqual(expect.objectContaining({ op: "session.subscribe", session_id: "s1" }));
    c.dispose();
  });

  it("shows the empty state when no root has a worktree, creating none", async () => {
    const t = currentHost([ws("r1"), { ...ws("p1"), is_git_repo: false, branch: null }]);
    const c = await mount(t, <Workbench bridge={bridge()} />);
    expect(shown()).toEqual({ workspaceId: null, sessionId: null });
    expect(selected()).toEqual([]);
    // DESIGN.md's empty state: a headline, then one line.
    const body = screen.getByText("Pick a repository to open a worktree.");
    const headline = screen.getByText("No worktree open");
    expect(headline.nextElementSibling).toBe(body);
    expect(headline.className).toContain("font-semibold");
    expect(created(t)).toEqual([]);
    c.dispose();
  });

  it("with no workspaces at all, keeps the add-a-folder empty state", async () => {
    const t = currentHost([]);
    const c = await mount(t, <Workbench bridge={bridge()} />);
    expect(screen.getByText("No workspace yet")).toBeTruthy();
    expect(screen.queryByText("Pick a repository to open a worktree.")).toBeNull();
    c.dispose();
  });

  it("opens the next root's worktree when the shown one leaves the host", async () => {
    rememberSelection("h1", { workspaceId: "t1", sessionId: "s1" });
    const t = currentHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    await t.feedEvent("workspace.removed", { workspace_id: "t1" });
    await tick();
    expect(shown()).toEqual({ workspaceId: "t2", sessionId: "s2" });
    await t.feedEvent("workspace.removed", { workspace_id: "t2" });
    await tick();
    expect(shown()).toEqual({ workspaceId: null, sessionId: null });
    expect(selected()).toEqual([]);
    expect(screen.getByText("Pick a repository to open a worktree.")).toBeTruthy();
    c.dispose();
  });

  describe("sessions that ran in the repository folder", () => {
    const tabs = () => screen.getByRole("navigation", { name: "Open tabs" });
    const continueButton = () => screen.getByRole("button", { name: "Continue in a new worktree" });
    const inspection = (git: "none" | "no_origin" | "origin") => ({
      git,
      branch: "main",
      suggested_name: "repo-r2",
      first_commit: git === "origin" ? null : { files: 3, bytes: 300, at_least: false, large: false },
      github: { available: true, login: "me", owners: [{ login: "me", kind: "user" }] },
    });
    /** `currentHost`, reading r2 as `git` and creating worktree t3 for it. */
    function continueHost(git: "none" | "no_origin" | "origin") {
      const t = currentHost();
      t.handlers["workspace.inspect"] = () => inspection(git);
      t.handlers["workspace.create_worktree"] = () => worktree("t3", "r2");
      t.handlers["github.repo_available"] = () => ({ available: true });
      t.handlers["workspace.add"] = () => ws("r2");
      return t;
    }
    /** Open r2's earlier sessions from its sidebar row. */
    async function openEarlier() {
      fireEvent.click(screen.getByRole("button", { name: "1 earlier session" }));
      await tick();
    }

    it("opens read-only at the root's newest session: a banner, no new session, no Files or Terminal", async () => {
      const t = continueHost("origin");
      const c = await mount(t, <Workbench bridge={bridge()} />);
      expect(screen.queryByTestId("root-banner")).toBeNull();
      await openEarlier();
      expect(shown()).toEqual({ workspaceId: "r2", sessionId: "s0" });
      expect(screen.getByTestId("root-banner").textContent).toBe(
        "These sessions ran in the repository folder — they're read-only.",
      );
      expect(within(tabs()).getByRole("button", { name: "task s0" }).getAttribute("aria-current")).toBe("page");
      expect(within(tabs()).queryByRole("button", { name: "New session" })).toBeNull();
      expect(screen.queryByTestId("workspace-panel")).toBeNull();
      expect(screen.getByText("This session ran in the repository folder.")).toBeTruthy();
      expect(screen.queryByPlaceholderText("Send a message")).toBeNull();
      // Nothing was read or created just to show them.
      expect(created(t)).toEqual([]);
      c.dispose();
    });

    it("continues in a new worktree beside the existing ones, with an empty composer", async () => {
      const t = continueHost("origin");
      const c = await mount(t, <Workbench bridge={bridge()} />);
      await openEarlier();
      fireEvent.click(continueButton());
      await tick();
      expect(created(t).map((x) => [x.op, x.params])).toEqual([
        ["workspace.inspect", { workspace_id: "r2" }],
        ["workspace.create_worktree", { workspace_id: "r2" }],
      ]);
      expect(shown()).toEqual({ workspaceId: "t3", sessionId: null });
      expect(screen.getByPlaceholderText(/^Describe the task/)).toBeTruthy();
      expect(screen.queryByTestId("root-banner")).toBeNull();
      expect(screen.getByTestId("workspace-panel")).toBeTruthy();
      expect(t.calls.some((x) => x.op === "session.start")).toBe(false);
      c.dispose();
    });

    it("asks before publishing a plain or unborn repository, then continues in the new worktree", async () => {
      for (const git of ["none", "no_origin"] as const) {
        const t = continueHost(git);
        const c = await mount(t, <Workbench bridge={bridge()} />);
        await openEarlier();
        fireEvent.click(continueButton());
        await tick();
        expect(screen.getByRole("dialog", { name: "Create a private GitHub repo?" })).toBeTruthy();
        expect(t.calls.some((x) => x.op === "workspace.create_worktree")).toBe(false);
        // The name check fires 300 ms after the dialog opens.
        await act(() => new Promise((resolve) => setTimeout(resolve, 350)));
        await tick();
        fireEvent.click(screen.getByRole("button", { name: /create repo/i }));
        await tick();
        expect(t.calls.filter((x) => x.op === "workspace.add").map((x) => x.params)).toEqual([
          { workspace_id: "r2", publish: { owner: "me", name: "repo-r2" } },
        ]);
        expect(t.calls.filter((x) => x.op === "workspace.create_worktree").map((x) => x.params)).toEqual([
          { workspace_id: "r2" },
        ]);
        expect(shown()).toEqual({ workspaceId: "t3", sessionId: null });
        c.dispose();
        cleanup();
      }
    });

    it("shows the sidebar for a Continue made while it was hidden", async () => {
      const t = continueHost("origin");
      const c = await mount(t, <Workbench bridge={bridge()} />);
      await openEarlier();
      fireEvent.click(screen.getByTitle("Hide sidebar"));
      fireEvent.click(continueButton());
      await tick();
      expect(screen.getByTestId("sidebar")).toBeTruthy();
      expect(shown()).toEqual({ workspaceId: "t3", sessionId: null });
      c.dispose();
    });

    it("makes one worktree per Continue, even with effects run twice", async () => {
      const t = continueHost("origin");
      let n = 3;
      t.handlers["workspace.create_worktree"] = () => worktree(`t${n++}`, "r2");
      const c = await mount(t, <StrictMode><Workbench bridge={bridge()} /></StrictMode>);
      const continued = () => created(t).filter((x) => x.params.workspace_id === "r2").map((x) => x.op);
      await openEarlier();
      fireEvent.click(continueButton());
      await tick();
      expect(continued()).toEqual(["workspace.inspect", "workspace.create_worktree"]);
      expect(shown()).toEqual({ workspaceId: "t3", sessionId: null });
      // From a hidden sidebar, which mounts holding the request.
      await openEarlier();
      fireEvent.click(screen.getByTitle("Hide sidebar"));
      fireEvent.click(continueButton());
      await tick();
      expect(continued()).toEqual([
        "workspace.inspect",
        "workspace.create_worktree",
        "workspace.inspect",
        "workspace.create_worktree",
      ]);
      expect(shown()).toEqual({ workspaceId: "t4", sessionId: null });
      c.dispose();
    });
  });
});

describe("agents Zwiim does not support yet", () => {
  const agent = (id: string, support: string, available = true, reason: string | null = null) =>
    ({ agent: id, name: id, available, reason, support, adapter: null, options: null, effort_by_model: {}, commands: null }) as const;

  it("are labelled experimental and never preselected over a supported agent", () => {
    const list = [agent("gemini", "experimental"), agent("codex", "supported", false, "not_installed"), agent("claude_code", "supported")];
    expect(agentLabel(list[0])).toBe("gemini — experimental");
    expect(agentLabel(list[1])).toBe("codex — not installed");
    expect(agentLabel(list[2])).toBe("claude_code");
    expect(defaultAgent([...list], null)).toBe("claude_code");
    expect(defaultAgent([...list], "gemini")).toBe("gemini");
    expect(defaultAgent([list[0]], null)).toBe("gemini");
  });

  it("are not offered for a new session: only Claude Code and Codex are", () => {
    const list = [agent("gemini", "experimental"), agent("codex", "supported", false, "not_installed"), agent("claude_code", "supported")];
    expect(offeredAgents(list).map((a) => a.agent)).toEqual(["codex", "claude_code"]);
  });
});
