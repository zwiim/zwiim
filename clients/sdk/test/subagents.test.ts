import { describe, expect, it } from "vitest";
import { applyEvent, initialSessionView, reduceSession, type SessionFrame } from "../src/state/session";
import { historyLaunches, historySubagentCards, lateSubagentBlocks, liveSubagentCards } from "../src/subagents/cards";
import { runText, subagentDescription, tokensText, waitingText } from "../src/subagents/labels";
import { modelShortName } from "../src/subagents/model";
import type { HistoryBlock, HistoryTurn, SessionEvent, SessionSnapshot, ToolCall } from "../src/types/protocol";

const ev = (seq: number, type: string, payload: unknown): SessionFrame => ({
  type: "event",
  event: { session_id: "s1", seq, type, payload } as SessionEvent,
});

const call = (tool_call_id: string, title: string, extra: Partial<ToolCall> = {}): ToolCall => ({
  tool_call_id,
  title,
  kind: "other",
  status: "in_progress",
  content: null,
  ...extra,
});

const delegating = [
  ev(1, "agent.message_delta", { text: "Delegating. " }),
  ev(2, "agent.tool_call", call("agent", "Find tests", { subagent: { description: "Find tests", prompt: "Look in src", model: "sonnet" } })),
  ev(3, "agent.subagent_thought_delta", { parent_id: "agent", text: "child " }),
  ev(4, "agent.subagent_thought_delta", { parent_id: "agent", text: "thinks" }),
  ev(5, "agent.subagent_message_delta", { parent_id: "agent", text: "child says" }),
  ev(6, "agent.tool_call", call("read", "Read a.rs", { parent_id: "agent" })),
  ev(7, "agent.tool_call_update", { tool_call_id: "read", title: null, status: "completed", content: null }),
  ev(8, "agent.message_delta", { text: "Done." }),
];

describe("live sub-agent stream", () => {
  it("keeps a sub-agent's text, thinking and tool calls out of the reply", () => {
    const v = delegating.reduce(reduceSession, initialSessionView("s1"));
    expect(v.live.message?.content).toEqual([
      { type: "text", text: "Delegating. " },
      { type: "tool_call", tool_call_id: "agent" },
      { type: "text", text: "Done." },
    ]);
    expect(v.live.subagents).toEqual({
      agent: {
        blocks: [
          { type: "thought", text: "child thinks" },
          { type: "text", text: "child says" },
          { type: "tool_call", tool_call_id: "read" },
        ],
      },
    });
    // The sub-agent's tool calls are still tool calls of the turn; an update
    // that does not repeat the parent keeps it.
    expect(v.live.toolCalls.read?.parent_id).toBe("agent");
    expect(v.live.toolCalls.read?.status).toBe("completed");
  });

  it("joins the main reply's text across a sub-agent's chunks", () => {
    const v = [
      ev(1, "agent.message_delta", { text: "one " }),
      ev(2, "agent.subagent_message_delta", { parent_id: "a", text: "child" }),
      ev(3, "agent.subagent_message_delta", { parent_id: "b", text: "other" }),
      ev(4, "agent.message_delta", { text: "two" }),
    ].reduce(reduceSession, initialSessionView("s1"));
    expect(v.live.message?.content).toEqual([{ type: "text", text: "one two" }]);
    expect(v.live.subagents?.a?.blocks).toEqual([{ type: "text", text: "child" }]);
    expect(v.live.subagents?.b?.blocks).toEqual([{ type: "text", text: "other" }]);
  });

  it("moves a tool call under its sub-agent when an update names the parent", () => {
    const v = [
      ev(1, "agent.tool_call", call("agent", "Task")),
      ev(2, "agent.tool_call", call("read", "Read")),
      ev(3, "agent.tool_call_update", { tool_call_id: "read", title: null, status: "completed", content: null, parent_id: "agent" }),
    ].reduce(reduceSession, initialSessionView("s1"));
    expect(v.live.message?.content).toEqual([{ type: "tool_call", tool_call_id: "agent" }]);
    expect(v.live.subagents?.agent?.blocks).toEqual([{ type: "tool_call", tool_call_id: "read" }]);
  });

  it("fills the launch's sub-agent fields as updates bring them", () => {
    const v = [
      ...delegating,
      ev(9, "agent.tool_call_update", {
        tool_call_id: "agent",
        title: null,
        status: "completed",
        content: null,
        subagent: { model: "claude-opus-5-5", duration_ms: 4200 },
      }),
    ].reduce(reduceSession, initialSessionView("s1"));
    expect(v.live.toolCalls.agent?.subagent).toEqual({
      description: "Find tests",
      prompt: "Look in src",
      model: "claude-opus-5-5",
      duration_ms: 4200,
    });
  });

  it("leaves a turn without sub-agents without the field", () => {
    const v = [ev(1, "agent.message_delta", { text: "hi" })].reduce(reduceSession, initialSessionView("s1"));
    expect(v.live).not.toHaveProperty("subagents");
  });

  it("ignores an event type it does not know", () => {
    const before = reduceSession(initialSessionView("s1"), ev(1, "agent.message_delta", { text: "hi" }));
    const after = applyEvent(before, { session_id: "s1", seq: 2, type: "agent.from_the_future", payload: { text: "x" } } as unknown as SessionEvent);
    expect(after.live).toEqual(before.live);
    expect(after.seq).toBe(2);
  });
});

