import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  ZwiimClientError,
  type SessionSummary,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { MONO_FONTS } from "@/lib/appearance";
import { HostProvider } from "@/lib/host-context";
import { rememberFonts } from "@/lib/prefs";
import { DiffPanel, DiffView } from "@/panels/DiffPanel";
import { Sidebar, type SidebarProps } from "@/panels/Sidebar";
import { TerminalPanel } from "@/panels/TerminalPanel";
import { OpenSettingsContext } from "@/settings/open";
import { FakeTransport, hostHello, tick } from "./fakes";

const links = vi.hoisted(() => ({ openLink: vi.fn(async (_href: string) => {}) }));
vi.mock("@/lib/bridge", async (o) => ({ ...(await o<typeof import("@/lib/bridge")>()), openLink: links.openLink }));

// jsdom cannot render xterm; the panel's terminal list is what is under test.
const xterms = vi.hoisted(() => [] as { options: Record<string, unknown> }[]);
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    options: Record<string, unknown>;
    constructor(options: Record<string, unknown>) {
      this.options = { ...options };
      xterms.push(this);
    }
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
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    fit() {}
  },
}));
globalThis.ResizeObserver ??= class {
  observe() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  links.openLink.mockClear();
  xterms.length = 0;
  window.localStorage.clear();
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
  base_commit: "3f9e1a2".padEnd(40, "b"),
  removed: false,
  is_git_repo: true,
  display_path: null,
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
  onSelectWorkspace: vi.fn(),
  onSessionStarted: vi.fn(),
  pickFolder: vi.fn(async () => null),
  remote: false,
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

  it("shows another client's worktree and its working count", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root];
    await mount(t, <Sidebar {...sidebarProps()} />);
    expect(screen.queryByTestId("workspace-a1b2c3d4")).toBeNull();

    await t.feedEvent("workspace.changed", { workspace: worktree("a1b2c3d4") });
    await t.feedEvent("session.changed", { session: session("s1", "a1b2c3d4") });
    const row = () => screen.getByTestId("workspace-a1b2c3d4").textContent!;
    expect(row()).toContain("zwiim/a1b2c3d4");
    expect(row()).not.toContain("working ");

    await t.feedEvent("session.changed", { session: session("s1", "a1b2c3d4", { live: true, status: "in_progress" }) });
    await t.feedEvent("session.changed", { session: session("s2", "a1b2c3d4", { live: true, status: "in_progress" }) });
    expect(row()).toContain("2 working");
    await t.feedEvent("session.changed", {
      session: session("s1", "a1b2c3d4", { live: false, status: "pending_review" }),
    });
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
    expect(screen.queryByTestId("workspace-a1b2c3d4")).toBeNull();
    fireEvent.click(screen.getByText("1 archived"));
    const gone = screen.getByTestId("workspace-a1b2c3d4");
    expect(gone.textContent).toContain("repo-a1b2c3d4");
    expect(gone.querySelector('[title="Archive worktree"]')).toBeNull();
    expect(screen.queryByTestId("workspace-b2c3d4e5")).toBeNull();
  });
});

describe("diff on the host feed", () => {
  it("refetches the open workspace's diff and file patch on workspace.diff_changed", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.list"] = () => [root, worktree("a1b2c3d4")];
    let files = ["a.txt"];
    let version = 1;
    t.handlers["git.diff"] = (p) => {
      if (p.file && !files.includes(p.file)) throw new ZwiimClientError("not_found", "File has no changes", 404);
      return {
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
        ...(p.file ? { patch: `@@ -1 +1 @@\n-v0\n+v${version}\n` } : {}),
      };
    };
    t.handlers["files.read"] = (p) => ({ path: p.path, content: `v${version}\n`, etag: "e", mtime_ms: 0, readonly: false, line_ending: "lf" });
    const ws = worktree("a1b2c3d4");
    await mount(
      t,
      <>
        <DiffPanel workspace={ws} selected="a.txt" onOpen={() => {}} />
        <DiffView workspace={ws} path="a.txt" />
      </>,
    );
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
    const refetched = t.calls
      .filter((c) => c.op === "git.diff")
      .slice(before)
      .map((c) => c.params);
    expect(refetched).toHaveLength(2);
    expect(refetched).toContainEqual({ workspace_id: "a1b2c3d4" });
    expect(refetched).toContainEqual({ workspace_id: "a1b2c3d4", file: "a.txt" });

    // The open file was committed away: its tab shows it unmarked instead of vanishing.
    files = ["b.txt"];
    await t.feedEvent("workspace.diff_changed", { workspace_id: "a1b2c3d4" });
    await tick();
    expect(screen.getByTestId("diff-files").textContent).not.toContain("a.txt");
    expect(screen.getByTestId("diff-patch").textContent).toContain("No uncommitted changes");
    expect(screen.queryByRole("alert")).toBeNull();
  });
});

