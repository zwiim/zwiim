import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { backoffDelay } from "../src/client/connection";
import { ZwiimClientError } from "../src/client/errors";
import { HostClient, type HostFeedUpdate } from "../src/client/host-client";
import type { SessionFrame } from "../src/state/session";
import type { TerminalOutput } from "../src/state/terminal";
import type { SessionEvent } from "../src/types/protocol";
import { FakeTransport, flush, hello } from "./fakes";

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

const delta = (seq: number, text: string): SessionEvent => ({
  session_id: "s1",
  seq,
  type: "agent.message_delta",
  payload: { text },
});

function setup(opts: { expectedHostId?: string } = {}) {
  const transport = new FakeTransport();
  const client = new HostClient({ transport, random: () => 0.5, ...opts });
  const statuses: string[] = [];
  const frames: [string, SessionFrame][] = [];
  const terminal: [string, TerminalOutput][] = [];
  const feed: HostFeedUpdate[] = [];
  client.subscribe({
    feed: (u) => feed.push(u),
    connection: (s) => {
      if (statuses[statuses.length - 1] !== s.status) statuses.push(s.status);
    },
    session: (id, f) => frames.push([id, f]),
    terminal: (id, o) => terminal.push([id, o]),
  });
  return { transport, client, statuses, frames, terminal, feed };
}

async function goLive(t: ReturnType<typeof setup>) {
  t.client.start();
  await flush();
  t.transport.channel.open();
}

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("backoff", () => {
  it("grows from ~0.5s to a 30s cap with jitter", () => {
    expect(backoffDelay(0, () => 0.5)).toBe(500);
    expect(backoffDelay(1, () => 0.5)).toBe(1000);
    expect(backoffDelay(5, () => 0.5)).toBe(16000);
    expect(backoffDelay(6, () => 0.5)).toBe(30000);
    expect(backoffDelay(50, () => 1)).toBe(30000);
    expect(backoffDelay(0, () => 0)).toBe(400);
    expect(backoffDelay(0, () => 1)).toBe(600);
  });
});