describe("snapshot with sub-agents", () => {
  const snapshot = {
    status: "prompting",
    pending_user_message: null,
    // The Host keeps a sub-agent's tool calls in `content` for older clients.
    live_message: {
      message_id: null,
      role: "assistant",
      content: [
        { type: "text", text: "Delegating." },
        { type: "tool_call", tool_call_id: "agent" },
        { type: "tool_call", tool_call_id: "read" },
      ],
      subagents: [
        {
          parent_id: "agent",
          blocks: [
            { type: "thought", text: "child thinks" },
            { type: "text", text: "child says" },
            { type: "tool_call", tool_call_id: "read" },
          ],
        },
      ],
    },
    tool_calls: [
      call("agent", "Find tests", { subagent: { description: "Find tests", model: "sonnet" } }),
      call("read", "Read", { parent_id: "agent", status: "completed" }),
    ],
    pending_request: null,
    last_error: null,
    usage: null,
  } as unknown as SessionSnapshot;

  it("rebuilds the sub-agents and keeps them out of the reply", () => {
    const v = reduceSession(initialSessionView("s1"), { type: "snapshot", epoch: "e", seq: 5, snapshot });
    expect(v.live.message?.content).toEqual([
      { type: "text", text: "Delegating." },
      { type: "tool_call", tool_call_id: "agent" },
    ]);
    expect(v.live.message).not.toHaveProperty("subagents");
    expect(v.live.subagents).toEqual({
      agent: {
        blocks: [
          { type: "thought", text: "child thinks" },
          { type: "text", text: "child says" },
          { type: "tool_call", tool_call_id: "read" },
        ],
      },
    });
    // Events after the snapshot keep adding to the same sub-agent.
    const w = reduceSession(v, ev(6, "agent.subagent_message_delta", { parent_id: "agent", text: " more" }));
    expect(w.live.subagents?.agent?.blocks.at(-1)).toEqual({ type: "text", text: " more" });
  });

  it("reads the same cards as the live stream", () => {
    const fromSnapshot = reduceSession(initialSessionView("s1"), { type: "snapshot", epoch: "e", seq: 5, snapshot });
    expect(liveSubagentCards(fromSnapshot.live)).toEqual([
      {
        parent_id: "agent",
        description: "Find tests",
        model: "sonnet",
        prompt: null,
        status: "in_progress",
        duration_ms: null,
        started_at: null,
        tokens: null,
        tool_count: 1,
        blocks: [
          { type: "thought", text: "child thinks" },
          { type: "text", text: "child says" },
          { type: "tool_call", tool_call_id: "read" },
        ],
      },
    ]);
  });
});

