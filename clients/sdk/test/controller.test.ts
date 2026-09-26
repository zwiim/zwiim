import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import { FakeTransport, flush } from "./fakes";

const snapshot = {
  status: "prompting" as const,
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands: [],
};

function setup() {
  const transport = new FakeTransport();
  const client = new HostClient({ transport, random: () => 0.5 });
  const controller = new HostController(client);
  return { transport, client, controller, store: controller.store };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("HostController", () => {
  it("takes host lists from the feed snapshot on every connect, without list calls", async () => {
    const t = setup();
    let listed = [{ workspace_id: "w1", name: "proj", alias: null, kind: "root", parent_workspace_id: null }];
    t.transport.handlers["workspace.list"] = () => listed;
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    expect(t.store.getState().workspaces).toHaveLength(1);
    expect(t.store.getState().sessionsByWorkspace).toEqual({ w1: [] });
    expect(t.store.getState().host?.host_id).toBe("host-1");

    t.transport.channel.drop();
    listed = [...listed, { ...listed[0]!, workspace_id: "w2" }];
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    await flush();
    expect(t.store.getState().workspaces?.map((w) => w.workspace_id)).toEqual(["w1", "w2"]);
    for (const op of ["workspace.list", "session.list", "terminal.list"] as const) {
      expect(t.transport.count(op)).toBe(0);
    }
  });

  it.each(["inprogress", "in_progress"])("keeps one tool status in a loaded transcript (%s)", async (wire) => {
    const t = setup();
    t.transport.handlers["session.history"] = () => ({
      session_id: "s1",
      turns: [
        {
          turn_id: "t",
          role: "assistant",
          timestamp: "2026-09-26T00:00:00Z",
          completed_at: null,
          duration_ms: null,
          model: null,
          usage: null,
          blocks: [{ type: "tool_call", tool_call_id: "c", title: "Bash", input_preview: null, status: wire }],
        },
      ],
    });
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const block = t.store.getState().sessions.s1?.history?.[0]?.blocks[0];
    expect(block?.type === "tool_call" && block.status).toBe("in_progress");
  });

  it("re-reads open transcripts on reconnect", async () => {
    const t = setup();
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const before = t.transport.count("session.history");
    t.transport.channel.drop();
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    await flush();
    expect(t.transport.count("session.history")).toBeGreaterThan(before);
  });

  it("loads history on open and again after a turn completes", async () => {
    const t = setup();
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot, event_seq: 1 });
    await flush();
    ch.frame({
      type: "event",
      subscription_id: "sub-1",
      event: { session_id: "s1", seq: 2, type: "agent.turn_complete", payload: { stop_reason: "end_turn" } },
    });
    await flush();
    // open + snapshot + turn_complete (the first two may coalesce)
    expect(t.transport.count("session.history")).toBeGreaterThanOrEqual(2);
    const view = t.store.getState().sessions.s1!;
    expect(view.historyStale).toBe(false);
    expect(view.lastStopReason).toBe("end_turn");
  });

  it("does not re-read a session being deleted, only one whose delete failed", async () => {
    const t = setup();
    let busy = false;
    t.transport.handlers["session.delete"] = () => {
      // The host stops the agent before the session is gone.
      t.transport.channel.frame({ type: "detached", subscription_id: "sub-1", reason: "session_not_live" });
      if (busy) throw new Error("turn_in_progress");
      return {};
    };
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    t.transport.channel.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot, event_seq: 1 });
    await flush();
    busy = true;
    const before = t.transport.count("session.history");
    await t.controller.deleteSession("s1").catch(() => {});
    await flush();
    expect(t.transport.count("session.history")).toBe(before + 1);
    busy = false;
    await t.controller.deleteSession("s1");
    await flush();
    expect(t.transport.count("session.history")).toBe(before + 1);
    expect(t.transport.count("session.options")).toBe(1);
  });

  it("keeps a finished turn on screen until the agent's transcript includes it", async () => {
    const t = setup();
    const user = { turn_id: "u", role: "user", blocks: [{ type: "text", text: "hi" }], timestamp: "", completed_at: null, duration_ms: null, model: null, usage: null };
    const reply = { ...user, turn_id: "a", role: "assistant", blocks: [{ type: "text", text: "hello" }] };
    let persisted: unknown[] = [];
    t.transport.handlers["session.history"] = () => ({ session_id: "s1", turns: persisted });
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot, event_seq: 1 });
    await flush();
    const ev = (seq: number, type: string, payload: unknown) =>
      ch.frame({ type: "event", subscription_id: "sub-1", event: { session_id: "s1", seq, type, payload } as never });
    ev(2, "agent.user_message", { message_id: "m", blocks: [{ type: "text", text: "hi" }] });
    ev(3, "agent.message_delta", { text: "hello" });
    persisted = [user]; // the reply is not flushed yet
    ev(4, "agent.turn_complete", { stop_reason: "end_turn" });
    await flush();
    expect(t.store.getState().sessions.s1!.settled?.message?.content).toEqual([{ type: "text", text: "hello" }]);

    persisted = [user, reply];
    await vi.advanceTimersByTimeAsync(250);
    await flush();
    const view = t.store.getState().sessions.s1!;
    expect(view.settled).toBeNull();
    expect(view.history).toHaveLength(2);
  });

  it("gives up waiting and shows the host's history", async () => {
    const t = setup();
    t.transport.handlers["session.history"] = () => ({ session_id: "s1", turns: [] });
    t.client.start();
    await flush();
    t.transport.channel.open();
    await flush();
    t.controller.openSession("s1");
    await flush();
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e", snapshot, event_seq: 1 });
    ch.frame({ type: "event", subscription_id: "sub-1", event: { session_id: "s1", seq: 2, type: "agent.user_message", payload: { message_id: "m", blocks: [{ type: "text", text: "hi" }] } } });
    ch.frame({ type: "event", subscription_id: "sub-1", event: { session_id: "s1", seq: 3, type: "agent.turn_complete", payload: { stop_reason: "cancelled" } } });
    await flush();
    expect(t.store.getState().sessions.s1!.settled).not.toBeNull();
    await vi.advanceTimersByTimeAsync(4000);
    await flush();
    expect(t.store.getState().sessions.s1!.settled).toBeNull();
  });

  it("does not load history for sessions that are not open", async () => {
    const t = setup();
    t.controller.openSession("s1");
    t.controller.closeSession("s1");
    await flush();
    expect(t.store.getState().sessions.s1).toBeUndefined();
  });

  it("adds a workspace from the host's answer", async () => {
    const t = setup();
    t.transport.handlers["workspace.add"] = () => ({
      workspace_id: "w2",
      name: "extra",
      alias: null,
      kind: "root",
      parent_workspace_id: null,
    });
    await t.controller.addWorkspace("/tmp/extra");
    expect(t.store.getState().workspaces?.map((w) => w.workspace_id)).toEqual(["w2"]);
    expect(t.transport.requests.at(-1)?.options).toEqual({ mutating: true });
  });
});