describe("connection lifecycle", () => {
  it("goes disconnected → connecting → handshaking → live", async () => {
    const t = setup();
    expect(t.client.state.status).toBe("disconnected");
    await goLive(t);
    expect(t.statuses).toEqual(["connecting", "handshaking", "live"]);
    expect(t.client.state.hostId).toBe("host-1");
  });

  it("stops on unauthenticated without retrying", async () => {
    const t = setup();
    t.transport.handlers["host.hello"] = () => {
      throw new ZwiimClientError("unauthenticated", "Missing or invalid token", 401);
    };
    t.client.start();
    await flush();
    expect(t.client.state.status).toBe("unauthenticated");
    await vi.advanceTimersByTimeAsync(120_000);
    expect(t.transport.count("host.hello")).toBe(1);
    expect(t.transport.channels).toHaveLength(0);
  });

  it("retries an unreachable host with growing backoff", async () => {
    const t = setup();
    let fail = true;
    t.transport.handlers["host.hello"] = () => {
      if (fail) throw new ZwiimClientError("network", "unreachable");
      return hello({ version: "x" });
    };
    t.client.start();
    await flush();
    expect(t.client.state).toMatchObject({ status: "reconnecting", attempt: 1, retryInMs: 500 });
    await vi.advanceTimersByTimeAsync(500);
    expect(t.client.state).toMatchObject({ status: "reconnecting", attempt: 2, retryInMs: 1000 });
    fail = false;
    await vi.advanceTimersByTimeAsync(1000);
    t.transport.channel.open();
    expect(t.client.state).toMatchObject({ status: "live", attempt: 0 });
    expect(t.transport.count("host.hello")).toBe(3);
  });

  it("reconnects after the socket drops: live → reconnecting → handshaking → live", async () => {
    const t = setup();
    await goLive(t);
    t.transport.channel.drop();
    expect(t.client.state.status).toBe("reconnecting");
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    expect(t.statuses).toEqual([
      "connecting",
      "handshaking",
      "live",
      "reconnecting",
      "handshaking",
      "live",
    ]);
  });

  it("refuses a different host at the same address", async () => {
    const t = setup({ expectedHostId: "host-other" });
    t.client.start();
    await flush();
    expect(t.client.state).toMatchObject({ status: "error", error: { code: "host_mismatch" } });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(t.transport.count("host.hello")).toBe(1);
  });

  it("re-reads discovery before every attempt", async () => {
    const transport = new FakeTransport();
    const factory = vi.fn(() => transport);
    const client = new HostClient({ transport: factory, random: () => 0.5 });
    client.start();
    await flush();
    transport.channel.open();
    transport.channel.drop();
    await vi.advanceTimersByTimeAsync(500);
    expect(factory).toHaveBeenCalledTimes(2);
  });

  it("stop() closes the socket and never reconnects", async () => {
    const t = setup();
    await goLive(t);
    t.client.stop();
    expect(t.client.state.status).toBe("disconnected");
    expect(t.transport.channel.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(120_000);
    expect(t.transport.channels).toHaveLength(1);
  });
});

describe("heartbeat", () => {
  it("pings every 20s and reconnects when no pong arrives within 10s", async () => {
    const t = setup();
    await goLive(t);
    const ch = t.transport.channel;
    await vi.advanceTimersByTimeAsync(20_000);
    expect(ch.sent).toContainEqual({ op: "ping" });
    ch.frame({ type: "pong" });
    await vi.advanceTimersByTimeAsync(20_000);
    expect(t.client.state.status).toBe("live");
    // Second ping unanswered.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(ch.closed).toBe(true);
    expect(t.client.state.status).toBe("reconnecting");
  });
});

describe("requests", () => {
  it("never retries a mutating request with an unknown outcome", async () => {
    const t = setup();
    await goLive(t);
    t.transport.handlers["session.send"] = () => {
      throw new ZwiimClientError("unknown_outcome", "response lost");
    };
    await expect(
      t.client.sendMessage({ session_id: "s1", blocks: [{ type: "text", text: "hi" }] }),
    ).rejects.toMatchObject({ code: "unknown_outcome" });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(t.transport.count("session.send")).toBe(1);
    expect(t.transport.requests.find((r) => r.op === "session.send")?.options).toEqual({
      mutating: true,
    });
  });

  it("marks reads as non-mutating", async () => {
    const t = setup();
    await t.client.request("workspace.list", {});
    expect(t.transport.requests[0]?.options).toEqual({ mutating: false });
  });

  it("goes unauthenticated when a request is rejected with 401", async () => {
    const t = setup();
    await goLive(t);
    t.transport.handlers["workspace.list"] = () => {
      throw new ZwiimClientError("unauthenticated", "no", 401);
    };
    await expect(t.client.request("workspace.list", {})).rejects.toThrow();
    expect(t.client.state.status).toBe("unauthenticated");
    expect(t.transport.channel.closed).toBe(true);
  });
});

describe("session subscriptions", () => {
  it("subscribes once live, tracks the cursor and drops duplicate events", async () => {
    const t = setup();
    t.client.attachSession("s1");
    await goLive(t);
    const ch = t.transport.channel;
    expect(ch.sent).toEqual([
      { op: "host.subscribe" },
      { op: "session.subscribe", subscription_id: "sub-1", session_id: "s1" },
    ]);

    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 4 });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(4, "old") });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(6, "a") });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(6, "a") });
    ch.frame({ type: "event", subscription_id: "sub-2", event: delta(9, "other") });
    const types = t.frames.map(([, f]) => f.type);
    expect(types).toEqual(["attaching", "snapshot", "event"]);
  });

  it("resumes with epoch + since_seq after reconnect and filters the replay", async () => {
    const t = setup();
    t.client.attachSession("s1");
    await goLive(t);
    let ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 4 });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(7, "a") });
    ch.drop();
    await vi.advanceTimersByTimeAsync(500);
    ch = t.transport.channel;
    ch.open();
    expect(ch.sent[1]).toEqual({
      op: "session.subscribe",
      subscription_id: "sub-1",
      session_id: "s1",
      epoch: "e1",
      since_seq: 7,
    });
    ch.frame({
      type: "replay",
      subscription_id: "sub-1",
      session_id: "s1",
      epoch: "e1",
      events: [delta(7, "dup"), delta(9, "b")],
      high_water_seq: 10,
    });
    const replay = t.frames[t.frames.length - 1]![1];
    expect(replay).toMatchObject({ type: "replay", high_water_seq: 10 });
    expect((replay as any).events.map((e: SessionEvent) => e.seq)).toEqual([9]);
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(10, "dup") });
    expect(t.frames[t.frames.length - 1]![1].type).toBe("replay");
  });

  it("requests a snapshot when the replay epoch changed", async () => {
    const t = setup();
    t.client.attachSession("s1");
    await goLive(t);
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 4 });
    ch.frame({ type: "replay", subscription_id: "sub-1", session_id: "s1", epoch: "e2", events: [], high_water_seq: 1 });
    expect(ch.sent[ch.sent.length - 1]).toEqual({
      op: "session.subscribe",
      subscription_id: "sub-1",
      session_id: "s1",
    });
  });

  it("resumes from the cursor after lagging", async () => {
    const t = setup();
    t.client.attachSession("s1");
    await goLive(t);
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 4 });
    ch.frame({ type: "detached", subscription_id: "sub-1", reason: "lagged" });
    expect(ch.sent[ch.sent.length - 1]).toMatchObject({ epoch: "e1", since_seq: 4 });
  });

  it("re-attaches a detached session after a successful send", async () => {
    const t = setup();
    t.transport.handlers["session.send"] = (p) => ({ session_id: p.session_id });
    t.client.attachSession("s1");
    await goLive(t);
    const ch = t.transport.channel;
    ch.frame({ type: "detached", subscription_id: "sub-1", reason: "session_not_live" });
    expect(t.frames[t.frames.length - 1]![1]).toEqual({ type: "detached", reason: "session_not_live" });
    const before = ch.sent.length;
    await t.client.sendMessage({ session_id: "s1", blocks: [{ type: "text", text: "go" }] });
    expect(ch.sent.slice(before)).toEqual([
      { op: "session.subscribe", subscription_id: "sub-1", session_id: "s1" },
    ]);
  });

  it("re-attaches a detached session after a revert, which may have resumed its agent", async () => {
    const t = setup();
    t.transport.handlers["session.revert"] = () => ({ files_restored: true, chat_rewound: true, head_moved: false });
    t.client.attachSession("s1");
    await goLive(t);
    const ch = t.transport.channel;
    ch.frame({ type: "detached", subscription_id: "sub-1", reason: "session_not_live" });
    const before = ch.sent.length;
    const out = await t.client.revertSession({ session_id: "s1", turn_id: "turn-2", files: true, chat: true });
    expect(out).toEqual({ files_restored: true, chat_rewound: true, head_moved: false });
    expect(ch.sent.slice(before)).toEqual([
      { op: "session.subscribe", subscription_id: "sub-1", session_id: "s1" },
    ]);
  });

  it("leaves a detached session detached after a revert that did not take the chat back", async () => {
    const t = setup();
    const results = [
      { files_restored: true, chat_rewound: false, head_moved: false },
      { files_restored: true, chat_rewound: false, head_moved: true },
    ];
    t.transport.handlers["session.revert"] = () => results.shift()!;
    t.client.attachSession("s1");
    await goLive(t);
    const ch = t.transport.channel;
    ch.frame({ type: "detached", subscription_id: "sub-1", reason: "session_not_live" });
    const before = ch.sent.length;
    // Files only: no agent resumed, so a subscribe would only be refused again.
    await t.client.revertSession({ session_id: "s1", turn_id: "turn-2", files: true, chat: false });
    // Chat asked for, but the agent could not take it back.
    await t.client.revertSession({ session_id: "s1", turn_id: "turn-2", files: true, chat: true });
    expect(ch.sent.slice(before)).toEqual([]);
  });

  it("leaves a live session's subscription alone after a revert", async () => {
    const t = setup();
    t.transport.handlers["session.revert"] = () => ({ files_restored: true, chat_rewound: true, head_moved: false });
    t.client.attachSession("s1");
    await goLive(t);
    const ch = t.transport.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 4 });
    const before = ch.sent.length;
    await t.client.revertSession({ session_id: "s1", turn_id: "turn-2", files: true, chat: true });
    expect(ch.sent.slice(before)).toEqual([]);
  });

  it("detach stops the subscription", async () => {
    const t = setup();
    t.client.attachSession("s1");
    await goLive(t);
    t.client.detachSession("s1");
    const ch = t.transport.channel;
    expect(ch.sent[ch.sent.length - 1]).toEqual({ op: "session.detach", subscription_id: "sub-1" });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(9, "late") });
    expect(t.frames.map(([, f]) => f.type)).toEqual(["attaching"]);
  });
});

