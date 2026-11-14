import {
  CLIENT_CAPABILITIES,
  clockTime,
  decodeServerFrame,
  evaluateCompatibility,
  FORK_BUSY_NOTE,
  FORK_OFFLINE_NOTE,
  FORK_OLDER_TURN_NOTE,
  initialSessionView,
  offersChatRewind,
  reduceSession,
  ZwiimClientError,
  type ForkTarget,
  type HistoryTurn,
  type SessionEvent,
  type SessionFrame,
  type SessionView,
  type WorkspaceInfo,
  waitingText,
} from "@zwiim/sdk";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FakeChannel } from "../../sdk/test/fakes";
import { parseInline, parseMarkdown, spanText } from "../src/sessions/markdown";
import {
  clock,
  duration,
  historyBefore,
  nowState,
  runSummary,
  subagentShown,
  subagentTime,
  transcriptBuilder,
  transcriptRows,
  turnKey,
  subagentLabel,
  turnRunning,
  type SubagentItem,
  type TranscriptRow,
} from "../src/sessions/transcript";
import { canReset, oneReset, revertCopy, revertError } from "../src/sessions/revert";
import { forkDisabledNote, forkError, forkNote, forkedFromLabel, forkedSendError, forkWorktreeLines } from "../src/sessions/fork";
import { waitFor, world } from "./support";

// ─── building views the way the SDK does ─────────────────────────────────

const snapshot = (status: "connected" | "prompting" = "connected"): SessionFrame => ({
  type: "snapshot",
  epoch: "e1",
  seq: 1,
  snapshot: {
    status,
    live_message: null,
    tool_calls: [],
    pending_request: null,
    pending_user_message: null,
    last_error: null,
    usage: null,
    options: null,
    commands: [],
  },
});

let seq = 1;
const ev = <E extends SessionEvent>(type: E["type"], payload: E["payload"]): SessionFrame => ({
  type: "event",
  event: { session_id: "s1", seq: ++seq, type, payload } as SessionEvent,
});

const user = (id: string, text: string) => ev("agent.user_message", { message_id: id, blocks: [{ type: "text", text }] });
const delta = (text: string) => ev("agent.message_delta", { text });
const thought = (text: string) => ev("agent.thought_delta", { text });
// As the SDK hands them over: every spelling of running decoded to `in_progress`.
const tool = (id: string, title: string, kind: string, status = "in_progress", content: unknown = null) =>
  ev("agent.tool_call", { tool_call_id: id, title, kind, status, content });
const toolDone = (id: string, status = "completed", content: unknown = null) =>
  ev("agent.tool_call_update", { tool_call_id: id, title: null, status, content });
const done = (stop_reason: string | null = "end_turn") => ev("agent.turn_complete", { stop_reason });

/** Events are numbered as they are made, so frames reused later replay as duplicates. */
function view(...frames: SessionFrame[]): SessionView {
  return frames.reduce(reduceSession, reduceSession(initialSessionView("s1"), snapshot()));
}
const play = (v: SessionView, ...frames: SessionFrame[]) => frames.reduce(reduceSession, v);

const turn = (id: string, role: string, blocks: HistoryTurn["blocks"], at = "2026-09-26T10:00:00Z", end?: string): HistoryTurn => ({
  turn_id: id,
  role,
  blocks,
  timestamp: at,
  completed_at: end ?? null,
  duration_ms: null,
  model: null,
  usage: null,
} as unknown as HistoryTurn);

const history = (turns: HistoryTurn[]): SessionFrame => ({ type: "history", turns });
const types = (v: SessionView) => transcriptRows(v).map((r) => r.type);

// ─── sub-agents ──────────────────────────────────────────────────────────

