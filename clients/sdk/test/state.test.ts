import { describe, expect, it } from "vitest";
import { historyCovers, initialSessionView, reduceSession, type SessionFrame, type SessionView } from "../src/state/session";
import { initialHostState, reduceHost } from "../src/state/store";
import {
  applyTerminalEvent,
  applyTerminalSnapshot,
  initialTerminalCursor,
  MAX_PENDING_EVENTS,
} from "../src/state/terminal";
import type { HistoryTurn, SessionEvent, SessionSnapshot, TerminalEvent } from "../src/types/protocol";

const ev = (seq: number, type: SessionEvent["type"], payload: unknown): SessionEvent =>
  ({ session_id: "s1", seq, type, payload }) as SessionEvent;

const baseSnapshot: SessionSnapshot = {
  status: "prompting",
  live_message: { message_id: "m1", role: "assistant", content: [{ type: "text", text: "Hel" }] },
  tool_calls: [{ tool_call_id: "tc1", title: "Run ls", kind: "execute", status: "in_progress", content: null }],
  pending_request: null,
  pending_user_message: { message_id: "u1", blocks: [{ type: "text", text: "hi" }] },
  last_error: null,
  usage: { used: 10, size: 100 },
  options: null,
  commands: [],
};

const run = (frames: SessionFrame[], view: SessionView = initialSessionView("s1")) =>
  frames.reduce(reduceSession, view);