describe("liveSubagentCards", () => {
  it("lists one card per launch in launch order, nested ones under theirs", () => {
    const v = [
      ev(1, "agent.tool_call", call("b", "Second", { subagent: { description: "Second" } })),
      ev(2, "agent.tool_call", call("a", "First", { subagent: { description: "First" } })),
      ev(3, "agent.subagent_message_delta", { parent_id: "a", text: "a speaks" }),
      ev(4, "agent.tool_call", call("inner", "Inner", { parent_id: "b", subagent: { description: "Inner" } })),
      ev(5, "agent.tool_call", call("grep", "Grep", { parent_id: "inner" })),
      ev(6, "agent.tool_call_update", { tool_call_id: "b", title: null, status: "completed", content: null, subagent: { duration_ms: 10 } }),
    ].reduce(reduceSession, initialSessionView("s1"));
    const cards = liveSubagentCards(v.live);
    expect(cards.map((c) => [c.parent_id, c.description, c.status, c.tool_count])).toEqual([
      ["b", "Second", "completed", 1],
      ["a", "First", "in_progress", 0],
    ]);
    expect(cards[0]!.duration_ms).toBe(10);
    const nested = liveSubagentCards(v.live, cards[0]!.blocks);
    expect(nested.map((c) => [c.parent_id, c.tool_count])).toEqual([["inner", 1]]);
    expect(nested[0]!.blocks).toEqual([{ type: "tool_call", tool_call_id: "grep" }]);
  });

  it("counts the tools seen so far, then takes the totals its result gives", () => {
    const running = [
      ev(1, "agent.tool_call", call("a", "Agent", { subagent: { description: "Survey" } })),
      ev(2, "agent.tool_call", call("r1", "Read", { parent_id: "a" })),
      ev(3, "agent.tool_call", call("r2", "Read", { parent_id: "a" })),
    ].reduce(reduceSession, initialSessionView("s1"));
    expect(liveSubagentCards(running.live).map((c) => [c.tool_count, c.tokens])).toEqual([[2, null]]);
    const done = reduceSession(
      running,
      ev(4, "agent.tool_call_update", {
        tool_call_id: "a",
        title: null,
        status: "completed",
        content: null,
        subagent: { duration_ms: 9000, tokens: 75223, tool_uses: 42 },
      }),
    );
    expect(liveSubagentCards(done.live).map((c) => [c.tool_count, c.tokens, c.duration_ms])).toEqual([[42, 75223, 9000]]);
  });

  it("reads when it started, in ms, and keeps it through later updates", () => {
    const v = [
      ev(1, "agent.tool_call", call("a", "Agent", { subagent: { started_at: "2026-10-24T09:30:00.000Z" } })),
      ev(2, "agent.tool_call", call("b", "Agent", { subagent: { started_at: "not a time" } })),
      ev(3, "agent.tool_call", call("c", "Agent", { subagent: {} })),
      ev(4, "agent.tool_call_update", {
        tool_call_id: "a",
        title: null,
        status: "completed",
        content: null,
        subagent: { duration_ms: 10 },
      }),
    ].reduce(reduceSession, initialSessionView("s1"));
    expect(liveSubagentCards(v.live).map((c) => [c.parent_id, c.started_at])).toEqual([
      ["a", Date.UTC(2026, 9, 24, 9, 30)],
      ["b", null],
      ["c", null],
    ]);
  });

  it("puts a sub-agent whose launch it never saw last, so nothing is hidden", () => {
    const v = [
      ev(1, "agent.tool_call", call("a", "First", { subagent: {} })),
      ev(2, "agent.subagent_message_delta", { parent_id: "gone", text: "lost" }),
    ].reduce(reduceSession, initialSessionView("s1"));
    expect(liveSubagentCards(v.live).map((c) => [c.parent_id, c.status, c.description])).toEqual([
      ["a", "in_progress", null],
      ["gone", null, null],
    ]);
  });

  it("is empty without sub-agents", () => {
    const v = [ev(1, "agent.tool_call", call("read", "Read"))].reduce(reduceSession, initialSessionView("s1"));
    expect(liveSubagentCards(v.live)).toEqual([]);
  });
});