describe("sub-agent cards", () => {
  const launch = (id: string, subagent: Record<string, unknown>, status = "in_progress") =>
    ev("agent.tool_call", { tool_call_id: id, title: "Agent", kind: "other", status, content: null, subagent } as never);
  const childTool = (id: string, parent: string, title: string) =>
    ev("agent.tool_call", { tool_call_id: id, title, kind: "read", status: "completed", content: null, parent_id: parent } as never);
  const background = (outstanding: number, waiting_since: string | null, tool_call_id?: string) =>
    ev("session.background", {
      outstanding,
      tasks: outstanding ? [{ task_id: "t1", label: "Review", state: "running", ...(tool_call_id ? { tool_call_id } : {}) }] : [],
      waiting_since,
    });
  const cards = (v: SessionView) =>
    transcriptRows(v).filter((r): r is Extract<TranscriptRow, { type: "subagent" }> => r.type === "subagent");
  const agentOf = (v: SessionView, i = 0): SubagentItem => cards(v)[i]!.agent;

  /** A turn with two sub-agents; the first thinks, speaks and runs three tools. */
  const twoSubagents = () =>
    view(
      user("u1", "go"),
      delta("Delegating."),
      launch("a1", { description: "Map client add-workspace code", model: "claude-sonnet-5-5", prompt: "Find it" }),
      ev("agent.subagent_thought_delta", { parent_id: "a1", text: "child plans" }),
      ev("agent.subagent_message_delta", { parent_id: "a1", text: "child found it" }),
      childTool("r1", "a1", "Read src/a.ts"),
      childTool("r2", "a1", "Read src/b.ts"),
      childTool("r3", "a1", "Read src/c.ts"),
      launch("a2", { description: "Review the diff" }),
    );

  it("is one row per sub-agent at its launch, in launch order, holding what it did", () => {
    const v = twoSubagents();
    expect(types(v)).toEqual(["user", "agent", "subagent", "subagent"]);
    const first = agentOf(v);
    expect(first).toMatchObject({
      model: "Sonnet 5.5",
      description: "Map client add-workspace code",
      state: "running",
      duration: null,
      tools: 3,
      prompt: "Find it",
    });
    expect(agentOf(v, 1)).toMatchObject({ model: null, description: "Review the diff", tools: 0, prompt: null });
    // Its three tool calls fold inside the card; none of it is a top-level row.
    expect(first.rows.map((r) => r.type)).toEqual(["thought", "agent", "tools"]);
    expect(first.rows[1]).toMatchObject({ text: "child found it" });
    const top = transcriptRows(v);
    expect(top.some((r) => r.type === "agent" && r.text.includes("child"))).toBe(false);
    expect(top.some((r) => r.type === "thought" || r.type === "tool" || r.type === "tools")).toBe(false);
    // Keyed by launch order within the turn, never by id.
    expect(cards(v).map((c) => c.key)).toEqual(["live:u1:agent0", "live:u1:agent1"]);
  });

  it("shows nothing while shut; open, its prompt behind a toggle and then what it did", () => {
    const agent = agentOf(twoSubagents());
    expect(subagentShown(agent, false, true)).toEqual({ promptToggle: false, prompt: null, rows: [] });
    expect(subagentShown(agent, true, false)).toEqual({ promptToggle: true, prompt: null, rows: agent.rows });
    expect(subagentShown(agent, true, true).prompt).toBe("Find it");
    // No prompt, no toggle.
    expect(subagentShown(agentOf(twoSubagents(), 1), true, true).promptToggle).toBe(false);
  });

  it("reads done with its duration, or failed", () => {
    const v = play(
      twoSubagents(),
      ev("agent.tool_call_update", { tool_call_id: "a1", title: null, status: "completed", content: null, subagent: { duration_ms: 106_400 } } as never),
      toolDone("a2", "failed"),
      done(),
    );
    expect(agentOf(v)).toMatchObject({ state: "done", duration: "1m 46s" });
    expect(agentOf(v, 1)).toMatchObject({ state: "failed", duration: null });
  });

  it("reads running while its background task is outstanding, after its turn ends", () => {
    let v = view(user("u1", "go"), launch("a1", { description: "Review" }, "completed"), done());
    expect(agentOf(v).state).toBe("done");
    v = play(v, background(1, "2026-10-19T10:00:00Z", "a1"));
    expect(agentOf(v).state).toBe("running");
  });

  it("builds the same card from a snapshot after a reconnect", () => {
    const v = reduceSession(initialSessionView("s1"), {
      ...snapshot("prompting"),
      snapshot: {
        ...(snapshot("prompting") as Extract<SessionFrame, { type: "snapshot" }>).snapshot,
        live_message: {
          message_id: "m1",
          role: "assistant",
          content: [{ type: "text", text: "Delegating." }, { type: "tool_call", tool_call_id: "a1" }],
          subagents: [{ parent_id: "a1", blocks: [{ type: "text", text: "child found it" }, { type: "tool_call", tool_call_id: "r1" }] }],
        },
        tool_calls: [
          { tool_call_id: "a1", title: "Agent", kind: "other", status: "in_progress", content: null, subagent: { description: "Map", model: "claude-sonnet-5-5" } },
          { tool_call_id: "r1", title: "Read src/a.ts", kind: "read", status: "completed", content: null, parent_id: "a1" },
        ],
      } as never,
    } as SessionFrame);
    expect(types(v)).toEqual(["agent", "subagent"]);
    expect(agentOf(v)).toMatchObject({ model: "Sonnet 5.5", description: "Map", state: "running", tools: 1 });
    expect(agentOf(v).rows.map((r) => r.type)).toEqual(["agent", "tool"]);
  });

  it("builds cards from history, nesting a sub-agent's own sub-agents", () => {
    const v = view(
      history([
        turn("t0", "user", [{ type: "text", text: "go" }]),
        turn("t1", "assistant", [
          {
            type: "tool_call",
            tool_call_id: "a1",
            title: "Task",
            input_preview: null,
            status: null,
            subagent: {
              description: "Survey the repo",
              model: "claude-opus-4-1-20250805",
              duration_ms: 42_000,
              tokens: 75_223,
              blocks: [
                { type: "text", text: "surveying" },
                { type: "tool_call", tool_call_id: "b1", title: "Bash", input_preview: JSON.stringify({ command: "ls" }), status: null },
                { type: "tool_result", tool_call_id: "b1", output_preview: "a.ts", is_error: false },
                { type: "tool_call", tool_call_id: "a2", title: "Task", input_preview: null, status: "completed", subagent: { description: "Dig deeper", blocks: [] } },
              ],
            },
          },
          { type: "tool_result", tool_call_id: "a1", output_preview: "done", is_error: false },
          { type: "text", text: "All done." },
        ] as never),
      ]),
    );
    expect(types(v)).toEqual(["user", "subagent", "agent", "footer"]);
    const agent = agentOf(v);
    expect(agent).toMatchObject({
      model: "Opus 4.1",
      description: "Survey the repo",
      state: "done",
      duration: "42s",
      tools: 2,
      tokens: "75k tokens",
    });
    expect(subagentLabel(agent)).toBe("Agent, Opus 4.1, Survey the repo, 2 tools, 75k tokens, 42s, done");
    // Still running (background work): the screen ticks, so the label drops the old duration.
    expect(subagentLabel({ ...agent, state: "running" })).toBe(
      "Agent, Opus 4.1, Survey the repo, 2 tools, 75k tokens, running",
    );
    expect(agent.rows.map((r) => r.type)).toEqual(["agent", "tool", "subagent"]);
    expect(agent.rows[1]).toMatchObject({ tool: { verb: "Run", target: "ls", state: "done" } });
    const nested = agent.rows[2] as Extract<TranscriptRow, { type: "subagent" }>;
    expect(nested.agent).toMatchObject({ description: "Dig deeper", state: "done" });
    expect(nested.key).toBe("t1:agent0:agent0");
  });

  it("nests a sub-agent's own sub-agents as cards in a live turn", () => {
    const v = play(
      twoSubagents(),
      ev("agent.tool_call", { tool_call_id: "a3", title: "Agent", kind: "other", status: "in_progress", content: null, parent_id: "a1", subagent: { description: "Dig deeper" } } as never),
      ev("agent.subagent_message_delta", { parent_id: "a3", text: "deep result" }),
    );
    expect(types(v)).toEqual(["user", "agent", "subagent", "subagent"]);
    const first = agentOf(v);
    expect(first.tools).toBe(4);
    // It folds with the three tool calls before it.
    expect(first.rows.map((r) => r.type)).toEqual(["thought", "agent", "tools"]);
    const run = first.rows.at(-1)!;
    const nested = (run.type === "tools" ? run.rows : []).find(
      (r): r is Extract<TranscriptRow, { type: "subagent" }> => r.type === "subagent",
    )!;
    expect(nested.key).toBe("live:u1:agent0:agent0");
    expect(nested.agent).toMatchObject({ description: "Dig deeper", state: "running" });
    expect(nested.agent.rows).toEqual([expect.objectContaining({ type: "agent", text: "deep result" })]);
    expect(transcriptRows(v).some((r) => r.type === "agent" && r.text === "deep result")).toBe(false);
  });

  it("puts a sub-agent whose launch is not in the reply last, so nothing it did is hidden", () => {
    const v = view(
      user("u1", "go"),
      delta("Working on it."),
      ev("agent.subagent_message_delta", { parent_id: "x9", text: "orphan says" }),
      launch("a1", { description: "Review the diff" }),
    );
    expect(types(v)).toEqual(["user", "agent", "subagent", "subagent"]);
    expect(agentOf(v).description).toBe("Review the diff");
    const orphan = agentOf(v, 1);
    // Nothing is known of it but what it did: no description, no status.
    expect(orphan).toMatchObject({ model: null, description: null, state: "idle", tools: 0 });
    expect(orphan.rows).toEqual([expect.objectContaining({ type: "agent", text: "orphan says" })]);
    expect(cards(v)[1]!.key).toBe("live:u1:agent1");
    expect(subagentLabel(orphan)).toBe("Agent");
  });

  it("describes a sub-agent by its launch's title only when that says more than Agent", () => {
    const v = view(
      user("u1", "go"),
      launch("a1", {}),
      ev("agent.tool_call", { tool_call_id: "a2", title: "`Survey the repo`", kind: "other", status: "in_progress", content: null, subagent: {} } as never),
    );
    expect(agentOf(v).description).toBeNull();
    expect(agentOf(v, 1).description).toBe("Survey the repo");
  });

  it("says a card's status to a screen reader only when it is known", () => {
    const v = play(
      twoSubagents(),
      ev("agent.tool_call_update", { tool_call_id: "a1", title: null, status: "completed", content: null, subagent: { duration_ms: 106_400 } } as never),
    );
    expect(subagentLabel(agentOf(v))).toBe("Agent, Sonnet 5.5, Map client add-workspace code, 3 tools, 1m 46s, done");
    expect(subagentLabel(agentOf(v, 1))).toBe("Agent, Review the diff, running");
    expect(subagentLabel({ ...agentOf(v, 1), state: "idle" })).toBe("Agent, Review the diff");
  });

  it("shows the images its launch returned after what it did, live, but not the launch's text", () => {
    const v = play(
      twoSubagents(),
      ev("agent.tool_call_update", {
        tool_call_id: "a1",
        title: null,
        status: "completed",
        content: "the launch's own text",
        images: [{ name: "shot.png", mime_type: "image/png", path: ".zwiim/images/shot.png" }],
      } as never),
    );
    expect(types(v)).toEqual(["user", "agent", "subagent", "subagent"]);
    const rows = agentOf(v).rows;
    expect(rows.map((r) => r.type)).toEqual(["thought", "agent", "tools", "image"]);
    expect(rows.at(-1)).toMatchObject({ key: "live:u1:agent0:img0", image: { name: "shot.png" } });
    expect(JSON.stringify(transcriptRows(v))).not.toContain("the launch's own text");
  });

  it("shows the images its launch returned after what it did, from history, but not the launch's text", () => {
    const v = view(
      history([
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "a1", title: "Task", input_preview: null, status: null, subagent: { description: "Draw it", blocks: [{ type: "text", text: "drew it" }] } },
          {
            type: "tool_result",
            tool_call_id: "a1",
            output_preview: "the launch's own text",
            is_error: false,
            images: [{ name: "plot.png", mime_type: "image/png", path: ".zwiim/images/plot.png" }],
          },
        ] as never),
      ]),
    );
    expect(types(v)).toEqual(["subagent"]);
    const rows = agentOf(v).rows;
    expect(rows.map((r) => r.type)).toEqual(["agent", "image"]);
    expect(rows[1]).toMatchObject({ key: "t1:agent0:img0", image: { name: "plot.png" } });
    expect(JSON.stringify(transcriptRows(v))).not.toContain("the launch's own text");
  });

  it("keeps an older Host's agent launch as a plain tool row", () => {
    const v = view(
      history([
        turn("t1", "assistant", [
          { type: "tool_call", tool_call_id: "a1", title: "Task", input_preview: JSON.stringify({ description: "Survey" }), status: null },
          { type: "tool_result", tool_call_id: "a1", output_preview: "done", is_error: false },
        ] as never),
      ]),
    );
    expect(types(v)).toEqual(["tool"]);
    const live = view(user("u1", "go"), tool("a1", "Agent", "other"));
    expect(types(live)).toEqual(["user", "tool"]);
  });

  it("reads a sub-agent its finished turn never heard back from as interrupted", () => {
    const unanswered = history([
      turn("t0", "user", [{ type: "text", text: "go" }]),
      turn("t1", "assistant", [
        { type: "tool_call", tool_call_id: "a1", title: "Task", input_preview: null, status: null, subagent: { description: "Survey", blocks: [] } },
      ] as never),
    ]);
    const v = view(unanswered);
    expect(agentOf(v).state).toBe("interrupted");
    expect(subagentLabel(agentOf(v))).toBe("Agent, Survey, interrupted");
    // While the agent may still be answering that prompt, it is not.
    expect(agentOf(reduceSession(view(unanswered), snapshot("prompting"))).state).toBe("idle");
  });

  it("shows a background sub-agent's work after a reload on its card in the history", () => {
    const build = transcriptBuilder();
    const past = history([
      turn("t0", "user", [{ type: "text", text: "go" }]),
      turn("t1", "assistant", [
        { type: "tool_call", tool_call_id: "a1", title: "Task", input_preview: null, status: "completed", subagent: { description: "Review", blocks: [{ type: "text", text: "started" }] } },
        { type: "tool_result", tool_call_id: "a1", output_preview: "launched", is_error: false, background: null },
      ] as never),
    ]);
    let v = play(view(past), background(1, null, "a1"), ev("agent.subagent_message_delta", { parent_id: "a1", text: "found more" }));
    let rows = build(v);
    expect(rows.map((r) => r.type)).toEqual(["user", "subagent", "footer"]);
    let agent = cards(v)[0]!.agent;
    expect(agent.state).toBe("running");
    expect(agent.rows.map((r) => (r.type === "agent" ? r.text : r.type))).toEqual(["started", "found more"]);
    // Its later work redraws the card, from the same builder.
    v = play(v, childTool("r1", "a1", "Read src/a.ts"));
    rows = build(v);
    agent = (rows[1] as Extract<TranscriptRow, { type: "subagent" }>).agent;
    expect(agent.rows.map((r) => r.type)).toEqual(["agent", "agent", "tool"]);
    expect(rows).toEqual(transcriptRows(v));
  });

  describe("clock", () => {
    beforeEach(() => {
      vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
      vi.setSystemTime(Date.parse("2026-10-24T10:01:05Z"));
    });
    afterEach(() => vi.useRealTimers());

    it("ticks a running sub-agent's time from when it started, then shows its duration once done", () => {
      let v = view(
        user("u1", "go"),
        launch("clock1", { description: "Survey", started_at: "2026-10-24T10:00:00.000Z" }),
      );
      let agent = agentOf(v);
      expect(agent.since).toBe(Date.parse("2026-10-24T10:00:00Z"));
      expect(subagentTime(agent, Date.now())).toBe("1m 5s");
      vi.advanceTimersByTime(2000);
      expect(subagentTime(agent, Date.now())).toBe("1m 7s");
      v = play(
        v,
        ev("agent.tool_call_update", {
          tool_call_id: "clock1",
          title: null,
          status: "completed",
          content: null,
          subagent: { duration_ms: 70_000 },
        } as never),
      );
      agent = agentOf(v);
      expect(agent).toMatchObject({ state: "done", since: null, duration: "1m 10s" });
      expect(subagentTime(agent, Date.now())).toBe("1m 10s");
      vi.advanceTimersByTime(5000);
      expect(subagentTime(agent, Date.now())).toBe("1m 10s");
    });

    it("counts from when it first saw the launch when the Host does not say, across a rebuild", () => {
      const v = view(
        user("u1", "go"),
        launch("clock2", { description: "Survey" }),
        launch("clock3", { description: "Ahead", started_at: "2026-10-24T10:05:00.000Z" }),
      );
      expect(agentOf(v).since).toBe(Date.now());
      expect(subagentTime(agentOf(v), Date.now())).toBe("0s");
      // A start ahead of this clock never reads below zero.
      expect(subagentTime(agentOf(v, 1), Date.now())).toBe("0s");
      vi.advanceTimersByTime(3000);
      expect(subagentTime(agentOf(v), Date.now())).toBe("3s");
      // Building its rows again keeps when it was first seen.
      expect(
        subagentTime(agentOf(view(user("u1", "go"), launch("clock2", { description: "Survey" }))), Date.now()),
      ).toBe("3s");
      // Not running and no duration: no time at all.
      expect(subagentTime(agentOf(play(v, toolDone("clock2", "failed"))), Date.now())).toBeNull();
    });
  });

  it("says what the idle agent waits on and since when, then clears", () => {
    expect(waitingText(1)).toBe("Waiting on 1 background task");
    expect(waitingText(3)).toBe("Waiting on 3 background tasks");
    let v = view(user("u1", "go"), done());
    expect(nowState(v, true)).toBeNull();
    v = play(v, background(2, "2026-10-19T10:00:00Z"));
    expect(nowState(v, true)).toEqual({ type: "waiting", count: 2, since: Date.parse("2026-10-19T10:00:00Z") });
    // A turn running again says what it does instead.
    expect(nowState(play(v, user("u2", "more")), true)).toMatchObject({ type: "working" });
    expect(nowState(play(v, background(2, null)), true)).toBeNull();
    expect(nowState(play(v, background(0, "2026-10-19T10:00:00Z")), true)).toBeNull();
    // A time it cannot read counts as not waiting, as on Desktop.
    expect(nowState(play(v, background(2, "not a time")), true)).toBeNull();
  });
});

