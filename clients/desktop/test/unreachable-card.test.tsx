import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController, ZwiimClientError } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { SessionPanel } from "@/panels/SessionPanel";
import { SignInNeededCard } from "@/panels/SignInNeededCard";
import { LiveTurnView } from "@/panels/Transcript";
import { UnreachableCard } from "@/panels/UnreachableCard";
import { OpenSettingsContext } from "@/settings/open";

import { FakeTransport, hostHello, tick } from "./fakes";

afterEach(cleanup);

async function mountCard(t: FakeTransport, props: Partial<Parameters<typeof UnreachableCard>[0]> = {}) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  const onStarted = vi.fn();
  render(
    <HostProvider value={controller}>
      <UnreachableCard
        agentName="Claude Code"
        message="provider unreachable"
        sessionId="s1"
        online
        onStarted={onStarted}
        {...props}
      />
    </HostProvider>,
  );
  await tick();
  return { controller, onStarted };
}

const retry = () => screen.getByRole("button", { name: "Retry →" }) as HTMLButtonElement;
const retryNew = () => screen.getByRole("button", { name: "Retry in new chat →" }) as HTMLButtonElement;

describe("UnreachableCard", () => {
  it("renders title and both buttons", async () => {
    const { controller } = await mountCard(new FakeTransport());
    expect(screen.getByTestId("unreachable-card").textContent).toContain("Claude Code error: provider unreachable");
    expect(retry()).toBeTruthy();
    expect(retryNew()).toBeTruthy();
    controller.dispose();
  });

  it("retry calls session.retry and nothing fires before the tap", async () => {
    const t = new FakeTransport();
    t.handlers["session.retry"] = (p) => ({ session_id: p.session_id });
    const { controller, onStarted } = await mountCard(t);
    expect(t.calls.some((c) => c.op === "session.retry" || c.op === "session.retry_in_new_chat")).toBe(false);
    fireEvent.click(retry());
    await tick();
    expect(t.calls.filter((c) => c.op === "session.retry").map((c) => c.params)).toEqual([{ session_id: "s1" }]);
    expect(onStarted).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("retry in new chat calls its op and opens the new session", async () => {
    const t = new FakeTransport();
    t.handlers["session.retry_in_new_chat"] = () => ({ session_id: "s2" });
    const { controller, onStarted } = await mountCard(t);
    fireEvent.click(retryNew());
    await tick();
    expect(t.calls.find((c) => c.op === "session.retry_in_new_chat")?.params).toEqual({ session_id: "s1" });
    expect(onStarted).toHaveBeenCalledWith("s2");
    controller.dispose();
  });

  it("disables both buttons while pending and when offline", async () => {
    const t = new FakeTransport();
    let release!: () => void;
    t.handlers["session.retry"] = (p) =>
      new Promise((r) => {
        release = () => r({ session_id: p.session_id });
      });
    const { controller } = await mountCard(t);
    expect(retry().disabled).toBe(false);
    expect(retryNew().disabled).toBe(false);
    fireEvent.click(retry());
    await tick();
    expect(retry().disabled).toBe(true);
    expect(retryNew().disabled).toBe(true);
    release();
    await tick();
    // The Host answered, but the card is still on screen until the new turn
    // clears the error: a second tap now would only be refused.
    expect(retry().disabled).toBe(true);
    expect(retryNew().disabled).toBe(true);
    controller.dispose();

    cleanup();
    const off = await mountCard(new FakeTransport(), { online: false });
    expect(retry().disabled).toBe(true);
    expect(retryNew().disabled).toBe(true);
    off.controller.dispose();
  });

  it("shows a refusal inline and lets the user try again", async () => {
    const t = new FakeTransport();
    t.handlers["session.retry"] = () => {
      throw new ZwiimClientError("nothing_to_retry", "There is nothing to retry", 409);
    };
    const { controller, onStarted } = await mountCard(t);
    fireEvent.click(retry());
    await tick();
    const refusal = screen.getByTestId("unreachable-card").querySelector("[role=alert]")!;
    expect(refusal.textContent).toBe("There is nothing to retry");
    expect(retry().disabled).toBe(false);
    expect(onStarted).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("keeps both buttons off after a retry in new chat starts a session", async () => {
    const t = new FakeTransport();
    t.handlers["session.retry_in_new_chat"] = () => ({ session_id: "s2" });
    const { controller, onStarted } = await mountCard(t);
    fireEvent.click(retryNew());
    await tick();
    expect(onStarted).toHaveBeenCalledWith("s2");
    expect(retry().disabled).toBe(true);
    expect(retryNew().disabled).toBe(true);
    controller.dispose();
  });

  it("re-enables the buttons when the retry is refused", async () => {
    const t = new FakeTransport();
    t.handlers["session.retry"] = () => {
      throw new ZwiimClientError("turn_in_progress", "turn in flight", 409);
    };
    const { controller } = await mountCard(t);
    fireEvent.click(retry());
    await tick();
    expect(retry().disabled).toBe(false);
    expect(retryNew().disabled).toBe(false);
    controller.dispose();
  });

  it("shows a refusal from retry in new chat inline", async () => {
    const t = new FakeTransport();
    t.handlers["session.retry_in_new_chat"] = () => {
      throw new ZwiimClientError("turn_in_progress", "turn in flight", 409);
    };
    const { controller, onStarted } = await mountCard(t);
    fireEvent.click(retryNew());
    await tick();
    expect(screen.getByRole("alert").textContent).toMatch(/still working/);
    expect(onStarted).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("shows an unknown outcome as a refusal instead of repeating the call", async () => {
    const t = new FakeTransport();
    t.handlers["session.retry_in_new_chat"] = () => {
      throw new ZwiimClientError("unknown_outcome", "lost");
    };
    const { controller } = await mountCard(t);
    fireEvent.click(retryNew());
    await tick();
    expect(t.calls.filter((c) => c.op === "session.retry_in_new_chat")).toHaveLength(1);
    expect(screen.getByRole("alert").textContent).toMatch(/may or may not have been applied/);
    controller.dispose();
  });
});

async function mountPanel(t: FakeTransport, lastError: { message: string; code: string } | null) {
  t.handlers["session.list"] = () => [
    {
      session_id: "s1",
      workspace_id: "w1",
      agent: "claude_code",
      title: "t",
      status: "active",
      live: true,
      created_at: "",
      updated_at: "",
    },
  ];
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  await controller.refreshAgents();
  await controller.refreshSessions("w1");
  render(
    <HostProvider value={controller}>
      <SessionPanel workspaceId="w1" sessionId="s1" onSessionStarted={() => {}} />
    </HostProvider>,
  );
  await tick();
  await act(async () => {
    t.frame({
      type: "snapshot",
      subscription_id: "sub-1",
      session_id: "s1",
      epoch: "e1",
      event_seq: 1,
      snapshot: {
        status: "connected",
        live_message: null,
        tool_calls: [],
        pending_request: null,
        pending_user_message: null,
        last_error: lastError,
        usage: null,
        options: null,
        commands: [],
      },
    });
  });
  await tick();
  return controller;
}

describe("SessionPanel can't-reach error", () => {
  it("shows the card for agent_unreachable on a Host with session.retry", async () => {
    const t = new FakeTransport();
    const c = await mountPanel(t, { message: "provider unreachable", code: "agent_unreachable" });
    expect(screen.getByTestId("unreachable-card").textContent).toContain("Claude Code error: provider unreachable");
    expect(screen.getByRole("button", { name: "Retry →" })).toBeTruthy();
    c.dispose();
  });

  it("drops the card when a new turn starts", async () => {
    const t = new FakeTransport();
    const c = await mountPanel(t, { message: "provider unreachable", code: "agent_unreachable" });
    expect(screen.getByTestId("unreachable-card")).toBeTruthy();
    await act(async () => {
      t.frame({
        type: "event",
        subscription_id: "sub-1",
        event: {
          session_id: "s1",
          seq: 2,
          type: "agent.user_message",
          payload: { message_id: "u2", blocks: [{ type: "text", text: "again" }] },
        },
      });
    });
    await tick();
    expect(screen.queryByTestId("unreachable-card")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
    c.dispose();
  });

  it("shows a fresh card when a new error replaces the last without a clear in between", async () => {
    const t = new FakeTransport();
    t.handlers["session.retry"] = (p) => ({ session_id: p.session_id });
    const c = await mountPanel(t, { message: "provider unreachable", code: "agent_unreachable" });
    fireEvent.click(retry());
    await tick();
    expect(retry().disabled).toBe(true);
    // E.g. the user message and the next failure arrive in one replay.
    await act(async () => {
      t.frame({
        type: "event",
        subscription_id: "sub-1",
        event: { session_id: "s1", seq: 2, type: "agent.error", payload: { message: "provider unreachable", code: "agent_unreachable" } },
      });
    });
    await tick();
    expect(retry().disabled).toBe(false);
    expect(retryNew().disabled).toBe(false);
    c.dispose();
  });

  it("keeps the plain line for other codes or without the capability", async () => {
    const t = new FakeTransport();
    const c = await mountPanel(t, { message: "boom", code: "internal" });
    expect(screen.queryByTestId("unreachable-card")).toBeNull();
    expect(screen.getByRole("alert").textContent).toBe("boom");
    c.dispose();
    cleanup();

    const old = new FakeTransport();
    old.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "session.retry") });
    const c2 = await mountPanel(old, { message: "provider unreachable", code: "agent_unreachable" });
    expect(screen.queryByTestId("unreachable-card")).toBeNull();
    expect(screen.getByRole("alert").textContent).toBe("provider unreachable");
    c2.dispose();
  });
});

describe("SessionPanel sign-in-needed error", () => {
  it("shows the sign-in card for turn_failed_auth_required", async () => {
    const t = new FakeTransport();
    const c = await mountPanel(t, { message: "Claude Code needs you to sign in", code: "turn_failed_auth_required" });
    expect(screen.getByTestId("sign-in-needed-card").textContent).toContain("Claude Code needs you to sign in");
    expect(screen.queryByTestId("unreachable-card")).toBeNull();
    // No settings dialog above the panel here, so no way to open one.
    expect(screen.queryByRole("button", { name: "Open settings →" })).toBeNull();
    c.dispose();
  });

  it("opens settings at the agents", () => {
    const open = vi.fn();
    render(
      <OpenSettingsContext.Provider value={open}>
        <SignInNeededCard message="sign in" />
      </OpenSettingsContext.Provider>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Open settings →" }));
    expect(open).toHaveBeenCalledWith({ tab: "agents" });
  });
});

describe("Transcript Working row", () => {
  const turn = { userMessage: null, message: null, toolCalls: {} };

  it("reads Reconnecting… when the agent is reconnecting", () => {
    render(<LiveTurnView turn={turn} streaming reconnecting />);
    expect(screen.getByTestId("working").textContent).toContain("Reconnecting…");
    expect(screen.getByTestId("working").textContent).not.toContain("Working");
  });

  it("reads Working otherwise", () => {
    render(<LiveTurnView turn={turn} streaming />);
    expect(screen.getByTestId("working").textContent).toContain("Working");
  });
});