describe("terminals on the host feed", () => {
  it("shows terminals another client opens and removes those that end", async () => {
    const t = new FakeTransport();
    t.handlers["terminal.snapshot"] = (p) => ({ terminal_id: p.terminal_id, alive: true, data: "", seq: 0 });
    await mount(t, <TerminalPanel workspaceId="w1" />);
    expect(screen.getByText("No terminals in this workspace.")).toBeTruthy();
    expect(screen.getByText("New terminal").tagName).toBe("BUTTON");

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

  it("draws in the fonts chosen in Settings, and redraws when they change", async () => {
    const t = new FakeTransport();
    t.handlers["terminal.snapshot"] = (p) => ({ terminal_id: p.terminal_id, alive: true, data: "", seq: 0 });
    await mount(t, <TerminalPanel workspaceId="w1" />);
    await t.feedEvent("terminal.changed", { terminal: { terminal_id: "t1", title: "zsh", workspace_id: "w1" } });
    await tick();
    const term = xterms.at(-1)!;
    expect(term.options.fontSize).toBe(12);
    expect(term.options.fontFamily).toBe(MONO_FONTS);

    act(() => rememberFonts({ code: "Fira Code" }));
    expect(term.options.fontFamily).toBe(`"Fira Code", ${MONO_FONTS}`);
    act(() => rememberFonts({ terminal: "Menlo", terminalSize: 15 }));
    expect(term.options.fontFamily).toBe(`"Menlo", ${MONO_FONTS}`);
    expect(term.options.fontSize).toBe(15);
  });

  it("leaves out terminals shown elsewhere", async () => {
    const t = new FakeTransport();
    t.handlers["terminal.snapshot"] = (p) => ({ terminal_id: p.terminal_id, alive: true, data: "", seq: 0 });
    await mount(t, <TerminalPanel workspaceId="w1" hidden={new Set(["t2"])} />);
    await t.feedEvent("terminal.changed", { terminal: { terminal_id: "t1", title: "zsh", workspace_id: "w1" } });
    await t.feedEvent("terminal.changed", { terminal: { terminal_id: "t2", title: "codex", workspace_id: "w1" } });
    await tick();
    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual(["zsh"]);
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

  it("runs, stops and re-runs a worktree's scripts", async () => {
    const t = new FakeTransport();
    t.handlers["terminal.snapshot"] = (p) => ({ terminal_id: p.terminal_id, alive: true, data: "", seq: 0 });
    t.handlers["terminal.kill"] = () => ({});
    const live: { terminal_id: string; title: string; workspace_id: string }[] = [];
    t.handlers["terminal.list"] = () => live;
    let n = 0;
    t.handlers["workspace.run_script"] = (p: { script: "setup" | "run" }) => {
      const title = p.script === "run" ? "Run" : "Setup";
      const kept = live.filter((x) => x.title !== title);
      live.splice(0, live.length, ...kept, { terminal_id: `r${++n}`, title, workspace_id: "w1" });
      return { terminal_id: `r${n}` };
    };
    const opened = vi.fn();
    const started = vi.fn();
    const c = await mount(
      t,
      <OpenSettingsContext.Provider value={opened}>
        <TerminalPanel workspaceId="w1" onStarted={started} />
      </OpenSettingsContext.Provider>,
    );
    act(() => c.store.dispatch({ type: "workspaces", workspaces: [root, worktree("w1")] }));

    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    await tick();
    expect(screen.getByRole("tab", { name: "Run", selected: true })).toBeTruthy();
    expect(started).toHaveBeenCalledTimes(1);
    // Running: Stop kills it, without asking.
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await tick();
    expect(t.calls.filter((x) => x.op === "terminal.kill").map((x) => x.params)).toEqual([{ terminal_id: "r1" }]);
    expect(screen.queryByTestId("confirm-kill")).toBeNull();

    fireEvent.click(screen.getByTitle("Run setup again"));
    await tick();
    fireEvent.click(screen.getByTitle("Run setup again"));
    await tick();
    // Again replaces the last: one Setup tab, the newest.
    expect(screen.getAllByRole("tab", { name: "Setup" })).toHaveLength(1);
    expect(screen.getByRole("tab", { name: "Setup", selected: true })).toBeTruthy();
    expect(t.calls.filter((x) => x.op === "workspace.run_script").map((x) => x.params)).toEqual([
      { workspace_id: "w1", script: "run" },
      { workspace_id: "w1", script: "setup" },
      { workspace_id: "w1", script: "setup" },
    ]);
    c.dispose();
  });

  it("points to Settings when the repository has no run script", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.run_script"] = () => {
      throw new ZwiimClientError("script_not_set", "No run script", 409, { script: "run" });
    };
    const opened = vi.fn();
    const c = await mount(
      t,
      <OpenSettingsContext.Provider value={opened}>
        <TerminalPanel workspaceId="w1" />
      </OpenSettingsContext.Provider>,
    );
    act(() => c.store.dispatch({ type: "workspaces", workspaces: [root, worktree("w1")] }));
    fireEvent.click(screen.getByRole("button", { name: "Run" }));
    await tick();
    expect(screen.getByRole("status").textContent).toContain("no run script");
    fireEvent.click(screen.getByRole("button", { name: "Add one in Settings" }));
    expect(opened).toHaveBeenCalledWith({ tab: "repository", settingId: "script-run" });
    expect(screen.queryByRole("status")).toBeNull();
    c.dispose();
  });

  it("offers scripts only in a worktree, of a Host that runs them", async () => {
    const t = new FakeTransport();
    const c = await mount(t, <TerminalPanel workspaceId="root" />);
    act(() => c.store.dispatch({ type: "workspaces", workspaces: [root, worktree("w1")] }));
    expect(screen.queryByRole("button", { name: "Run" })).toBeNull();
    c.dispose();
    cleanup();

    const old = new FakeTransport();
    old.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "workspace.run_script") });
    const c2 = await mount(old, <TerminalPanel workspaceId="w1" />);
    act(() => c2.store.dispatch({ type: "workspaces", workspaces: [root, worktree("w1")] }));
    expect(screen.queryByRole("button", { name: "Run" })).toBeNull();
    expect(screen.queryByTitle("Run setup again")).toBeNull();
    c2.dispose();
  });

  it("opens a worktree's first preview and lists them all", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.previews"] = () => ({
      previews: [
        { name: "App", url: "http://localhost:40001" },
        { name: null, url: "http://127.0.0.1:5173/" },
      ],
    });
    const opened = vi.fn();
    const c = await mount(
      t,
      <OpenSettingsContext.Provider value={opened}>
        <TerminalPanel workspaceId="w1" />
      </OpenSettingsContext.Provider>,
    );
    act(() => c.store.dispatch({ type: "workspaces", workspaces: [root, worktree("w1")] }));

    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    await tick();
    expect(links.openLink.mock.calls).toEqual([["http://localhost:40001"]]);
    expect(t.calls.filter((x) => x.op === "workspace.previews").map((x) => x.params)).toEqual([{ workspace_id: "w1" }]);

    fireEvent.click(screen.getByRole("button", { name: "Previews" }));
    await tick();
    const menu = within(screen.getByRole("menu", { name: "Previews" }));
    const items = menu.getAllByRole("menuitem");
    expect(items.map((i) => i.textContent)).toEqual([
      "Apphttp://localhost:40001",
      "http://127.0.0.1:5173/",
      "Preview URLs…",
    ]);
    fireEvent.click(items[1]);
    expect(links.openLink).toHaveBeenLastCalledWith("http://127.0.0.1:5173/");
    expect(screen.getByRole("button", { name: "Previews" }).getAttribute("aria-expanded")).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "Previews" }));
    await tick();
    fireEvent.click(within(screen.getByRole("menu", { name: "Previews" })).getByText("Preview URLs…"));
    expect(opened).toHaveBeenCalledWith({ tab: "repository", settingId: "preview-urls" });
    c.dispose();
  });

  it("says how to get something to open when nothing is running", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.previews"] = () => ({ previews: [] });
    const opened = vi.fn();
    const c = await mount(
      t,
      <OpenSettingsContext.Provider value={opened}>
        <TerminalPanel workspaceId="w1" />
      </OpenSettingsContext.Provider>,
    );
    act(() => c.store.dispatch({ type: "workspaces", workspaces: [root, worktree("w1")] }));
    fireEvent.click(screen.getByRole("button", { name: "Previews" }));
    await tick();
    expect(screen.getByRole("menu", { name: "Previews" }).textContent).toContain("No servers running");
    fireEvent.keyDown(window, { key: "Escape" });

    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    await tick();
    expect(links.openLink).not.toHaveBeenCalled();
    expect(screen.getByRole("status").textContent).toContain("Nothing to open yet");
    fireEvent.click(screen.getByRole("button", { name: "Add one in Settings" }));
    expect(opened).toHaveBeenCalledWith({ tab: "repository", settingId: "preview-urls" });
    expect(screen.queryByRole("status")).toBeNull();
    c.dispose();
  });

  it("offers Open only on the Host's own computer, of a Host that has previews", async () => {
    const t = new FakeTransport();
    const c = await mount(t, <TerminalPanel workspaceId="w1" remote />);
    act(() => c.store.dispatch({ type: "workspaces", workspaces: [root, worktree("w1")] }));
    expect(screen.getByRole("button", { name: "Run" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Open" })).toBeNull();
    c.dispose();
    cleanup();

    const old = new FakeTransport();
    old.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "workspace.previews") });
    const c2 = await mount(old, <TerminalPanel workspaceId="w1" />);
    act(() => c2.store.dispatch({ type: "workspaces", workspaces: [root, worktree("w1")] }));
    expect(screen.getByRole("button", { name: "Run" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Open" })).toBeNull();
    c2.dispose();
  });
});