// ─── the transcript ──────────────────────────────────────────────────────

describe("transcript", () => {
  it("renders history: messages, thoughts, tool calls with their results, and how the turn ended", () => {
    const v = view(
      history([
        turn("u1", "user", [{ type: "text", text: "Fix the build" }], "2026-09-26T10:00:00Z"),
        turn(
          "a1",
          "assistant",
          [
            { type: "thought", text: "Look at the error first." },
            { type: "tool_call", tool_call_id: "t1", title: "`cargo build`", input_preview: "cargo build", status: null },
            { type: "tool_result", tool_call_id: "t1", output_preview: "error[E0425]", is_error: true },
            { type: "text", text: "Fixed the import." },
          ],
          "2026-09-26T10:00:05Z",
          "2026-09-26T10:03:17Z",
        ),
      ]),
    );
    const rows = transcriptRows(v);
    expect(rows.map((r) => r.type)).toEqual(["user", "thought", "tool", "agent", "footer"]);
    const t = rows[2]!;
    expect(t.type === "tool" && t.tool).toMatchObject({ verb: null, target: "cargo build", state: "failed", output: "error[E0425]" });
    expect(rows.at(-1)).toMatchObject({ text: `3m 17s · ${clockTime("2026-09-26T10:03:17Z")}`, stopped: false, files: [] });
  });

  it("cuts a huge tool input and output short, so the sheet cannot freeze the app", () => {
    const huge = "x".repeat(1_000_000);
    const v = view(
      history([
        turn("a1", "assistant", [
          { type: "tool_call", tool_call_id: "t1", title: "Write", input_preview: huge, status: null },
          { type: "tool_result", tool_call_id: "t1", output_preview: huge, is_error: false },
        ]),
      ]),
    );
    const t = transcriptRows(v).find((r) => r.type === "tool");
    expect(t?.type === "tool" && t.tool.output).toBe(`${"x".repeat(20_000)}\n… 980,000 more characters`);
    expect(t?.type === "tool" && t.tool.input?.length).toBeLessThan(21_000);
  });

  it("reads a transcript's tool names and JSON input as a verb and a target", () => {
    const v = view(
      history([
        turn("u1", "user", [{ type: "text", text: "Test it" }]),
        turn("a1", "assistant", [
          { type: "tool_call", tool_call_id: "t1", title: "Bash", input_preview: '{"command":"cargo test","description":"Run tests"}', status: null },
          { type: "tool_result", tool_call_id: "t1", output_preview: "ok", is_error: false },
          { type: "tool_call", tool_call_id: "t2", title: "Read", input_preview: '{"file_path":"src/lib.rs"}', status: null },
          { type: "tool_call", tool_call_id: "t3", title: "TodoWrite", input_preview: '{"todos":[]}', status: null },
          { type: "tool_call", tool_call_id: "t4", title: "ExitPlanMode", input_preview: '{"plan":"Do it"}', status: null },
          { type: "tool_call", tool_call_id: "t5", title: "mcp__zwiim-mcp__update_progress", input_preview: '{"steps":[]}', status: null },
          { type: "tool_call", tool_call_id: "t6", title: "mcp__zwiim-mcp__browser_snapshot", input_preview: '{"tabId":"a"}', status: null },
          { type: "tool_call", tool_call_id: "t7", title: "mcp__zwiim-mcp__ask_user_question", input_preview: '{"questions":[]}', status: null },
        ]),
      ]),
    );
    const tools = transcriptRows(v).flatMap((r) =>
      r.type === "tools"
        ? r.rows.flatMap((t) => (t.type === "tool" ? [t.tool] : []))
        : r.type === "tool"
          ? [r.tool]
          : [],
    );
    expect(tools.map((t) => [t.verb, t.target, t.state, t.input])).toEqual([
      ["Run", "cargo test", "done", "cargo test"],
      ["Read", "src/lib.rs", "idle", '{\n  "file_path": "src/lib.rs"\n}'],
      [null, "TodoWrite", "idle", '{\n  "todos": []\n}'],
      // Its verb says it all, as it reads live.
      [null, "Exited Plan Mode", "idle", '{\n  "plan": "Do it"\n}'],
      // Zwiim's own tool reads by what it does, never by its MCP name.
      [null, "Updated tasks", "idle", '{\n  "steps": []\n}'],
      [null, "Read page", "idle", '{\n  "tabId": "a"\n}'],
      [null, "Ask", "idle", '{\n  "questions": []\n}'],
    ]);
  });

  it("streams the live turn, with only its last block still writing", () => {
    const v = view(user("m1", "Run the tests"), thought("Plan"), tool("t1", "cargo test", "execute"), delta("Running"));
    const rows = transcriptRows(v);
    expect(rows.map((r) => [r.type, "streaming" in r ? r.streaming : null])).toEqual([
      ["user", null],
      ["thought", false],
      ["tool", null],
      ["agent", true],
    ]);
    const t = rows[2]!;
    expect(t.type === "tool" && t.tool).toMatchObject({ verb: "Run", target: "cargo test", state: "running" });
  });

  it("names Zwiim's own tool by what it does while it runs", () => {
    const v = view(user("m1", "Plan it"), tool("t1", "mcp__zwiim-mcp__update_progress", "other"));
    const t = transcriptRows(v)[1]!;
    expect(t.type === "tool" && t.tool).toMatchObject({ verb: null, target: "Updated tasks", state: "running" });
  });

  it("folds three or more tool calls in a row", () => {
    const v = view(
      user("m1", "Look around"),
      tool("t1", "Read src/a.ts", "read", "completed"),
      tool("t2", "src/b.ts", "read", "completed"),
      tool("t3", "grep foo", "search", "failed"),
      delta("Found it"),
      tool("t4", "src/c.ts", "edit", "completed", "--- a/c\n+++ b/c\n-old\n+new\n+more"),
    );
    const rows = transcriptRows(v);
    expect(rows.map((r) => r.type)).toEqual(["user", "tools", "agent", "tool"]);
    const group = rows[1]!;
    expect(
      group.type === "tools" && group.rows.map((t) => t.type === "tool" && [t.tool.verb, t.tool.target, t.tool.state]),
    ).toEqual([
      [null, "Read src/a.ts", "done"],
      ["Read", "src/b.ts", "done"],
      ["Search", "grep foo", "failed"],
    ]);
    const edit = rows[3]!;
    expect(edit.type === "tool" && [edit.tool.added, edit.tool.removed]).toEqual([2, 1]);
  });

  it("folds sub-agents in with the tool calls, counting a failed one with the failed calls", () => {
    const launch = (id: string, status: string) =>
      ev("agent.tool_call", {
        tool_call_id: id,
        title: "Agent",
        kind: "other",
        status,
        content: null,
        subagent: { description: "Survey" },
      } as never);
    const v = view(
      user("m1", "Look around"),
      tool("t1", "Read src/a.ts", "read", "completed"),
      launch("a1", "failed"),
      tool("t2", "grep foo", "search", "failed"),
    );
    const rows = transcriptRows(play(v, delta("Found it")));
    expect(rows.map((r) => r.type)).toEqual(["user", "tools", "agent"]);
    const group = rows[1]!;
    if (group.type !== "tools") throw new Error("not folded");
    expect(group.rows.map((r) => r.type)).toEqual(["tool", "subagent", "tool"]);
    expect(runSummary(group.rows)).toEqual({ text: "2 tool calls, 1 sub-agent", failed: 2, running: false });
    // A running sub-agent keeps the fold running once its tool calls are done.
    const busy = transcriptRows(play(v, launch("a2", "in_progress"))).find((r) => r.type === "tools");
    expect(busy?.type === "tools" && runSummary(busy.rows)).toEqual({
      text: "2 tool calls, 2 sub-agents",
      failed: 2,
      running: true,
    });
  });

  it("says a stopped turn stopped before the history has it, and keeps saying so after", () => {
    const base = view(user("m1", "Go"), tool("t1", "ls", "execute"), toolDone("t1"), delta("Done."));
    // A finished turn gets its row once the history has it.
    expect(types(play(base, done()))).not.toContain("footer");
    const stopped = play(base, done("cancelled"));
    expect(transcriptRows(stopped).at(-1)).toMatchObject({ type: "footer", text: "Stopped", stopped: true });
    // No footer while it runs or waits on the user.
    expect(types(base)).not.toContain("footer");
    const after = play(
      stopped,
      history([
        turn("u1", "user", [{ type: "text", text: "Go" }], "2026-09-26T10:00:00Z"),
        turn("a1", "assistant", [{ type: "text", text: "Done." }], "2026-09-26T10:00:01Z", "2026-09-26T10:00:42Z"),
      ]),
    );
    expect(transcriptRows(after).at(-1)).toMatchObject({ text: `Stopped · 42s · ${clockTime("2026-09-26T10:00:42Z")}`, stopped: true });
  });

  it("ends every answered prompt with its time and the files it changed", () => {
    const edit = (id: string, path: string) => ({
      type: "tool_call" as const,
      tool_call_id: id,
      title: "Edit",
      input_preview: JSON.stringify({ file_path: path, old_string: "a", new_string: "b\nc" }),
      status: null,
    });
    const v = view(
      history([
        turn("u1", "user", [{ type: "text", text: "One" }], "2026-09-26T10:00:00Z"),
        turn("a1", "assistant", [edit("t1", "src/a.ts"), edit("t2", "src/b.ts"), edit("t3", "src/a.ts")], "2026-09-26T10:00:01Z", "2026-09-26T10:09:31Z"),
        turn("u2", "user", [{ type: "text", text: "Two" }], "2026-09-26T10:10:00Z"),
        turn("a2", "assistant", [{ type: "text", text: "Nothing to change." }], "2026-09-26T10:10:01Z", "2026-09-26T10:10:05Z"),
      ]),
    );
    const rows = transcriptRows(v);
    expect(rows.map((r) => r.type)).toEqual(["user", "tools", "footer", "user", "agent", "footer"]);
    expect(rows[2]).toMatchObject({
      text: `9m 31s · ${clockTime("2026-09-26T10:09:31Z")}`,
      files: [
        { path: "src/a.ts", added: 4, removed: 2 },
        { path: "src/b.ts", added: 2, removed: 1 },
      ],
    });
    expect(rows[5]).toMatchObject({ text: `5s · ${clockTime("2026-09-26T10:10:05Z")}`, files: [] });
    // An edit row names its file, to open it.
    const tools = rows[1]!;
    expect(tools.type === "tools" && tools.rows.map((t) => t.type === "tool" && t.tool.path)).toEqual([
      "src/a.ts",
      "src/b.ts",
      "src/a.ts",
    ]);
  });

  it("has no row for the prompt the agent is still answering", () => {
    const answered = history([
      turn("u1", "user", [{ type: "text", text: "Go" }], "2026-09-26T10:00:00Z"),
      turn("a1", "assistant", [{ type: "text", text: "Working on it" }], "2026-09-26T10:00:01Z", "2026-09-26T10:00:05Z"),
    ]);
    expect(types(view(answered))).toContain("footer");
    // Reattached mid-turn: the history holds the running prompt, the live turn has no prompt.
    const running = reduceSession(reduceSession(initialSessionView("s1"), snapshot("prompting")), answered);
    expect(types(running)).not.toContain("footer");
  });

  it("a tool still marked running after the turn ends is not shown as running", () => {
    const v = view(user("m1", "Go"), tool("t1", "sleep 100", "execute"), done("cancelled"));
    const t = transcriptRows(v)[1]!;
    expect(t.type === "tool" && t.tool.state).toBe("idle");
  });

  it("shows a history read mid-turn and the live turn once", () => {
    const v = view(
      history([
        turn("u0", "user", [{ type: "text", text: "Earlier" }]),
        turn("a0", "assistant", [{ type: "text", text: "Earlier answer" }]),
        // The agent's transcript already holds the running turn's start.
        turn("u1", "user", [{ type: "text", text: "Now" }]),
        turn("a1", "assistant", [{ type: "text", text: "Partial" }]),
      ]),
      user("m1", "Now"),
      delta("Partial and more"),
    );
    const rows = transcriptRows(v);
    expect(rows.map((r) => ("text" in r ? r.text : r.type))).toEqual([
      "Earlier",
      "Earlier answer",
      `0s · ${clockTime("2026-09-26T10:00:00Z")}`,
      "Now",
      "Partial and more",
    ]);
    expect(historyBefore(v.history!, v)).toHaveLength(2);
  });

  it("does not repeat anything when the stream replays after a reconnect", () => {
    const frames = [user("m1", "Go"), delta("Hello"), tool("t1", "ls", "execute")];
    const v = view(...frames);
    // The same events again (a replay from an older cursor) change nothing.
    const again = play(v, ...frames);
    expect(transcriptRows(again)).toEqual(transcriptRows(v));
    // A fresh snapshot replaces the live turn rather than adding to it.
    const snap: SessionFrame = {
      type: "snapshot",
      epoch: "e1",
      seq: 50,
      snapshot: {
        status: "prompting",
        live_message: { message_id: null, role: "assistant", content: [{ type: "text", text: "Hello" }, { type: "tool_call", tool_call_id: "t1" }] },
        tool_calls: [{ tool_call_id: "t1", title: "ls", kind: "execute", status: "in_progress", content: null }],
        pending_request: null,
        pending_user_message: { message_id: "m1", blocks: [{ type: "text", text: "Go" }] },
        last_error: null,
        usage: null,
        options: null,
        commands: [],
      },
    };
    expect(transcriptRows(play(v, snap)).map((r) => r.type)).toEqual(["user", "agent", "tool"]);
  });

  it("keeps a finished turn until history has it, then shows history alone", () => {
    const v = view(user("m1", "Go"), delta("Done."), done());
    expect(types(v)).toEqual(["user", "agent"]);
    const after = play(
      v,
      history([
        turn("u1", "user", [{ type: "text", text: "Go" }], "2026-09-26T10:00:00Z"),
        turn("a1", "assistant", [{ type: "text", text: "Done." }], "2026-09-26T10:00:01Z", "2026-09-26T10:00:42Z"),
      ]),
    );
    expect(transcriptRows(after).map((r) => ("text" in r ? r.text : r.type))).toEqual(["Go", "Done.", `42s · ${clockTime("2026-09-26T10:00:42Z")}`]);
  });

  it("shows the agent's last error", () => {
    const v = view(user("m1", "Go"), ev("agent.error", { message: "rate limited", code: 429 }), done("end_turn"));
    expect(types(v)).toContain("error");
  });

  it("shows agent images as rows after their tool call, from history and live", () => {
    const shot = { name: "ab.png", mime_type: "image/png", path: ".zwiim/images/ab.png" };
    const v = view(
      history([
        turn("a1", "assistant", [
          { type: "tool_call", tool_call_id: "r1", title: "Read", input_preview: JSON.stringify({ file_path: "/tmp/s.png" }), status: "completed" },
          { type: "tool_result", tool_call_id: "r1", output_preview: null, is_error: false, images: [shot] },
          { type: "agent_image", name: "cd.png", mime_type: "image/png", path: ".zwiim/images/cd.png" },
          { type: "text", text: "done" },
        ]),
      ]),
    );
    expect(types(v)).toEqual(["tool", "image", "image", "agent"]);
    const rows = transcriptRows(v);
    expect(rows[1]).toMatchObject({ type: "image", image: shot });

    const live = play(v, user("m1", "go"), ev("agent.tool_call", { tool_call_id: "g1", title: "Generate image", kind: "other", status: "completed", content: null, images: [shot] }));
    expect(types(live).slice(-2)).toEqual(["tool", "image"]);
  });

  it("shows a live message image as an image row between text", () => {
    const shot = { name: "ab.png", mime_type: "image/png", path: ".zwiim/images/ab.png" };
    const v = view(user("m1", "go"), delta("look:"), ev("agent.message_image", shot), delta(" done"));
    expect(types(v)).toEqual(["user", "agent", "image", "agent"]);
  });
});

