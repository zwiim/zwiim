import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type WorkspaceInfo } from "@zwiim/sdk";
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
        ? [{ session_id: "s9", workspace_id: null, title: "old task", agent: "claude_code", status: "pending_review", live: false, created_at: "", updated_at: "" }]
        : [];
    t.handlers["workspace.tree"] = () => [{ name: "a.txt", path: "a.txt", kind: "file" }];
    t.handlers["files.read"] = (p) => ({ path: p.path, content: "hello from a", etag: "e", truncated: false, binary: false, size: 12 });
    return t;
  }

  it("keeps panels mounted across tabs", async () => {
    const t = workbenchHost();
    const c = await mount(t, <Workbench bridge={bridge()} />);
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    await tick();
    fireEvent.click(screen.getByText("a.txt"));
    await tick();
    expect(screen.getByText("hello from a")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Session" }));
    await tick();
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    await tick();
    expect(screen.getByText("hello from a")).toBeTruthy();
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