describe("session view", () => {
  it("rebuilds the live turn from a snapshot plus later events", () => {
    const v = run([
      { type: "attaching" },
      { type: "snapshot", epoch: "e1", seq: 5, snapshot: baseSnapshot },
      { type: "event", event: ev(6, "agent.message_delta", { text: "lo" }) },
      { type: "event", event: ev(7, "agent.tool_call_update", { tool_call_id: "tc1", title: null, status: "completed", content: "ok" }) },
      { type: "event", event: ev(9, "agent.thought_delta", { text: "hmm" }) },
    ]);
    expect(v.attach).toBe("live");
    expect(v.epoch).toBe("e1");
    expect(v.seq).toBe(9);
    expect(v.live.userMessage?.message_id).toBe("u1");
    expect(v.live.message?.content).toEqual([
      { type: "text", text: "Hello" },
      { type: "tool_call", tool_call_id: "tc1" },
      { type: "thought", text: "hmm" },
    ]);
    expect(v.live.toolCalls.tc1).toEqual({
      tool_call_id: "tc1",
      title: "Run ls",
      kind: "execute",
      status: "completed",
      content: "ok",
    });
  });

  it("ignores events at or below the cursor", () => {
    const v1 = run([{ type: "snapshot", epoch: "e1", seq: 5, snapshot: baseSnapshot }]);
    const v2 = reduceSession(v1, { type: "event", event: ev(5, "agent.message_delta", { text: "X" }) });
    expect(v2).toBe(v1);
  });

  it("applies a replay and advances to its high-water mark", () => {
    const v = run([
      { type: "snapshot", epoch: "e1", seq: 5, snapshot: baseSnapshot },
      {
        type: "replay",
        epoch: "e1",
        events: [ev(6, "agent.message_delta", { text: "lo" }), ev(8, "agent.message_delta", { text: "!" })],
        high_water_seq: 11,
      },
    ]);
    expect(v.live.message?.content[0]).toEqual({ type: "text", text: "Hello!" });
    expect(v.seq).toBe(11);
  });

  it("a new epoch's snapshot replaces the live turn", () => {
    const v = run([
      { type: "snapshot", epoch: "e1", seq: 50, snapshot: baseSnapshot },
      {
        type: "snapshot",
        epoch: "e2",
        seq: 2,
        snapshot: { ...baseSnapshot, live_message: null, tool_calls: [], pending_user_message: null, status: "connected" },
      },
      { type: "event", event: ev(3, "agent.message_delta", { text: "new" }) },
    ]);
    expect(v.epoch).toBe("e2");
    expect(v.live.message?.content).toEqual([{ type: "text", text: "new" }]);
    expect(v.live.toolCalls).toEqual({});
    expect(v.agentStatus).toBe("connected");
  });

  it("tracks requests until resolved", () => {
    const req = { kind: "permission", request_id: "r1", title: "Run rm", options: [{ option_id: "allow", name: "Allow", kind: "allow_once" }] };
    let v = run([
      { type: "snapshot", epoch: "e1", seq: 1, snapshot: baseSnapshot },
      { type: "event", event: ev(2, "agent.request", req) },
    ]);
    expect(v.pendingRequest).toEqual(req);
    v = reduceSession(v, { type: "event", event: ev(3, "agent.request_resolved", { kind: "permission", request_id: "other" }) });
    expect(v.pendingRequest).toEqual(req);
    v = reduceSession(v, { type: "event", event: ev(4, "agent.request_resolved", { kind: "permission", request_id: "r1" }) });
    expect(v.pendingRequest).toBeNull();
  });

  it("keeps a completed turn on screen until history includes it", () => {
    let v = run([
      { type: "snapshot", epoch: "e1", seq: 1, snapshot: baseSnapshot },
      { type: "history", turns: [] },
      { type: "event", event: ev(2, "agent.turn_complete", { stop_reason: "end_turn" }) },
    ]);
    expect(v.live.message).toBeNull();
    expect(v.settled?.message?.content[0]).toEqual({ type: "text", text: "Hel" });
    expect(v.historyStale).toBe(true);
    expect(v.lastStopReason).toBe("end_turn");
    const turns = [{ turn_id: "t1", role: "user", blocks: [], timestamp: "", completed_at: null, duration_ms: null, model: null, usage: null }];
    v = reduceSession(v, { type: "history", turns });
    expect(v.settled).toBeNull();
    expect(v.historyStale).toBe(false);
    expect(v.history).toEqual(turns);
  });

  it("a user message starts a fresh live turn", () => {
    const v = run([
      { type: "snapshot", epoch: "e1", seq: 1, snapshot: { ...baseSnapshot, live_message: null, tool_calls: [], pending_user_message: null } },
      { type: "event", event: ev(2, "agent.user_message", { message_id: "u2", blocks: [{ type: "text", text: "next" }] }) },
      { type: "event", event: ev(3, "agent.message_delta", { text: "ok" }) },
    ]);
    expect(v.live.userMessage?.message_id).toBe("u2");
    expect(v.live.message?.content).toEqual([{ type: "text", text: "ok" }]);
  });

  it("tracks the agent's working state across turns without a new snapshot", () => {
    const idle = { ...baseSnapshot, status: "connected" as const, live_message: null, tool_calls: [], pending_user_message: null };
    let v = run([{ type: "snapshot", epoch: "e1", seq: 1, snapshot: idle }]);
    expect(v.agentStatus).toBe("connected");
    v = reduceSession(v, { type: "event", event: ev(2, "agent.user_message", { message_id: "u2", blocks: [] }) });
    expect(v.agentStatus).toBe("prompting");
    v = reduceSession(v, { type: "event", event: ev(3, "agent.turn_complete", { stop_reason: "end_turn" }) });
    expect(v.agentStatus).toBe("connected");
    // A request only arrives mid-turn.
    v = reduceSession(v, {
      type: "event",
      event: ev(4, "agent.request", { kind: "plan_approval", request_id: "p", plan_markdown: "" }),
    });
    expect(v.agentStatus).toBe("prompting");
  });

  it("detached (agent gone) clears live state and forgets the cursor", () => {
    const v = run([
      { type: "snapshot", epoch: "e1", seq: 5, snapshot: baseSnapshot },
      { type: "detached", reason: "session_not_live" },
    ]);
    expect(v).toMatchObject({ attach: "detached", epoch: null, seq: null, pendingRequest: null, historyStale: true });
    expect(v.live.message).toBeNull();
    expect(v.settled?.userMessage?.message_id).toBe("u1");
  });

  it("records attach errors and session status", () => {
    let v = run([{ type: "error", error: { code: "not_found", message: "Session not found" } }]);
    expect(v.attach).toBe("error");
    v = run([
      { type: "snapshot", epoch: "e1", seq: 1, snapshot: baseSnapshot },
      { type: "event", event: ev(2, "session.status", { status: "pending_review" }) },
      { type: "event", event: ev(3, "agent.error", { message: "boom", code: null }) },
    ]);
    expect(v.sessionStatus).toBe("pending_review");
    expect(v.lastError).toEqual({ message: "boom", code: null });
  });
});