// ─── the Now bar ─────────────────────────────────────────────────────────

describe("redrawing the transcript", () => {
  const past = history([
    turn("u1", "user", [{ type: "text", text: "One" }], "2026-09-26T10:00:00Z"),
    turn(
      "a1",
      "assistant",
      [
        { type: "thought", text: "Look first." },
        { type: "tool_call", tool_call_id: "t1", title: "Read", input_preview: '{"file_path":"src/a.ts"}', status: null },
        { type: "tool_call", tool_call_id: "t2", title: "Read", input_preview: '{"file_path":"src/b.ts"}', status: null },
        { type: "tool_call", tool_call_id: "t3", title: "Read", input_preview: '{"file_path":"src/c.ts"}', status: null },
        { type: "text", text: "Read them." },
      ],
      "2026-09-26T10:00:01Z",
      "2026-09-26T10:00:09Z",
    ),
  ]);

  it("keeps the history's rows the same objects while the live reply grows", () => {
    const build = transcriptBuilder();
    const v1 = view(past, user("u2", "Two"), delta("Work"));
    const v2 = play(v1, delta("ing on it"));
    expect(v2.history).toBe(v1.history);
    const before = build(v1);
    const after = build(v2);
    const history = before.filter((r) => !r.key.startsWith("live:"));
    expect(history.map((r) => r.type)).toEqual(["user", "thought", "tools", "agent", "footer"]);
    history.forEach((row, i) => expect(after[i]).toBe(row));
    expect(after.at(-1)).not.toBe(before.at(-1));
    expect(after).toEqual(transcriptRows(v2));
  });

  it("folds tool calls across the history's end and the live turn as one run", () => {
    const build = transcriptBuilder();
    // A history that starts mid-reply has no prompt, so no footer ends it.
    const midTurn = history([
      turn("a1", "assistant", [
        { type: "tool_call", tool_call_id: "t1", title: "Read", input_preview: '{"file_path":"src/a.ts"}', status: null },
        { type: "tool_call", tool_call_id: "t2", title: "Read", input_preview: '{"file_path":"src/b.ts"}', status: null },
      ]),
    ]);
    const v1 = reduceSession(view(midTurn), snapshot("prompting"));
    const v2 = play(v1, tool("t3", "Read src/c.ts", "read"));
    build(v1);
    expect(build(v2).map((r) => r.type)).toEqual(["tools"]);
    expect(build(v2)).toEqual(transcriptBuilder()(v2));
  });

  it("folds sub-agents into a run across the history's end and the live turn", () => {
    const build = transcriptBuilder();
    const midTurn = history([
      turn("a1", "assistant", [
        {
          type: "tool_call",
          tool_call_id: "t1",
          title: "Read",
          input_preview: '{"file_path":"src/a.ts"}',
          status: null,
        },
        {
          type: "tool_call",
          tool_call_id: "s1",
          title: "Task",
          input_preview: null,
          status: null,
          subagent: { description: "Survey", blocks: [] },
        },
      ]),
    ]);
    const v1 = reduceSession(view(midTurn), snapshot("prompting"));
    const v2 = play(v1, tool("t3", "Read src/c.ts", "read"));
    const v3 = play(
      v2,
      ev("agent.tool_call", {
        tool_call_id: "s2",
        title: "Agent",
        kind: "other",
        status: "in_progress",
        content: null,
        subagent: { description: "Review" },
      } as never),
    );
    build(v1);
    const rows = build(v3);
    expect(rows.map((r) => r.type)).toEqual(["tools"]);
    expect(rows[0]!.type === "tools" && rows[0]!.rows.map((r) => r.type)).toEqual([
      "tool",
      "subagent",
      "tool",
      "subagent",
    ]);
    expect(rows).toEqual(transcriptBuilder()(v3));
  });
});