describe("historySubagentCards", () => {
  const blocks: HistoryBlock[] = [
    { type: "text", text: "Delegating." },
    {
      type: "tool_call",
      tool_call_id: "toolu_01Agent",
      title: "Find tests",
      input_preview: '{"description":"Find tests","prompt":"Look in src"}',
      status: null,
      subagent: {
        description: "Find tests",
        model: "claude-sonnet-5",
        prompt: "Look in src",
        duration_ms: 4200,
        started_at: "2026-10-24T09:30:00.000Z",
        blocks: [
          { type: "thought", text: "child thinks" },
          { type: "tool_call", tool_call_id: "toolu_01Read", title: "Read", input_preview: null, status: null },
          { type: "tool_result", tool_call_id: "toolu_01Read", output_preview: "x", is_error: false },
          { type: "text", text: "child says" },
        ],
      },
    },
    { type: "tool_result", tool_call_id: "toolu_01Agent", output_preview: "found", is_error: false },
    { type: "tool_call", tool_call_id: "toolu_01Bad", title: "Other", input_preview: null, status: null, subagent: { model: "haiku" } },
    { type: "tool_result", tool_call_id: "toolu_01Bad", output_preview: "no", is_error: true },
    { type: "tool_call", tool_call_id: "toolu_01Bash", title: "Bash", input_preview: null, status: null },
  ];

  it("reads one card per launch, status from its result", () => {
    const cards = historySubagentCards(blocks);
    expect(cards).toEqual([
      {
        parent_id: "toolu_01Agent",
        description: "Find tests",
        model: "claude-sonnet-5",
        prompt: "Look in src",
        status: "completed",
        duration_ms: 4200,
        started_at: Date.UTC(2026, 9, 24, 9, 30),
        tokens: null,
        tool_count: 1,
        blocks: (blocks[1] as { subagent: { blocks: HistoryBlock[] } }).subagent.blocks,
      },
      {
        parent_id: "toolu_01Bad",
        description: null,
        model: "haiku",
        prompt: null,
        status: "failed",
        duration_ms: null,
        started_at: null,
        tokens: null,
        tool_count: 0,
        blocks: [],
      },
    ]);
  });
});