describe("tool call images", () => {
  it("appends a message image to the live message", () => {
    const shot = { name: "ab.png", mime_type: "image/png", path: ".zwiim/images/ab.png" };
    let v = run([{ type: "snapshot", epoch: "e1", seq: 0, snapshot: { ...baseSnapshot, live_message: null, tool_calls: [] } }]);
    v = reduceSession(v, { type: "event", event: ev(1, "agent.message_delta", { text: "look:" }) });
    v = reduceSession(v, { type: "event", event: ev(2, "agent.message_image", shot) });
    v = reduceSession(v, { type: "event", event: ev(3, "agent.message_delta", { text: " done" }) });
    expect(v.live.message!.content).toEqual([
      { type: "text", text: "look:" },
      { type: "agent_image", ...shot },
      { type: "text", text: " done" },
    ]);
  });

  it("keeps a tool call's images across updates that carry none", () => {
    const shot = { name: "ab.png", mime_type: "image/png", path: ".zwiim/images/ab.png" };
    let v = run([{ type: "snapshot", epoch: "e1", seq: 0, snapshot: { ...baseSnapshot, tool_calls: [] } }]);
    v = reduceSession(v, { type: "event", event: ev(1, "agent.tool_call", { tool_call_id: "t1", title: "Read", kind: "read", status: "in_progress", content: null }) });
    v = reduceSession(v, { type: "event", event: ev(2, "agent.tool_call_update", { tool_call_id: "t1", title: null, status: "in_progress", content: null, images: [shot] }) });
    v = reduceSession(v, { type: "event", event: ev(3, "agent.tool_call_update", { tool_call_id: "t1", title: null, status: "completed", content: null }) });
    expect(v.live.toolCalls.t1!.images).toEqual([shot]);
  });
});

describe("historyCovers", () => {
  const turn = (role: string, text: string): HistoryTurn =>
    ({ turn_id: role + text, role, blocks: [{ type: "text", text }], timestamp: "", completed_at: null, duration_ms: null, model: null, usage: null });
  const settled = (text: string, reply: boolean) => ({
    userMessage: { message_id: "m", blocks: [{ type: "text" as const, text }] },
    message: reply ? { message_id: null, role: "assistant", content: [{ type: "text" as const, text: "ok" }] } : null,
    toolCalls: {},
  });

  it("needs the user turn and, when the agent replied, a reply after it", () => {
    expect(historyCovers([], settled("hi", true))).toBe(false);
    expect(historyCovers([turn("user", "hi")], settled("hi", true))).toBe(false);
    expect(historyCovers([turn("user", "hi"), turn("assistant", "ok")], settled("hi", true))).toBe(true);
    expect(historyCovers([turn("user", "hi")], settled("hi", false))).toBe(true);
  });

  it("matches the latest occurrence of a repeated prompt", () => {
    const turns = [turn("user", "again"), turn("assistant", "1"), turn("user", "again")];
    expect(historyCovers(turns, settled("again", true))).toBe(false);
  });
});