describe("now bar", () => {
  it("says what the running turn does, from the SDK's events alone", () => {
    let v = view(user("m1", "Go"));
    expect(nowState(v, true)).toEqual({ type: "working", text: "Working" });
    v = play(v, thought("hmm"));
    expect(nowState(v, true)).toEqual({ type: "thinking" });
    v = play(v, tool("t1", "cargo test", "execute"));
    expect(nowState(v, true)).toEqual({ type: "working", text: "Running cargo test" });
    v = play(v, toolDone("t1"), delta("All green"));
    expect(nowState(v, true)).toEqual({ type: "working", text: "Responding" });
    expect(turnRunning(v)).toBe(true);
    expect(turnKey("s1", v)).toBe("s1:m1");
    v = play(v, done());
    expect(nowState(v, true)).toBeNull();
    expect(turnRunning(v)).toBe(false);
  });

  it.each(["inprogress", "in_progress"])("reads a running tool the same whichever way the host spells it (%s)", (wire) => {
    const started = view(user("m1", "Go"));
    const frame = decodeServerFrame(
      JSON.stringify({
        type: "event",
        subscription_id: "sub-1",
        event: {
          session_id: "s1",
          seq: ++seq,
          type: "agent.tool_call",
          payload: { tool_call_id: "w1", title: "cargo test", kind: "execute", status: wire, content: null },
        },
      }),
    );
    if (frame?.type !== "event") throw new Error("expected an event frame");
    const v = play(started, { type: "event", event: frame.event });
    expect(nowState(v, true)).toEqual({ type: "working", text: "Running cargo test" });
    expect(transcriptRows(v).find((r) => r.type === "tool")).toMatchObject({ tool: { state: "running" } });
  });

  it("waits on the user, reconnects, and reports a gone agent", () => {
    const v = view(user("m1", "Go"));
    expect(nowState(v, false)).toEqual({ type: "reconnecting" });
    const asking = play(
      v,
      ev("agent.request", {
        kind: "question",
        request_id: "r1",
        questions: [],
      } as never),
    );
    expect(nowState(asking, true)).toEqual({ type: "needs_you" });
    const gone = reduceSession(initialSessionView("s1"), {
      ...snapshot(),
      snapshot: { ...(snapshot() as Extract<SessionFrame, { type: "snapshot" }>).snapshot, status: "disconnected" },
    } as SessionFrame);
    expect(nowState(gone, true)).toEqual({ type: "disconnected" });
    expect(nowState(reduceSession(gone, { type: "detached", reason: "session_not_live" }), true)).toBeNull();
  });

  it("formats time in mono", () => {
    expect(clock(42_000)).toBe("0:42");
    expect(clock(12 * 60_000 + 5_000)).toBe("12:05");
    expect(clock(3_730_000)).toBe("1:02:10");
    expect(duration(192_000)).toBe("3m 12s");
  });
});