describe("terminal streams", () => {
  const out = (seq: number, data: string) =>
    ({ type: "terminal", event: { terminal_id: "t1", seq, type: "terminal.output", payload: { data } } }) as const;

  it("subscribes, snapshots, then applies only newer contiguous output", async () => {
    const t = setup();
    let resolveSnap!: (v: unknown) => void;
    t.transport.handlers["terminal.snapshot"] = () => new Promise((r) => (resolveSnap = r));
    await goLive(t);
    const ch = t.transport.channel;
    t.client.attachTerminal("t1");
    expect(ch.sent).toContainEqual({ op: "terminal.subscribe", terminal_id: "t1" });
    // Output racing the snapshot is buffered.
    ch.frame(out(5, "e"));
    ch.frame(out(6, "f"));
    resolveSnap({ terminal_id: "t1", alive: true, data: "abcde", seq: 5 });
    await flush();
    expect(t.terminal.map(([, o]) => o)).toEqual([
      { kind: "reset", data: "abcde" },
      { kind: "append", data: "f" },
    ]);
    ch.frame(out(6, "f"));
    ch.frame(out(7, "g"));
    expect(t.terminal.map(([, o]) => o).slice(2)).toEqual([{ kind: "append", data: "g" }]);
  });

  it("re-snapshots on a sequence gap instead of applying stale output", async () => {
    const t = setup();
    let snapSeq = 1;
    t.transport.handlers["terminal.snapshot"] = () => ({
      terminal_id: "t1",
      alive: true,
      data: `screen@${snapSeq}`,
      seq: snapSeq,
    });
    await goLive(t);
    t.client.attachTerminal("t1");
    await flush();
    snapSeq = 4;
    t.transport.channel.frame(out(4, "gap"));
    await flush();
    expect(t.transport.count("terminal.snapshot")).toBe(2);
    expect(t.terminal.map(([, o]) => o)).toEqual([
      { kind: "reset", data: "screen@1" },
      { kind: "reset", data: "screen@4" },
    ]);
  });

  it("ignores a snapshot response superseded by a newer request", async () => {
    const t = setup();
    const pending: ((v: unknown) => void)[] = [];
    t.transport.handlers["terminal.snapshot"] = () => new Promise((r) => pending.push(r));
    await goLive(t);
    t.client.attachTerminal("t1");
    t.client.attachTerminal("t1"); // explicit re-sync
    pending[1]!({ terminal_id: "t1", alive: true, data: "new", seq: 9 });
    await flush();
    pending[0]!({ terminal_id: "t1", alive: true, data: "old", seq: 3 });
    await flush();
    expect(t.terminal.map(([, o]) => o)).toEqual([{ kind: "reset", data: "new" }]);
  });

  it("re-subscribes and re-snapshots after reconnect", async () => {
    const t = setup();
    t.transport.handlers["terminal.snapshot"] = () => ({ terminal_id: "t1", alive: true, data: "x", seq: 2 });
    await goLive(t);
    t.client.attachTerminal("t1");
    await flush();
    t.transport.channel.drop();
    // Output from the dead socket must not apply.
    await vi.advanceTimersByTimeAsync(500);
    const ch = t.transport.channel;
    ch.open();
    await flush();
    expect(ch.sent).toContainEqual({ op: "terminal.subscribe", terminal_id: "t1" });
    expect(t.transport.count("terminal.snapshot")).toBe(2);
  });

  it("reports exit once", async () => {
    const t = setup();
    t.transport.handlers["terminal.snapshot"] = () => ({ terminal_id: "t1", alive: true, data: "", seq: 0 });
    await goLive(t);
    t.client.attachTerminal("t1");
    await flush();
    const exit = { type: "terminal", event: { terminal_id: "t1", seq: 0, type: "terminal.exit", payload: {} } } as const;
    t.transport.channel.frame(exit);
    t.transport.channel.frame(exit);
    expect(t.terminal.map(([, o]) => o.kind)).toEqual(["reset", "exit"]);
  });
});