describe("background sub-agents", () => {
  const background = (tool_call_id: string, outstanding = 1) => ({
    outstanding,
    tasks: outstanding === 0 ? [] : [{ task_id: "task-00000000000000aa", label: "Review the diff", state: "running", tool_call_id }],
    waiting_since: null,
  });

  it("a launch that returned at once reads running while its task is outstanding", () => {
    const launched = [
      ev(1, "agent.tool_call", call("agent", "Review", { subagent: { description: "Review the diff" } })),
      ev(2, "agent.tool_call_update", { tool_call_id: "agent", title: null, status: "completed", content: null }),
      ev(3, "agent.subagent_message_delta", { parent_id: "agent", text: "looking" }),
    ];
    const v = launched.reduce(reduceSession, initialSessionView("s1"));
    expect(liveSubagentCards(v.live)[0]!.status).toBe("completed");
    expect(liveSubagentCards(v.live, undefined, background("agent"))[0]!.status).toBe("in_progress");
    // Another task's launch, or nothing outstanding: the launch's own status.
    expect(liveSubagentCards(v.live, undefined, background("other"))[0]!.status).toBe("completed");
    expect(liveSubagentCards(v.live, undefined, background("agent", 0))[0]!.status).toBe("completed");
  });

  it("after reload too", () => {
    const blocks: HistoryBlock[] = [
      { type: "tool_call", tool_call_id: "agent", title: "Review", input_preview: null, status: null, subagent: { description: "Review the diff" } },
      { type: "tool_result", tool_call_id: "agent", output_preview: "Async agent launched", is_error: false },
    ];
    expect(historySubagentCards(blocks)[0]!.status).toBe("completed");
    expect(historySubagentCards(blocks, background("agent"))[0]!.status).toBe("in_progress");
    expect(historySubagentCards(blocks, null)[0]!.status).toBe("completed");
  });

  const launch = (id: string, result?: Partial<Extract<HistoryBlock, { type: "tool_result" }>>): HistoryBlock[] => [
    { type: "tool_call", tool_call_id: id, title: "Agent", input_preview: null, status: "completed", subagent: {} },
    ...(result ? [{ type: "tool_result" as const, tool_call_id: id, output_preview: null, is_error: false, ...result }] : []),
  ];

  it("a background launch reads how its task ended, whatever the launch says", () => {
    const status = (background: "completed" | "failed" | "stopped" | null, session = background === null ? background0 : null) =>
      historySubagentCards(launch("agent", { background }), session)[0]!.status;
    const background0 = background("agent", 0);
    expect(status("completed")).toBe("completed");
    expect(status("failed")).toBe("failed");
    expect(status("stopped")).toBe("interrupted");
    // Never said and not running: it died with the session.
    expect(status(null)).toBe("interrupted");
    expect(status(null, background("agent"))).toBe("in_progress");
    // Unknown while the session's work is unknown, or the turn may still be running.
    expect(status(null, null)).toBeNull();
    expect(historySubagentCards(launch("agent", { background: null }), background0, false)[0]!.status).toBeNull();
  });

  it("a launch with no result reads interrupted once its turn is over", () => {
    const blocks: HistoryBlock[] = [
      { type: "tool_call", tool_call_id: "a", title: "Agent", input_preview: null, status: null, subagent: {} },
      { type: "tool_call", tool_call_id: "b", title: "Agent", input_preview: null, status: "in_progress", subagent: {} },
      { type: "tool_call", tool_call_id: "c", title: "Agent", input_preview: null, status: "failed", subagent: {} },
    ];
    expect(historySubagentCards(blocks).map((c) => c.status)).toEqual(["interrupted", "interrupted", "failed"]);
    expect(historySubagentCards(blocks, null, false).map((c) => c.status)).toEqual([null, "in_progress", "failed"]);
    // Still running in the background is not interrupted.
    expect(historySubagentCards(blocks, background("a"))[0]!.status).toBe("in_progress");
  });
});

describe("sub-agents of earlier turns", () => {
  const nested: HistoryBlock = {
    type: "tool_call",
    tool_call_id: "outer",
    title: "Agent",
    input_preview: null,
    status: null,
    subagent: {
      blocks: [
        { type: "tool_call", tool_call_id: "inner", title: "Agent", input_preview: null, status: null, subagent: {} },
        { type: "tool_call", tool_call_id: "read", title: "Read", input_preview: null, status: null },
      ],
    },
  };
  const turn = (blocks: HistoryBlock[]): HistoryTurn => ({
    turn_id: "t",
    role: "assistant",
    blocks,
    timestamp: "2026-10-22T10:00:00Z",
    completed_at: null,
    duration_ms: null,
    model: null,
    usage: null,
  });

  it("finds every launch in history, nested ones too", () => {
    expect([...historyLaunches([turn([{ type: "text", text: "hi" }]), turn([nested])])].sort()).toEqual(["inner", "outer"]);
  });

  it("the reply leaves out sub-agents whose card is in history", () => {
    const v = [
      ev(1, "agent.subagent_message_delta", { parent_id: "outer", text: "late" }),
      ev(2, "agent.subagent_message_delta", { parent_id: "stray", text: "?" }),
    ].reduce(reduceSession, initialSessionView("s1"));
    expect(liveSubagentCards(v.live, undefined, null, new Set(["outer"])).map((c) => c.parent_id)).toEqual(["stray"]);
  });

  it("what a reloaded sub-agent did since arrives as its late blocks, without repeats", () => {
    const v = [
      ev(1, "agent.tool_call", call("read", "Read", { parent_id: "outer", status: "completed" })),
      ev(2, "agent.tool_call", call("grep", "Grep", { parent_id: "outer" })),
      ev(3, "agent.subagent_message_delta", { parent_id: "outer", text: "found it" }),
    ].reduce(reduceSession, initialSessionView("s1"));
    const [card] = historySubagentCards([nested]);
    expect(lateSubagentBlocks(card!, v.live)).toEqual([
      { type: "tool_call", tool_call_id: "grep" },
      { type: "text", text: "found it" },
    ]);
    const [inner] = historySubagentCards(card!.blocks);
    expect(lateSubagentBlocks(inner!, v.live)).toEqual([]);
  });
});