// ─── markdown ────────────────────────────────────────────────────────────

describe("markdown", () => {
  it("reads the blocks agents write", () => {
    expect(parseMarkdown("# Plan\n\nFirst line\nsecond line\n\n- one\n2. two\n> note\n```rust\nfn main() {}\n```\nafter")).toEqual([
      { type: "h", text: "Plan" },
      { type: "p", text: "First line\nsecond line" },
      { type: "li", marker: "•", text: "one" },
      { type: "li", marker: "2.", text: "two" },
      { type: "quote", text: "note" },
      { type: "code", text: "fn main() {}" },
      { type: "p", text: "after" },
    ]);
  });

  it("shows a fence still being written as code", () => {
    expect(parseMarkdown("Run:\n```\ncargo te")).toEqual([
      { type: "p", text: "Run:" },
      { type: "code", text: "cargo te" },
    ]);
  });

  it("reads inline code and bold", () => {
    expect(parseInline("Use `cargo test` **now**, ok")).toEqual([
      { text: "Use " },
      { text: "cargo test", code: true },
      { text: " " },
      { text: "now", bold: true },
      { text: ", ok" },
    ]);
    expect(parseInline("**`resume`** failed")).toEqual([{ text: "resume", code: true, bold: true }, { text: " failed" }]);
  });

  it("reads a line that is only an image as an image block, and links as spans", () => {
    expect(parseMarkdown("Here:\n![screen](.zwiim/images/a.png)\nafter")).toEqual([
      { type: "p", text: "Here:" },
      { type: "image", alt: "screen", src: ".zwiim/images/a.png" },
      { type: "p", text: "after" },
    ]);
    expect(parseInline("see [the report](out/r.pdf) now")).toEqual([
      { text: "see " },
      { text: "the report", href: "out/r.pdf" },
      { text: " now" },
    ]);
  });

  it("keeps the URL of a link that opens nothing in the app", () => {
    const [, link] = parseInline("see [docs](https://example.com/x)");
    expect(spanText(link!)).toBe("[docs](https://example.com/x)");
    expect(spanText({ text: "plain" })).toBe("plain");
  });
});

// ─── showing a session ───────────────────────────────────────────────────

type Sub = { subscription_id: string; session_id: string };
const subscribes = (ch: FakeChannel | undefined): Sub[] =>
  (ch?.sent.filter((m) => m.op === "session.subscribe") ?? []) as unknown as Sub[];
const detaches = (ch: FakeChannel | undefined) => ch?.sent.filter((m) => m.op === "session.detach") ?? [];

function hostWith(sessions: { session_id: string; live: boolean }[]) {
  const w = world({ onLan: false });
  const summaries = sessions.map((s) => ({
    session_id: s.session_id,
    workspace_id: "w1",
    title: s.session_id,
    agent: "claude_code",
    status: s.live ? "in_progress" : "completed",
    live: s.live,
    updated_at: "2026-09-26T10:00:00Z",
    created_at: "2026-09-26T10:00:00Z",
  }));
  w.host.backend.handlers["workspace.list"] = () => [
    { workspace_id: "w1", name: "site", alias: null, kind: "root", parent_workspace_id: null, branch: "main", path: "/site" },
  ];
  w.host.backend.handlers["session.list"] = () => summaries;
  const histories: string[] = [];
  w.host.backend.handlers["session.history"] = (p) => {
    histories.push((p as { session_id: string }).session_id);
    return { turns: [] };
  };
  w.host.backend.handlers["session.options"] = () => ({ options: null });
  const sends: unknown[] = [];
  w.host.backend.handlers["session.send"] = (p) => {
    sends.push(p);
    return {};
  };
  const c = w.connect({ mode: "cloud" });
  return { w, c, histories, sends };
}