describe("host state feed", () => {
  const removed = (terminal_id: string) => ({ terminal_id });
  const feedSummary = (t: ReturnType<typeof setup>) =>
    t.feed.map((u) => (u.type === "snapshot" ? `snapshot@${u.snapshot.epoch}:${u.snapshot.seq}` : u.event.seq));
  const subscribes = (t: ReturnType<typeof setup>) =>
    t.transport.channels.flatMap((c) => c.sent).filter((m) => m.op === "host.subscribe").length;

  it("subscribes when live, then passes contiguous events and drops duplicates", async () => {
    const t = setup();
    t.transport.feedSeq = 4;
    await goLive(t);
    const ch = t.transport.channel;
    expect(ch.sent).toEqual([{ op: "host.subscribe" }]);
    const ev = (seq: number) =>
      ch.frame({ type: "host.event", epoch: "e1", event: { seq, type: "terminal.removed", payload: removed("t") } });
    ev(4); // already in the snapshot
    ev(5);
    ev(5);
    ev(6);
    ev(3);
    expect(feedSummary(t)).toEqual(["snapshot@e1:4", 5, 6]);
    expect(subscribes(t)).toBe(1);
  });

  it("resubscribes on a gap and ignores events until the new snapshot", async () => {
    const t = setup();
    t.transport.answerFeed = false;
    await goLive(t);
    const ch = t.transport.channel;
    // Events before any snapshot have no baseline to apply to.
    ch.frame({ type: "host.event", epoch: "e1", event: { seq: 1, type: "terminal.removed", payload: removed("t") } });
    expect(t.feed).toEqual([]);
    ch.frame({ type: "host.snapshot", ...t.transport.snapshot() });
    ch.frame({ type: "host.event", epoch: "e1", event: { seq: 2, type: "terminal.removed", payload: removed("t") } });
    expect(subscribes(t)).toBe(2);
    ch.frame({ type: "host.event", epoch: "e1", event: { seq: 3, type: "terminal.removed", payload: removed("t") } });
    expect(subscribes(t)).toBe(2);
    t.transport.feedSeq = 3;
    ch.frame({ type: "host.snapshot", ...t.transport.snapshot() });
    ch.frame({ type: "host.event", epoch: "e1", event: { seq: 4, type: "terminal.removed", payload: removed("t") } });
    expect(feedSummary(t)).toEqual(["snapshot@e1:0", "snapshot@e1:3", 4]);
  });

  it("resubscribes when an event comes from another host generation", async () => {
    const t = setup();
    await goLive(t);
    const ch = t.transport.channel;
    t.transport.feedEpoch = "e2";
    t.transport.answerFeed = false;
    ch.frame({ type: "host.event", epoch: "e2", event: { seq: 1, type: "terminal.removed", payload: removed("t") } });
    expect(subscribes(t)).toBe(2);
    expect(feedSummary(t)).toEqual(["snapshot@e1:0"]);
    ch.frame({ type: "host.snapshot", ...t.transport.snapshot() });
    t.transport.feedEvent("terminal.removed", removed("t"));
    expect(feedSummary(t)).toEqual(["snapshot@e1:0", "snapshot@e2:0", 1]);
  });

  it("starts over from a fresh snapshot after a reconnect or host restart", async () => {
    const t = setup();
    await goLive(t);
    t.transport.feedEvent("terminal.removed", removed("t"));
    t.transport.channel.drop();
    // Restarted host: new generation, numbering starts again.
    t.transport.feedEpoch = "e2";
    t.transport.feedSeq = 0;
    await vi.advanceTimersByTimeAsync(500);
    t.transport.channel.open();
    t.transport.feedEvent("terminal.removed", removed("t"));
    expect(feedSummary(t)).toEqual(["snapshot@e1:0", 1, "snapshot@e2:0", 1]);
    expect(t.transport.channels[1]!.sent).toEqual([{ op: "host.subscribe" }]);
  });

  it("reconnects when the host cannot start its feed", async () => {
    const t = setup();
    t.transport.answerFeed = false;
    await goLive(t);
    const ch = t.transport.channel;
    ch.frame({ type: "error", subscription_id: null, error: { code: "internal", message: "db" } });
    expect(ch.closed).toBe(true);
    expect(t.client.state).toMatchObject({ status: "reconnecting", error: { code: "internal" } });
  });
});