describe("model short name", () => {
  it("names current Claude ids by family and version", () => {
    expect(modelShortName("claude-sonnet-5-5")).toBe("Sonnet 5.5");
    expect(modelShortName("claude-opus-4-1-20250805")).toBe("Opus 4.1");
    expect(modelShortName("claude-sonnet-4-20250514")).toBe("Sonnet 4");
    expect(modelShortName("claude-haiku-4-5")).toBe("Haiku 4.5");
    expect(modelShortName("claude-opus-5-5[1m]")).toBe("Opus 5.5");
  });

  it("names older Claude ids and bare family aliases", () => {
    expect(modelShortName("claude-3-5-sonnet-20241022")).toBe("Sonnet 3.5");
    expect(modelShortName("claude-3-opus-20240229")).toBe("Opus 3");
    expect(modelShortName("sonnet")).toBe("Sonnet");
    expect(modelShortName("Opus")).toBe("Opus");
  });

  it("passes unknown ids through and omits a missing model", () => {
    expect(modelShortName("gpt-5.2-codex")).toBe("gpt-5.2-codex");
    expect(modelShortName("claude-sonnet-latest")).toBe("claude-sonnet-latest");
    expect(modelShortName("inherit")).toBe("inherit");
    expect(modelShortName("")).toBeNull();
    expect(modelShortName("   ")).toBeNull();
    expect(modelShortName(null)).toBeNull();
    expect(modelShortName(undefined)).toBeNull();
  });
});

describe("sub-agent labels", () => {
  it("describes a sub-agent by what it was asked, else a launch title that says more than Agent", () => {
    expect(subagentDescription("  Review the diff ", "Agent")).toBe("Review the diff");
    expect(subagentDescription(null, "`Survey the repo`")).toBe("Survey the repo");
    expect(subagentDescription("", "Survey the repo")).toBe("Survey the repo");
    // The generic launch names say nothing, so nothing is shown.
    for (const title of ["Agent", "Task", "`Agent`", "agent", "spawn_agent", "Spawn subagent", "", "  ", null, undefined]) {
      expect(subagentDescription(null, title)).toBeNull();
    }
  });

  it("shortens token counts", () => {
    expect([0, 840, 999, 1000, 9949, 12_400, 75_223, 999_499, 999_500, 1_234_567].map(tokensText)).toEqual([
      "0 tokens",
      "840 tokens",
      "999 tokens",
      "1k tokens",
      "9.9k tokens",
      "12k tokens",
      "75k tokens",
      "999k tokens",
      "1M tokens",
      "1.2M tokens",
    ]);
  });

  it("counts background tasks in the singular and plural", () => {
    expect(waitingText(1)).toBe("Waiting on 1 background task");
    expect(waitingText(3)).toBe("Waiting on 3 background tasks");
  });

  it("names a folded run by its tool calls and sub-agents, in the singular and plural", () => {
    expect(runText(3, 0)).toBe("3 tool calls");
    expect(runText(1, 0)).toBe("1 tool call");
    expect(runText(0, 3)).toBe("3 sub-agents");
    expect(runText(2, 1)).toBe("2 tool calls, 1 sub-agent");
    expect(runText(1, 2)).toBe("1 tool call, 2 sub-agents");
    expect(runText(1, 1)).toBe("1 tool call, 1 sub-agent");
  });
});