describe("showing a session", () => {
  it("streams a finished session and loads its transcript while it is on screen", async () => {
    const { w, c, histories } = hostWith([{ session_id: "old", live: false }]);
    c.showSession("old"); // before the connection is up: kept until it is
    c.connect();
    await waitFor(() => subscribes(w.host.backend.channels.at(-1)).length === 1);
    expect(subscribes(w.host.backend.channel)[0]!.session_id).toBe("old");
    await waitFor(() => histories.includes("old"));
    await waitFor(() => c.store.getState().sessions.old?.history !== null && c.store.getState().sessions.old !== undefined);

    c.hideSession("old");
    await waitFor(() => detaches(w.host.backend.channel).length === 1);
    expect(c.store.getState().sessions.old).toBeUndefined();
    c.close();
  });

  it("keeps following a live session after it is hidden", async () => {
    const { w, c } = hostWith([{ session_id: "live", live: true }]);
    c.connect();
    await waitFor(() => subscribes(w.host.backend.channels.at(-1)).length === 1);
    c.showSession("live");
    c.hideSession("live");
    await new Promise((r) => setTimeout(r, 20));
    expect(detaches(w.host.backend.channel)).toEqual([]);
    // Shown and followed at once: one stream, not two.
    expect(subscribes(w.host.backend.channel)).toHaveLength(1);
    c.close();
  });

  it("sends a message with the phone's id, and refuses while offline", async () => {
    const { w, c, sends } = hostWith([{ session_id: "live", live: true }]);
    await expect(c.send("live", "hi", "id-1")).rejects.toMatchObject({ code: "network" });
    c.connect();
    await waitFor(() => subscribes(w.host.backend.channels.at(-1)).length === 1);
    await c.send("live", "hi", "id-1");
    expect(sends).toEqual([{ session_id: "live", blocks: [{ type: "text", text: "hi" }], client_message_id: "id-1" }]);
    c.close();
  });
});

describe("files sent with a message", () => {
  const pdf = {
    type: "attachment" as const,
    attachment_id: "att-1",
    name: "report.pdf",
    mime_type: "application/pdf",
    size: 2048,
    path: ".zwiim/attachments/att-1/report.pdf",
  };
  const sheet = { ...pdf, attachment_id: "att-2", name: "q3.xlsx", mime_type: "application/vnd.ms-excel", path: "x/q3.xlsx" };

  it("shows a history message's files, and one carrying only files", () => {
    const v = view(
      history([
        turn("u0", "user", [{ type: "text", text: "Summarize this" }, pdf]),
        turn("a0", "assistant", [{ type: "text", text: "Done" }]),
        turn("u1", "user", [{ ...sheet, size: undefined }]),
      ]),
    );
    const users = transcriptRows(v).filter((r) => r.type === "user");
    expect(users).toEqual([
      { type: "user", key: "u0", text: "Summarize this", files: [pdf] },
      { type: "user", key: "u1", text: "", files: [{ ...sheet, size: undefined }] },
    ]);
  });

  it("shows a live message's files", () => {
    const v = view(ev("agent.user_message", { message_id: "m1", blocks: [pdf] }), delta("Reading it"));
    expect(transcriptRows(v)[0]).toMatchObject({ type: "user", text: "", files: [pdf] });
    expect(types(v)).toEqual(["user", "agent"]);
  });

  it("drops the history's copy of a files-only turn shown live, and only that one", () => {
    const live = view(
      history([
        turn("u0", "user", [pdf]),
        turn("a0", "assistant", [{ type: "text", text: "Earlier" }]),
        turn("u1", "user", [sheet]),
      ]),
      ev("agent.user_message", { message_id: "m1", blocks: [sheet] }),
    );
    expect(historyBefore(live.history!, live).map((t) => t.turn_id)).toEqual(["u0", "a0"]);
    // Different files, same (empty) text: a different message.
    const other = view(history([turn("u0", "user", [pdf])]), ev("agent.user_message", { message_id: "m1", blocks: [sheet] }));
    expect(historyBefore(other.history!, other)).toHaveLength(1);
  });
});

describe("forking from a turn", () => {
  it("says what a fork to a worktree made", () => {
    const worktree = { repository: "zwiim", name: "manali", branch: "manali-v2", files: 1234 };
    expect(forkWorktreeLines(worktree)).toEqual({
      title: "You're in a new copy of zwiim called manali",
      detail: "Created manali-v2 and copied 1,234 files",
    });
    expect(forkWorktreeLines({ ...worktree, branch: null, files: 1 }).detail).toBe("Copied 1 file");
  });

  const err = (code: ConstructorParameters<typeof ZwiimClientError>[0]) => new ZwiimClientError(code, "m");
  const said = (id: string, text: string) => turn(id, "user", [{ type: "text", text }]);
  const answered = (id: string, fork_targets?: ForkTarget[]) => ({
    ...turn(id, "assistant", [{ type: "text", text: "Done." }], "2026-09-26T10:00:00Z", "2026-09-26T10:00:01Z"),
    ...(fork_targets ? { fork_targets } : {}),
  });
  const forks = (v: SessionView) => transcriptRows(v).flatMap((r) => (r.type === "footer" ? [r.fork] : []));
  const both: ForkTarget[] = ["current", "new_worktree"];

  it("offers each finished turn as the Host offers it, the latest not older", () => {
    const v = play(view(), history([said("u1", "One"), answered("a1", both), said("u2", "Two"), answered("a2", ["new_worktree"])]));
    expect(forks(v)).toEqual([
      { turn_id: "a1", targets: both, older: true },
      { turn_id: "a2", targets: ["new_worktree"], older: false },
    ]);
  });

  it("offers nothing the Host doesn't", () => {
    expect(forks(play(view(), history([said("u1", "One"), answered("a1")])))).toEqual([null]);
  });

  it("forks from the turn the Host offers when a system turn ends the group", () => {
    const v = play(view(), history([said("u1", "One"), answered("a1", both), turn("s1", "system", [{ type: "text", text: "Note" }])]));
    expect(forks(v)).toEqual([{ turn_id: "a1", targets: both, older: false }]);
  });

  it("marks the turn older once a newer one finishes", () => {
    const build = transcriptBuilder();
    const first = play(view(), history([said("u1", "One"), answered("a1", both)]));
    const footer = (v: SessionView) => build(v).find((r) => r.type === "footer" && r.fork?.turn_id === "a1");
    expect(footer(first)).toMatchObject({ fork: { older: false } });
    const next = play(first, history([said("u1", "One"), answered("a1", both), said("u2", "Two"), answered("a2", both)]));
    expect(footer(next)).toMatchObject({ fork: { older: true } });
  });

  it("warns before forking an older turn in place, and only then", () => {
    const fork = (older: boolean) => ({ turn_id: "a1", targets: both, older });
    expect(forkNote(fork(true), "current")).toBe(FORK_OLDER_TURN_NOTE);
    expect(forkNote(fork(true), "new_worktree")).toBeNull();
    expect(forkNote(fork(false), "current")).toBeNull();
  });

  it("says why forking is off", () => {
    expect(forkDisabledNote(false, false)).toBe(FORK_OFFLINE_NOTE);
    expect(forkDisabledNote(false, true)).toBe(FORK_OFFLINE_NOTE);
    expect(forkDisabledNote(true, true)).toBe(FORK_BUSY_NOTE);
    expect(forkDisabledNote(true, false)).toBeNull();
  });

  it("words a failed fork", () => {
    expect(forkError(err("fork_unavailable"), "Mac")).toBe("This turn can't be forked.");
    expect(forkError(err("checkpoint_unavailable"), "Mac")).toBe("The files at this point are no longer available.");
    expect(forkError(err("unknown_outcome"), "Mac")).toBe("Not sure the fork went through. Check your sessions on Mac before trying again.");
    expect(forkError(err("network"), "Mac")).toBe("Not connected to Mac. Try again when it's back.");
    expect(forkError(err("timeout"), "Mac")).toBe("Not connected to Mac. Try again when it's back.");
    expect(forkError(new Error("x"), "Mac")).toBe("Couldn't fork. Try again.");
  });

  it("says how to go on when a fork's transcript is gone", () => {
    expect(forkedSendError(err("transcript_missing"))).toBe("This session's transcript is gone. Send again to go on without it.");
    expect(forkedSendError(err("network"))).toBeNull();
  });

  it("names the session a fork came from, gone or not", () => {
    expect(forkedFromLabel("Fix login")).toBe("Forked from Fix login");
    expect(forkedFromLabel(null)).toBe("Forked from a session that is gone");
  });
});