describe("terminal cursor", () => {
  const out = (seq: number, data: string): TerminalEvent => ({
    terminal_id: "t1",
    seq,
    type: "terminal.output",
    payload: { data },
  });

  it("buffers until the snapshot then drops what it already covers", () => {
    let c = initialTerminalCursor();
    c = applyTerminalEvent(c, out(3, "c")).cursor;
    c = applyTerminalEvent(c, out(4, "d")).cursor;
    const step = applyTerminalSnapshot(c, { terminal_id: "t1", alive: true, data: "abc", seq: 3 });
    expect(step.outputs).toEqual([{ kind: "reset", data: "abc" }, { kind: "append", data: "d" }]);
    expect(step.cursor.seq).toBe(4);
  });

  it("detects a gap in buffered output", () => {
    let c = initialTerminalCursor();
    c = applyTerminalEvent(c, out(7, "x")).cursor;
    const step = applyTerminalSnapshot(c, { terminal_id: "t1", alive: true, data: "abc", seq: 3 });
    expect(step.outputs).toEqual([{ kind: "reset", data: "abc" }, { kind: "resync" }]);
    expect(step.cursor.synced).toBe(false);
  });

  it("resyncs after the pending buffer overflows", () => {
    let c = initialTerminalCursor();
    for (let i = 1; i <= MAX_PENDING_EVENTS + 1; i++) c = applyTerminalEvent(c, out(i, ".")).cursor;
    expect(c.overflowed).toBe(true);
    const step = applyTerminalSnapshot(c, { terminal_id: "t1", alive: true, data: "", seq: 1 });
    expect(step.outputs).toEqual([{ kind: "resync" }]);
  });

  it("applies unnumbered chunks and marks a dead terminal", () => {
    let c = applyTerminalSnapshot(initialTerminalCursor(), { terminal_id: "t1", alive: true, data: "", seq: 0 }).cursor;
    expect(applyTerminalEvent(c, out(0, "?")).outputs).toEqual([{ kind: "append", data: "?" }]);
    c = applyTerminalEvent(c, out(1, "a")).cursor;
    const gone = applyTerminalSnapshot(c, { terminal_id: "t1", alive: false, data: "", seq: 0 });
    expect(gone.outputs).toEqual([{ kind: "exit" }]);
    expect(gone.cursor.alive).toBe(false);
  });
});

