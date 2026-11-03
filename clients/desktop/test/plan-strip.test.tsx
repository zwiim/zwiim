import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  ZwiimClientError,
  type AgentStatus,
  type HistoryTurn,
  type PlanContinues,
  type Progress,
  type SavedPlan,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { ARCHIVED_REASON, ROOT_REASON } from "@/panels/PlanStrip";
import { SessionPanel } from "@/panels/SessionPanel";

import { FakeTransport, hostHello, tick } from "./fakes";

afterEach(cleanup);

const titles = ["Map the reconnect paths", "Patch the LAN client", "Run the LAN tests", "Update the docs", "Ship it"];

const working: Progress = {
  steps: titles.map((title, i) => ({ title, status: i < 2 ? "completed" : i === 2 ? "in_progress" : "pending" })),
  completed: 2,
  total: 5,
};

const stopped: SavedPlan = {
  steps: titles.map((title, i) => ({ title, status: i < 2 ? "completed" : "pending" })),
  completed: 2,
  total: 5,
  plan_id: "p1",
  agent: "claude_code",
  state: "interrupted",
  resumable: true,
};

const workspace = (parent: string | null, removed = false): WorkspaceInfo => ({
  workspace_id: "w1",
  name: "repo",
  alias: null,
  kind: parent === null ? "root" : "worktree",
  parent_workspace_id: parent,
  branch: "zwiim/w1",
  base_branch: null,
  base_commit: null,
  removed,
  is_git_repo: true,
  display_path: null,
});

const session = (session_id: string, title: string) => ({
  session_id,
  workspace_id: "w1",
  agent: "claude_code",
  title,
  status: "active",
  live: true,
  created_at: "",
  updated_at: "",
});

const turn = (turn_id: string, role: string, blocks: HistoryTurn["blocks"]): HistoryTurn => ({
  turn_id,
  role,
  blocks,
  timestamp: "",
  completed_at: null,
  duration_ms: null,
  model: null,
  usage: null,
});

async function mountPlan(
  t: FakeTransport,
  opts: {
    sessionId?: string;
    status?: AgentStatus;
    progress?: Progress | null;
    lastError?: { message: string; code: string } | null;
    history?: {
      turns?: HistoryTurn[];
      progress?: SavedPlan | null;
      continues?: PlanContinues | null;
      continued_in?: { session_id: string } | null;
    };
    parent?: string | null;
    removed?: boolean;
  } = {},
) {
  const sessionId = opts.sessionId ?? "s1";
  t.handlers["workspace.list"] = () => [workspace(opts.parent === undefined ? "w0" : opts.parent, opts.removed)];
  t.handlers["session.list"] = () => [session("s1", "Fix LAN reconnect"), session("s2", "Fix LAN reconnect (continued)")];
  t.handlers["session.history"] = (p) => ({ session_id: p.session_id, turns: [], ...opts.history });
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  await controller.refreshAgents();
  await controller.refreshWorkspaces();
  await controller.refreshSessions("w1");
  const onSessionStarted = vi.fn();
  render(
    <HostProvider value={controller}>
      <SessionPanel workspaceId="w1" sessionId={sessionId} onSessionStarted={onSessionStarted} />
    </HostProvider>,
  );
  await tick();
  await act(async () => {
    t.frame({
      type: "snapshot",
      subscription_id: "sub-1",
      session_id: sessionId,
      epoch: "e1",
      event_seq: 1,
      snapshot: {
        status: opts.status ?? "connected",
        live_message: null,
        tool_calls: [],
        pending_request: null,
        pending_user_message: null,
        last_error: opts.lastError ?? null,
        usage: null,
        options: null,
        commands: [],
        progress: opts.progress ?? null,
      },
    });
  });
  await tick();
  return { controller, onSessionStarted };
}

const continueButton = () => screen.getByRole("button", { name: "Continue with Claude Code" }) as HTMLButtonElement;
const historyReads = (t: FakeTransport) => t.calls.filter((c) => c.op === "session.history").length;
const continueCalls = (t: FakeTransport) => t.calls.filter((c) => c.op === "session.continue_plan").length;
const refuse = (t: FakeTransport, code: string) => {
  t.handlers["session.continue_plan"] = () => {
    throw new ZwiimClientError(code as never, "refused", 409);
  };
};