describe("resetting to a message", () => {
  const said = (id: string, text: string, revertible?: boolean): HistoryTurn => ({
    ...turn(id, "user", [{ type: "text", text }]),
    ...(revertible === undefined ? {} : { revertible }),
  });
  const answered = (id: string, text: string) => turn(id, "assistant", [{ type: "text", text }]);
  const compat = (capabilities: readonly string[] = CLIENT_CAPABILITIES) =>
    evaluateCompatibility({ host_id: "h", protocol: "zwiim/1", version: "1", owner: null, revision: 3, min_client_revision: 1, capabilities: [...capabilities] });
  const users = (v: SessionView) => transcriptRows(v).filter((r) => r.type === "user");

  it("offers a reset only on the history's messages the Host can put back", () => {
    const v = view(
      history([said("u1", "first", true), answered("a1", "done"), said("u2", "second", false), answered("a2", "ok"), said("u3", "third", true)]),
      user("m1", "live one"),
    );
    expect(users(v).map((r) => [r.key, r.type === "user" ? (r.revert ?? null) : null])).toEqual([
      ["u1", { turn_id: "u1", before: null }],
      ["u2", null],
      ["u3", { turn_id: "u3", before: answered("a2", "ok") }],
      ["live:m1:user", null],
    ]);
  });

  it("takes the chat back too only when a reply comes before the message", () => {
    const v = view(history([said("u1", "first", true), answered("a1", "done"), said("u2", "second", true)]));
    const [first, second] = users(v).map((r) => (r.type === "user" ? r.revert! : null!));
    const options = { model: null, effort: null, mode: null, can_rewind_chat: true };
    // The rest of the rule (the Host, the agent) is the SDK's, tested there.
    expect(offersChatRewind(second!.before, options, compat())).toBe(true);
    expect(offersChatRewind(first!.before, options, compat())).toBe(false);
  });

  it("offers a reset only in a worktree that is still there", () => {
    const ws = (over: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
      workspace_id: "w1",
      name: "a9b70af5",
      alias: null,
      kind: "worktree",
      parent_workspace_id: "root",
      is_git_repo: true,
      branch: "zwiim/a9b70af5",
      base_branch: "main",
      base_commit: null,
      removed: false,
      display_path: null,
      ...over,
    });
    expect(canReset(ws())).toBe(true);
    expect(canReset(ws({ removed: true, branch: null }))).toBe(false);
    expect(canReset(ws({ kind: "root", parent_workspace_id: null, base_branch: null }))).toBe(false);
    expect(canReset(null)).toBe(false);
    expect(canReset(undefined)).toBe(false);
  });

  it("sends one reset while it is pending, and the next once it ends", async () => {
    const run = oneReset<string>();
    let finish!: (v: string) => void;
    let calls = 0;
    const first = run(() => {
      calls++;
      return new Promise((r) => (finish = r));
    });
    // A second tap while the first is in flight is dropped, not sent.
    expect(await run(async () => (calls++, "again"))).toBeUndefined();
    expect(calls).toBe(1);
    finish("done");
    expect(await first).toBe("done");
    expect(await run(async () => (calls++, "next"))).toBe("next");
    expect(calls).toBe(2);
    // A failed one frees it too.
    await expect(run(async () => Promise.reject(new Error("boom")))).rejects.toThrow("boom");
    expect(await run(async () => "after")).toBe("after");
  });

  it("asks in the words agreed, and promises nothing about messages on a files-only reset", () => {
    expect(revertCopy(true)).toEqual({
      title: "Reset chat to here?",
      body: "Later messages will be removed.",
      filesOption: "Also reset files",
      confirm: "Reset",
    });
    const files = revertCopy(false);
    expect(files).toMatchObject({ title: "Reset files to here?", filesOption: null, confirm: "Reset files" });
    expect(files.body).not.toMatch(/messages will be removed/i);
  });

  it("says why a reset did not happen", () => {
    const err = (code: ConstructorParameters<typeof ZwiimClientError>[0], details?: Record<string, unknown>) =>
      new ZwiimClientError(code, "x", 409, details);
    // The Host's refusals read as the SDK words them, on desktop too.
    expect(revertError(err("checkpoint_unavailable"), "Mac")).toBe("This point can't be restored.");
    expect(revertError(err("workspace_busy", { live_sessions: 0, terminals: 0, session_ids: [], operation: "merge" }), "Mac")).toMatch(
      /^The workspace is busy: a merge is in progress/,
    );
    expect(revertError(err("root_workspace"), "Mac")).toMatch(/repository folder/);
    expect(revertError(err("unknown_outcome"), "Mac")).toMatch(/Not sure .* Check .* on Mac/);
    expect(revertError(err("network"), "Mac")).toBe("Not connected to Mac. Try again when it's back.");
    expect(revertError(err("timeout"), "Mac")).toBe("Not connected to Mac. Try again when it's back.");
    expect(revertError(new Error("boom"), "Mac")).toBe("Couldn't reset. Try again.");
  });

  it("asks the Host for the reset once, and refuses while offline", async () => {
    const { w, c } = hostWith([{ session_id: "live", live: true }]);
    const asked: unknown[] = [];
    w.host.backend.handlers["session.revert"] = (p) => {
      asked.push(p);
      return { files_restored: true, chat_rewound: true, head_moved: false };
    };
    await expect(c.revertSession("live", "u1", { files: true, chat: true })).rejects.toMatchObject({ code: "network" });
    c.connect();
    await waitFor(() => subscribes(w.host.backend.channels.at(-1)).length === 1);
    expect(await c.revertSession("live", "u1", { files: false, chat: true })).toEqual({ files_restored: true, chat_rewound: true, head_moved: false });
    expect(asked).toEqual([{ session_id: "live", turn_id: "u1", files: false, chat: true }]);
    c.close();
  });
});

// ─── the can't-reach error ───────────────────────────────────────────────

describe("can't-reach error", () => {
  const snap = (patch: Record<string, unknown>): SessionFrame => {
    const f = snapshot("prompting") as Extract<SessionFrame, { type: "snapshot" }>;
    return { ...f, snapshot: { ...f.snapshot, ...patch } } as SessionFrame;
  };
  const failed = (code: string) => reduceSession(initialSessionView("s1"), snap({ last_error: { message: "provider unreachable", code } }));

  it("gives an unreachable row when the Host can retry", () => {
    const v = failed("agent_unreachable");
    expect(transcriptRows(v, true).at(-1)).toEqual({ type: "unreachable", key: "last-error-1", message: "provider unreachable" });
    expect(transcriptBuilder()(v, true).at(-1)).toMatchObject({ type: "unreachable" });
  });

  it("keys the card on the error, so one replacing another gets a fresh card", () => {
    const err = { message: "provider unreachable", code: "agent_unreachable" };
    const once = failed("agent_unreachable");
    // The same text again, with no clear seen between (user message and failure in one replay).
    const twice = play(once, ev("agent.error", err));
    const keyOf = (v: SessionView) => transcriptRows(v, true).at(-1)!.key;
    expect(keyOf(twice)).not.toBe(keyOf(once));
    // Reading the error already shown (a history read, a re-attach) keeps the card.
    const reread = play(twice, snap({ last_error: { ...err } }));
    expect(keyOf(reread)).toBe(keyOf(twice));
  });

  it("keeps the plain error row without the capability or for another code", () => {
    const plain = { type: "error", key: "last-error", message: "provider unreachable" };
    expect(transcriptRows(failed("agent_unreachable"), false).at(-1)).toEqual(plain);
    expect(transcriptRows(failed("agent_unreachable")).at(-1)).toEqual(plain);
    expect(transcriptRows(failed("internal"), true).at(-1)).toEqual(plain);
  });

  it("drops the card when a new turn starts", () => {
    const v = play(failed("agent_unreachable"), user("u2", "again"));
    expect(transcriptRows(v, true).some((r) => r.type === "unreachable" || r.type === "error")).toBe(false);
  });

  it("says Reconnecting… while the working agent reconnects", () => {
    const v = reduceSession(initialSessionView("s1"), snap({ reconnecting: true }));
    expect(nowState(v, true)).toEqual({ type: "working", text: "Reconnecting…" });
    const back = reduceSession(initialSessionView("s1"), snap({ reconnecting: false }));
    expect(nowState(back, true)).toEqual({ type: "working", text: "Working" });
  });
});
