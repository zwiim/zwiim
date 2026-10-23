import { describe, expect, it } from "vitest";
import { applyEvent, initialSessionView, reduceSession, type SessionFrame } from "../src/state/session";
import { historySubagentCards, liveSubagentCards } from "../src/subagents/cards";
import type { HistoryBlock, SessionEvent, SessionSnapshot, ToolCall } from "../src/types/protocol";

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
});