describe("PlanStrip", () => {
  it("shows the working strip with count and steps", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, { status: "prompting", progress: working });
    const strip = screen.getByTestId("plan-strip");
    expect(strip.textContent).toContain("Run the LAN tests");
    expect(strip.textContent).toContain("2 of 5");
    expect(screen.queryByTestId("plan-steps")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show tasks" }));
    const list = screen.getByTestId("plan-steps");
    expect(list.querySelectorAll("li")).toHaveLength(5);
    expect(list.textContent).toContain("Ship it");
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByTestId("plan-steps")).toBeNull();
    // A click outside closes it too.
    fireEvent.click(screen.getByRole("button", { name: "Show tasks" }));
    expect(screen.getByTestId("plan-steps")).toBeTruthy();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByTestId("plan-steps")).toBeNull();
    // Stop stays the composer's own button.
    expect(screen.getByTestId("stop-agent")).toBeTruthy();
    controller.dispose();
  });

  it("shows the resume bar when stopped", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, { history: { progress: stopped } });
    const strip = screen.getByTestId("plan-strip");
    expect(strip.textContent).toContain("Stopped at task 3 · Run the LAN tests");
    expect(strip.textContent).toContain("2 of 5 done");
    expect(continueButton().disabled).toBe(false);
    controller.dispose();
  });

  it("continues with the session's agent", async () => {
    const t = new FakeTransport();
    t.handlers["session.continue_plan"] = () => ({ session_id: "s3", plan_id: "p1" });
    const { controller, onSessionStarted } = await mountPlan(t, { history: { progress: stopped } });
    expect(t.calls.some((c) => c.op === "session.continue_plan")).toBe(false);
    fireEvent.click(continueButton());
    await tick();
    expect(t.calls.filter((c) => c.op === "session.continue_plan").map((c) => c.params)).toEqual([
      { session_id: "s1" },
    ]);
    expect(onSessionStarted).toHaveBeenCalledWith("s3");
    controller.dispose();
  });

  it("continues with another agent from the menu", async () => {
    const t = new FakeTransport();
    t.handlers["agent.list"] = () => [
      { agent: "claude_code", name: "Claude Code", available: true, reason: null, support: "supported", options: null },
      { agent: "codex", name: "Codex", available: true, reason: null, support: "supported", options: null },
    ];
    t.handlers["session.continue_plan"] = () => ({ session_id: "s3", plan_id: "p1" });
    const { controller, onSessionStarted } = await mountPlan(t, { history: { progress: stopped } });
    fireEvent.click(screen.getByRole("button", { name: "Continue with another agent" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Codex" }));
    await tick();
    expect(t.calls.filter((c) => c.op === "session.continue_plan").map((c) => c.params)).toEqual([
      { session_id: "s1", agent: "codex" },
    ]);
    expect(onSessionStarted).toHaveBeenCalledWith("s3");
    controller.dispose();
  });

  it("disables Continue in the root workspace", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, { history: { progress: stopped }, parent: null });
    expect(continueButton().disabled).toBe(true);
    expect(screen.getByTestId("plan-strip").textContent).toContain(
      "Continue needs a worktree. Start a new session from a worktree.",
    );
    controller.dispose();
  });

  it("re-reads history on nothing_to_continue", async () => {
    const t = new FakeTransport();
    t.handlers["session.continue_plan"] = () => {
      throw new ZwiimClientError("nothing_to_continue", "nothing to continue", 409);
    };
    const { controller, onSessionStarted } = await mountPlan(t, { history: { progress: stopped } });
    const before = historyReads(t);
    fireEvent.click(continueButton());
    await tick();
    expect(historyReads(t)).toBeGreaterThan(before);
    expect(screen.queryByTestId("plan-strip")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(onSessionStarted).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("shows the generic notice on other errors", async () => {
    const t = new FakeTransport();
    t.handlers["session.continue_plan"] = () => {
      throw new ZwiimClientError("agent_not_installed", "no such agent", 409);
    };
    const { controller, onSessionStarted } = await mountPlan(t, { history: { progress: stopped } });
    fireEvent.click(continueButton());
    await tick();
    expect(screen.getByRole("alert").textContent).toBe("That agent is not installed on the host.");
    // The bar stays, ready to try again.
    expect(continueButton().disabled).toBe(false);
    expect(onSessionStarted).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("shows Continued in row instead of the bar", async () => {
    const t = new FakeTransport();
    const { controller, onSessionStarted } = await mountPlan(t, {
      history: { progress: { ...stopped, resumable: false }, continued_in: { session_id: "s2" } },
    });
    expect(screen.queryByTestId("plan-strip")).toBeNull();
    const row = screen.getByRole("button", { name: "Continued in Fix LAN reconnect (continued) →" });
    fireEvent.click(row);
    expect(onSessionStarted).toHaveBeenCalledWith("s2");
    controller.dispose();
  });

  it("reads Continued in new session when the new one is not listed", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, { history: { continued_in: { session_id: "s9" } } });
    expect(screen.getByRole("button", { name: "Continued in new session →" })).toBeTruthy();
    controller.dispose();
  });

  it("puts Continued in below the can't-reach card", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, {
      lastError: { message: "provider unreachable", code: "agent_unreachable" },
      history: { progress: { ...stopped, resumable: false }, continued_in: { session_id: "s2" } },
    });
    const card = screen.getByTestId("unreachable-card");
    expect(card.textContent).toContain("Claude Code error: provider unreachable");
    const row = screen.getByTestId("continued-in");
    expect(card.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.queryByTestId("plan-strip")).toBeNull();
    controller.dispose();
  });

  it("hides the resume bar while the can't-reach card shows", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, {
      lastError: { message: "provider unreachable", code: "agent_unreachable" },
      history: { progress: stopped },
    });
    expect(screen.getByTestId("unreachable-card")).toBeTruthy();
    expect(screen.queryByTestId("plan-strip")).toBeNull();
    controller.dispose();
  });

  it("renders the first message as a card", async () => {
    const t = new FakeTransport();
    const attachment = (attachment_id: string, name: string) => ({
      type: "attachment" as const,
      attachment_id,
      name,
      mime_type: "text/markdown",
      size: 1200,
      path: `.zwiim/attachments/${attachment_id}/${name}`,
    });
    const { controller, onSessionStarted } = await mountPlan(t, {
      sessionId: "s2",
      history: {
        turns: [
          turn("u1", "user", [
            { type: "text", text: "Carry on with the generated plan prompt" },
            attachment("a1", "tasks.md"),
            attachment("a2", "previous-session.md"),
          ]),
          turn("r1", "assistant", [{ type: "text", text: "On it." }]),
        ],
        continues: { session_id: "s1", plan_id: "p1", agent: "claude_code", completed: 2, total: 5, title: "Fix LAN reconnect" },
      },
    });
    const card = screen.getByTestId("continued-card");
    expect(card.textContent).toContain("Continuing Claude Code's tasks · 2 of 5 done");
    const chips = [...card.querySelectorAll("[data-testid=sent-attachment]")].map((c) => c.textContent);
    expect(chips.some((c) => c?.includes("tasks.md"))).toBe(true);
    expect(chips.some((c) => c?.includes("previous-session.md"))).toBe(true);
    expect(screen.queryByText(/generated plan prompt/)).toBeNull();
    expect(screen.getByText("On it.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "From Fix LAN reconnect" }));
    expect(onSessionStarted).toHaveBeenCalledWith("s1");
    controller.dispose();
  });

  it("shows From without a link when the old session is gone", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, {
      sessionId: "s2",
      history: {
        turns: [turn("u1", "user", [{ type: "text", text: "prompt" }])],
        continues: { session_id: "s0", plan_id: "p1", agent: "claude_code", completed: 2, total: 5, title: "Old work" },
      },
    });
    expect(screen.getByTestId("continued-card").textContent).toContain("From Old work");
    expect(screen.queryByRole("button", { name: "From Old work" })).toBeNull();
    controller.dispose();
  });

  it("renders nothing without the capability", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () =>
      hostHello({
        capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "agent.progress" && x !== "session.continue_plan"),
      });
    const { controller } = await mountPlan(t, {
      history: { progress: stopped, continued_in: null },
    });
    expect(screen.queryByTestId("plan-strip")).toBeNull();
    controller.dispose();
    cleanup();

    const t2 = new FakeTransport();
    t2.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "agent.progress") });
    const c2 = await mountPlan(t2, { status: "prompting", progress: working });
    expect(screen.queryByTestId("plan-strip")).toBeNull();
    c2.controller.dispose();
  });

  it("never uses the error colour when stopped", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, { history: { progress: stopped } });
    const mark = screen.getByTestId("plan-mark");
    expect(mark.className).toMatch(/\bborder-brand\b/);
    expect(screen.getByTestId("plan-strip").outerHTML).not.toMatch(/destructive/);
    controller.dispose();
  });
  it("shows the loading state while Continue waits on the Host", async () => {
    const t = new FakeTransport();
    let resolve!: (v: unknown) => void;
    t.handlers["session.continue_plan"] = () => new Promise((r) => (resolve = r));
    const { controller, onSessionStarted } = await mountPlan(t, { history: { progress: stopped } });
    expect(continueButton().querySelector(".animate-spin")).toBeNull();
    fireEvent.click(continueButton());
    await tick();
    expect(continueButton().disabled).toBe(true);
    expect(continueButton().querySelector(".animate-spin")).not.toBeNull();
    fireEvent.click(continueButton());
    await tick();
    expect(continueCalls(t)).toBe(1);
    await act(async () => resolve({ session_id: "s3", plan_id: "p1" }));
    await tick();
    expect(onSessionStarted).toHaveBeenCalledWith("s3");
    controller.dispose();
  });

  it("keeps the bar on turn_in_progress, once history says it stopped", async () => {
    const t = new FakeTransport();
    refuse(t, "turn_in_progress");
    const { controller, onSessionStarted } = await mountPlan(t, { history: { progress: stopped } });
    const before = historyReads(t);
    fireEvent.click(continueButton());
    await tick();
    expect(historyReads(t)).toBeGreaterThan(before);
    // History still says it stopped: the bar is not hidden for good.
    expect(screen.getByTestId("plan-strip").textContent).toContain("Stopped at task 3");
    expect(continueButton().disabled).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(onSessionStarted).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("says Continue needs a worktree when the Host refuses with root_workspace", async () => {
    const t = new FakeTransport();
    refuse(t, "root_workspace");
    const { controller } = await mountPlan(t, { history: { progress: stopped } });
    fireEvent.click(continueButton());
    await tick();
    expect(screen.getByRole("alert").textContent).toBe(ROOT_REASON);
    controller.dispose();
  });

  it("disables Continue in an archived worktree", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, { history: { progress: stopped }, removed: true });
    expect(continueButton().disabled).toBe(true);
    expect(screen.getByTestId("plan-strip").textContent).toContain(ARCHIVED_REASON);
    controller.dispose();
  });

  it("says the worktree was archived when the Host refuses with workspace_removed", async () => {
    const t = new FakeTransport();
    refuse(t, "workspace_removed");
    const { controller } = await mountPlan(t, { history: { progress: stopped } });
    fireEvent.click(continueButton());
    await tick();
    expect(screen.getByRole("alert").textContent).toBe(ARCHIVED_REASON);
    controller.dispose();
  });

  it("clears a refused Continue's notice once the bar goes away", async () => {
    const t = new FakeTransport();
    refuse(t, "agent_not_installed");
    const { controller } = await mountPlan(t, { history: { progress: stopped } });
    fireEvent.click(continueButton());
    await tick();
    expect(screen.getByRole("alert")).toBeTruthy();
    t.handlers["session.history"] = (p) => ({
      session_id: p.session_id,
      turns: [],
      progress: { ...stopped, resumable: false },
      continued_in: { session_id: "s2" },
    });
    await act(async () => controller.openSession("s1"));
    await tick();
    expect(screen.queryByTestId("plan-strip")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    controller.dispose();
  });

  it("moves focus into the menu and back to its button on Esc", async () => {
    const t = new FakeTransport();
    t.handlers["agent.list"] = () => [
      { agent: "claude_code", name: "Claude Code", available: true, reason: null, support: "supported", options: null },
      { agent: "codex", name: "Codex", available: true, reason: null, support: "supported", options: null },
    ];
    const { controller } = await mountPlan(t, { history: { progress: stopped } });
    const trigger = screen.getByRole("button", { name: "Continue with another agent" });
    fireEvent.click(trigger);
    const menu = screen.getByRole("menu", { name: "Continue with" });
    expect(menu.className).toMatch(/\banimate-pop-in\b/);
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Codex" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    // The steps popover is a labelled dialog, and Esc hands focus back too.
    const steps = screen.getByRole("button", { name: "Show tasks" });
    fireEvent.click(steps);
    expect(screen.getByRole("dialog", { name: "Tasks" })).toBeTruthy();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Tasks" })).toBeNull();
    expect(document.activeElement).toBe(steps);
    controller.dispose();
  });

  it("keeps Reset to here on a continued plan's first message", async () => {
    const t = new FakeTransport();
    const { controller } = await mountPlan(t, {
      sessionId: "s2",
      history: {
        turns: [
          { ...turn("u1", "user", [{ type: "text", text: "prompt" }]), revertible: true },
          turn("r1", "assistant", [{ type: "text", text: "On it." }]),
        ],
        continues: { session_id: "s1", plan_id: "p1", agent: "claude_code", completed: 2, total: 5, title: "Fix LAN reconnect" },
      },
    });
    const card = screen.getByTestId("continued-card");
    const reset = screen.getByRole("button", { name: "Reset to here" });
    expect(card.parentElement?.contains(reset)).toBe(true);
    controller.dispose();
  });
});
