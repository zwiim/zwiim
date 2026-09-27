import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, ZwiimClientError, type WorkspaceInfo } from "@zwiim/sdk";
import { Workbench } from "@/App";
import { bootHost, type BootPhase } from "@/lib/boot";
import type { DiscoveredHost, NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberSelection } from "@/lib/prefs";
import { SessionPanel } from "@/panels/SessionPanel";
import { FakeTransport, tick } from "./fakes";

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
  function workbenchHost() {
    const t = new FakeTransport();
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
    expect(screen.getByPlaceholderText("Describe the task…")).toBeTruthy();
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
    expect(screen.getByPlaceholderText("Describe the task…")).toBeTruthy();
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
    expect(screen.getByPlaceholderText("Describe the task…")).toBeTruthy();
    expect(tabs().textContent).toContain("old task");
    fireEvent.click(within(tabs()).getByRole("button", { name: "old task" }));
    expect(screen.queryByPlaceholderText("Describe the task…")).toBeNull();
    // Another workspace has its own tabs; coming back returns to the session shown.
    fireEvent.click(screen.getByText("repo-w1"));
    expect(tabs().textContent).not.toContain("old task");
    fireEvent.click(screen.getByText("repo-w2"));
    expect(within(tabs()).getByRole("button", { name: "old task" }).getAttribute("aria-current")).toBe("page");
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
      const send = () => screen.getByPlaceholderText("Describe the task…") as HTMLTextAreaElement;
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