describe("host store", () => {
  it("dedupes an added (revived) workspace", () => {
    const w = {
      workspace_id: "w1",
      name: "a",
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
    let s = reduceHost(initialHostState(), { type: "workspaces", workspaces: [w] });
    s = reduceHost(s, { type: "workspace_added", workspace: { ...w, name: "b" } });
    expect(s.workspaces).toEqual([{ ...w, name: "b" }]);
  });

  it("keeps an attached terminal that left the host list, as ended", () => {
    let s = reduceHost(initialHostState(), { type: "terminals", terminals: [{ terminal_id: "t1", title: "zsh", workspace_id: null }] });
    s = reduceHost(s, { type: "terminal", terminal_id: "t1", patch: { attached: true } });
    s = reduceHost(s, { type: "terminals", terminals: [] });
    expect(s.terminals.t1).toMatchObject({ alive: false, attached: true });
  });

  it("routes session frames into per-session views", () => {
    const s = reduceHost(initialHostState(), {
      type: "session_frame",
      session_id: "s9",
      frame: { type: "attaching" },
    });
    expect(s.sessions.s9?.attach).toBe("attaching");
  });
});

describe("history generation and background work", () => {
  const hist: HistoryTurn[] = [];
  const snap = (extra: Partial<SessionSnapshot>): SessionFrame => ({
    type: "snapshot",
    epoch: "e1",
    seq: 5,
    snapshot: { ...baseSnapshot, ...extra },
  });

  it("a snapshot at the generation of the history on screen keeps it current", () => {
    const v = run([{ type: "history", turns: hist, generation: 7 }, snap({ history_generation: 7 })]);
    expect(v.historyGeneration).toBe(7);
    expect(v.historyStale).toBe(false);
  });

  it("a snapshot at another generation, or none, or before any history, marks it stale", () => {
    // A turn finished (or the chat was rewound) while away.
    expect(run([{ type: "history", turns: hist, generation: 7 }, snap({ history_generation: 8 })]).historyStale).toBe(true);
    // An older Host does not say.
    expect(run([{ type: "history", turns: hist, generation: 7 }, snap({})]).historyStale).toBe(true);
    // Nothing loaded yet.
    expect(run([snap({ history_generation: 7 })]).historyStale).toBe(true);
    // A history read without a generation (no agent attached then).
    expect(run([{ type: "history", turns: hist }, snap({ history_generation: 7 })]).historyStale).toBe(true);
  });

  it("a rewind snapshot still reloads history", () => {
    // The Host bumps the generation when a chat rewind forks the session;
    // the snapshot that follows the rewind carries the new one.
    let v = run([{ type: "history", turns: hist, generation: 7 }, snap({ history_generation: 7 })]);
    expect(v.historyStale).toBe(false);
    v = reduceSession(v, { type: "snapshot", epoch: "e1", seq: 9, snapshot: { ...baseSnapshot, live_message: null, history_generation: 8 } });
    expect(v.historyStale).toBe(true);
  });

  it("an older snapshot without the new fields leaves background unknown", () => {
    const v = run([snap({})]);
    expect(v.background).toBeNull();
  });

  it("takes background work from the snapshot and session.background events", () => {
    const background = {
      outstanding: 1,
      tasks: [{ task_id: "task-0123456789abcdef", label: "Run the dev server", state: "running", tool_call_id: "tc9" }],
      waiting_since: "2026-10-19T10:00:00Z",
    };
    let v = run([snap({ background, history_generation: 1 })]);
    expect(v.background).toEqual(background);
    v = reduceSession(v, {
      type: "event",
      event: ev(6, "session.background", { outstanding: 0, tasks: [], waiting_since: null }),
    });
    expect(v.background).toEqual({ outstanding: 0, tasks: [], waiting_since: null });
    // The agent going away takes its background work with it.
    v = reduceSession(reduceSession(v, { type: "event", event: ev(7, "session.background", background) }), {
      type: "detached",
      reason: "session_not_live",
    });
    expect(v.background).toBeNull();
  });

  it("a session.background generation unlike the history on screen marks it stale", () => {
    const idle = { outstanding: 0, tasks: [], waiting_since: null };
    let v = run([{ type: "history", turns: hist, generation: 7 }, snap({ history_generation: 7 })]);
    // At the generation on screen: still current, and the generation is not
    // kept as part of the background work.
    v = reduceSession(v, { type: "event", event: ev(6, "session.background", { ...idle, history_generation: 7 }) });
    expect(v.historyStale).toBe(false);
    expect(v.background).toEqual(idle);
    // Turns landed out of turn: the Host moved the generation with no
    // turn_complete.
    v = reduceSession(v, { type: "event", event: ev(7, "session.background", { ...idle, history_generation: 8 }) });
    expect(v.historyStale).toBe(true);
    expect(v.background).toEqual(idle);
    // An older Host's event says nothing about history.
    v = run([{ type: "history", turns: hist, generation: 7 }, snap({ history_generation: 7 })]);
    v = reduceSession(v, { type: "event", event: ev(6, "session.background", idle) });
    expect(v.historyStale).toBe(false);
  });

  it("a turn with only sub-agent work is not empty", () => {
    const v = run([
      { type: "snapshot", epoch: "e1", seq: 1, snapshot: { ...baseSnapshot, live_message: null, tool_calls: [], pending_user_message: null } },
      { type: "event", event: ev(2, "agent.subagent_message_delta", { parent_id: "launch", text: "child works" }) },
      { type: "event", event: ev(3, "agent.turn_complete", { stop_reason: "end_turn" }) },
    ]);
    expect(v.settled?.subagents).toEqual({ launch: { blocks: [{ type: "text", text: "child works" }] } });
  });
});
