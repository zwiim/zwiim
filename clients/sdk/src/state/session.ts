// Session view reconstruction. A view is the persisted transcript
// (`session.history`) plus the live turn rebuilt from a snapshot and the
// events after it. Everything here is derived from host frames; nothing is
// authoritative on the client.

import type {
  AgentRequest,
  AgentStatus,
  DetachReason,
  HistoryTurn,
  LiveBlock,
  LiveMessage,
  PendingUserMessage,
  SessionCommand,
  SessionErrorInfo,
  SessionEvent,
  SessionOptions,
  SessionSnapshot,
  SessionStatus,
  SubagentInfo,
  ToolCall,
  TurnUsage,
  Usage,
  WireError,
} from "../types/protocol";

export type AttachState = "idle" | "attaching" | "live" | "detached" | "error";

/** The in-flight turn: what the user sent and what the agent has produced. */
export interface LiveTurn {
  userMessage: PendingUserMessage | null;
  /** The reply itself; a sub-agent's blocks are never in its `content`. */
  message: LiveMessage | null;
  toolCalls: Record<string, ToolCall>;
  /**
   * What each sub-agent has produced so far (its thinking, text and tool
   * calls, in order), by the tool call that launched it. Absent until a
   * sub-agent speaks. Read it through `liveSubagentCards`.
   */
  subagents?: Record<string, LiveSubagent>;
}

export interface LiveSubagent {
  blocks: LiveBlock[];
}

export interface SessionView {
  session_id: string;
  attach: AttachState;
  detachReason: DetachReason | null;
  attachError: WireError | null;
  /** Replay cursor; only meaningful with `epoch`. */
  epoch: string | null;
  seq: number | null;
  agentStatus: AgentStatus | null;
  sessionStatus: SessionStatus | null;
  live: LiveTurn;
  /** A completed live turn kept on screen until history includes it. */
  settled: LiveTurn | null;
  pendingRequest: AgentRequest | null;
  lastError: SessionErrorInfo | null;
  /** The context window as the agent last reported it; null until it does. */
  usage: Usage | null;
  /** The last finished turn's tokens as the agent reported them. */
  turnUsage: TurnUsage | null;
  lastStopReason: string | null;
  /**
   * Model / effort / mode, as the host last reported them. Kept while the
   * agent is away: the host re-applies them when it respawns.
   */
  options: SessionOptions | null;
  /** The agent's slash commands; null until a live agent announces them. */
  commands: SessionCommand[] | null;
  history: HistoryTurn[] | null;
  /** The transcript changed on the host since `history` was loaded. */
  historyStale: boolean;
}

export type SessionFrame =
  | { type: "attaching" }
  | { type: "snapshot"; epoch: string; seq: number; snapshot: SessionSnapshot }
  | { type: "replay"; epoch: string; events: SessionEvent[]; high_water_seq: number }
  | { type: "event"; event: SessionEvent }
  | { type: "detached"; reason: DetachReason }
  | { type: "error"; error: WireError }
  | { type: "history"; turns: HistoryTurn[] }
  | { type: "options"; options: SessionOptions };

const emptyTurn = (): LiveTurn => ({ userMessage: null, message: null, toolCalls: {} });

const turnIsEmpty = (t: LiveTurn) =>
  t.userMessage === null && t.message === null && Object.keys(t.toolCalls).length === 0;

const blockText = (blocks: readonly { type: string; text?: string }[]) =>
  blocks
    .map((b) => (b.type === "text" ? (b.text ?? "") : ""))
    .join("")
    .trim();

/**
 * Whether a loaded transcript already contains a turn that finished live.
 * Agents persist their transcript asynchronously, so a history read right
 * after `agent.turn_complete` can still be missing the reply.
 */
export function historyCovers(turns: readonly HistoryTurn[], settled: LiveTurn): boolean {
  if (settled.userMessage === null) return true; // nothing to match against
  const text = blockText(settled.userMessage.blocks);
  let i = turns.length - 1;
  while (i >= 0 && !(turns[i]!.role === "user" && blockText(turns[i]!.blocks) === text)) i--;
  if (i < 0) return false;
  if (settled.message === null) return true;
  return turns.slice(i + 1).some((t) => t.role === "assistant");
}

export function initialSessionView(session_id: string): SessionView {
  return {
    session_id,
    attach: "idle",
    detachReason: null,
    attachError: null,
    epoch: null,
    seq: null,
    agentStatus: null,
    sessionStatus: null,
    live: emptyTurn(),
    settled: null,
    pendingRequest: null,
    lastError: null,
    usage: null,
    turnUsage: null,
    lastStopReason: null,
    options: null,
    commands: null,
    history: null,
    historyStale: true,
  };
}

export function reduceSession(view: SessionView, frame: SessionFrame): SessionView {
  switch (frame.type) {
    case "attaching":
      return { ...view, attach: "attaching", attachError: null };
    case "snapshot":
      return applySnapshot(view, frame.epoch, frame.seq, frame.snapshot);
    case "replay": {
      let next: SessionView = { ...view, attach: "live", detachReason: null, epoch: frame.epoch };
      for (const ev of frame.events) next = applyEvent(next, ev);
      return { ...next, seq: Math.max(next.seq ?? 0, frame.high_water_seq) };
    }
    case "event":
      return applyEvent(view, frame.event);
    case "detached":
      return applyDetached(view, frame.reason);
    case "error":
      return { ...view, attach: "error", attachError: frame.error };
    case "history":
      return { ...view, history: frame.turns, historyStale: false, settled: null };
    case "options":
      return { ...view, options: keepRewindHint(view.options, frame.options) };
  }
}

/**
 * `next`, keeping `can_rewind_chat` from `prev` when `next` does not say:
 * only `session.options` replies carry it, not live option changes.
 */
function keepRewindHint(prev: SessionOptions | null, next: SessionOptions | null): SessionOptions | null {
  if (!next || next.can_rewind_chat !== undefined || prev?.can_rewind_chat === undefined) return next;
  return { ...next, can_rewind_chat: prev.can_rewind_chat };
}

function applySnapshot(
  view: SessionView,
  epoch: string,
  seq: number,
  s: SessionSnapshot,
): SessionView {
  const toolCalls: Record<string, ToolCall> = {};
  for (const tc of s.tool_calls) toolCalls[tc.tool_call_id] = tc;
  let message: LiveMessage | null = null;
  const subagents: Record<string, LiveSubagent> = {};
  if (s.live_message) {
    // The Host leaves a sub-agent's tool calls in `content` too, for clients
    // that predate sub-agents; here they belong to their sub-agent only.
    const { subagents: groups, ...rest } = s.live_message;
    const isChild = (b: LiveBlock) => b.type === "tool_call" && toolCalls[b.tool_call_id]?.parent_id !== undefined;
    message = { ...rest, content: rest.content.filter((b) => !isChild(b)) };
    for (const g of groups ?? []) subagents[g.parent_id] = { blocks: [...(subagents[g.parent_id]?.blocks ?? []), ...g.blocks] };
  }
  const live: LiveTurn = { userMessage: s.pending_user_message, message, toolCalls };
  if (Object.keys(subagents).length > 0) live.subagents = subagents;
  return {
    ...view,
    attach: "live",
    detachReason: null,
    attachError: null,
    epoch,
    seq,
    agentStatus: s.status,
    live,
    // The snapshot is authoritative for the live turn; anything that
    // finished while we were away is in the transcript.
    settled: null,
    pendingRequest: s.pending_request,
    lastError: s.last_error,
    usage: s.usage,
    turnUsage: s.turn_usage ?? null,
    // A freshly spawned agent may not have advertised its options yet; its
    // `session.options` event follows.
    options: s.options ? keepRewindHint(view.options, s.options) : view.options,
    // Likewise its commands, announced shortly after it starts.
    commands: s.commands && s.commands.length > 0 ? s.commands : view.commands,
    historyStale: true,
  };
}

function applyDetached(view: SessionView, reason: DetachReason): SessionView {
  if (reason !== "session_not_live") {
    // Transient: the client re-attaches with its cursor.
    return { ...view, attach: "attaching", detachReason: reason };
  }
  // The agent is gone: its turn is over and in the transcript.
  return {
    ...view,
    attach: "detached",
    detachReason: reason,
    epoch: null,
    seq: null,
    agentStatus: null,
    live: emptyTurn(),
    settled: turnIsEmpty(view.live) ? view.settled : view.live,
    pendingRequest: null,
    historyStale: true,
  };
}

/** `blocks` with a chunk appended, joined onto a trailing block of its type. */
function appendTo(blocks: readonly LiveBlock[], type: "text" | "thought", text: string): LiveBlock[] {
  const out = [...blocks];
  const last = out[out.length - 1];
  if (last && last.type === type) {
    out[out.length - 1] = { type, text: last.text + text };
  } else {
    out.push({ type, text });
  }
  return out;
}

function appendText(message: LiveMessage | null, type: "text" | "thought", text: string): LiveMessage {
  const base: LiveMessage = message ?? { message_id: null, role: "assistant", content: [] };
  return { ...base, content: appendTo(base.content, type, text) };
}

const isRef = (b: LiveBlock, tool_call_id: string) => b.type === "tool_call" && b.tool_call_id === tool_call_id;

function withRef(blocks: readonly LiveBlock[], tool_call_id: string): LiveBlock[] {
  return blocks.some((b) => isRef(b, tool_call_id)) ? [...blocks] : [...blocks, { type: "tool_call", tool_call_id }];
}

function withToolRef(message: LiveMessage | null, tool_call_id: string): LiveMessage {
  const base: LiveMessage = message ?? { message_id: null, role: "assistant", content: [] };
  if (base.content.some((b) => isRef(b, tool_call_id))) return base;
  return { ...base, content: withRef(base.content, tool_call_id) };
}

/** `live` with `update` applied to sub-agent `parent_id`'s blocks. */
function inSubagent(live: LiveTurn, parent_id: string, update: (blocks: readonly LiveBlock[]) => LiveBlock[]): LiveTurn {
  const subagents = live.subagents ?? {};
  return {
    ...live,
    subagents: { ...subagents, [parent_id]: { blocks: update(subagents[parent_id]?.blocks ?? []) } },
  };
}

/**
 * `live` with tool call `call` placed: under its sub-agent when it has a
 * parent (moved out of the reply if an earlier event put it there), in the
 * reply otherwise.
 */
function placeToolCall(live: LiveTurn, call: ToolCall): LiveTurn {
  const toolCalls = { ...live.toolCalls, [call.tool_call_id]: call };
  if (call.parent_id === undefined) {
    return { ...live, message: withToolRef(live.message, call.tool_call_id), toolCalls };
  }
  const message = live.message?.content.some((b) => isRef(b, call.tool_call_id))
    ? { ...live.message, content: live.message.content.filter((b) => !isRef(b, call.tool_call_id)) }
    : live.message;
  return inSubagent({ ...live, message, toolCalls }, call.parent_id, (blocks) => withRef(blocks, call.tool_call_id));
}

/** What `newer` says about a sub-agent, over what `older` said. */
function mergeSubagent(older: SubagentInfo | undefined, newer: SubagentInfo | undefined): SubagentInfo | undefined {
  if (older === undefined || newer === undefined) return newer ?? older;
  return { ...older, ...newer };
}

export function applyEvent(view: SessionView, ev: SessionEvent): SessionView {
  if (view.seq !== null && ev.seq <= view.seq) return view;
  const next: SessionView = { ...view, seq: ev.seq };
  const live = view.live;
  switch (ev.type) {
    case "agent.message_delta":
      return { ...next, live: { ...live, message: appendText(live.message, "text", ev.payload.text) } };
    case "agent.thought_delta":
      return { ...next, live: { ...live, message: appendText(live.message, "thought", ev.payload.text) } };
    case "agent.subagent_message_delta":
      return { ...next, live: inSubagent(live, ev.payload.parent_id, (b) => appendTo(b, "text", ev.payload.text)) };
    case "agent.subagent_thought_delta":
      return { ...next, live: inSubagent(live, ev.payload.parent_id, (b) => appendTo(b, "thought", ev.payload.text)) };
    case "agent.message_image": {
      const base: LiveMessage = live.message ?? { message_id: null, role: "assistant", content: [] };
      return {
        ...next,
        live: { ...live, message: { ...base, content: [...base.content, { type: "agent_image", ...ev.payload }] } },
      };
    }
    case "agent.user_message": {
      // A new turn starts; an unsettled previous turn stays until history catches up.
      const settled = turnIsEmpty(live) ? view.settled : live;
      return {
        ...next,
        settled,
        live: { userMessage: ev.payload, message: null, toolCalls: {} },
        lastStopReason: null,
        // Status travels only in snapshots; turn boundaries are its events.
        agentStatus: "prompting",
      };
    }
    case "agent.tool_call":
      return { ...next, live: placeToolCall(live, ev.payload) };
    case "agent.tool_call_update": {
      const prev = live.toolCalls[ev.payload.tool_call_id];
      const merged: ToolCall = {
        tool_call_id: ev.payload.tool_call_id,
        kind: prev?.kind ?? null,
        title: ev.payload.title ?? prev?.title ?? null,
        status: ev.payload.status ?? prev?.status ?? null,
        content: ev.payload.content ?? prev?.content ?? null,
        images: ev.payload.images ?? prev?.images,
      };
      const parent_id = ev.payload.parent_id ?? prev?.parent_id;
      if (parent_id !== undefined) merged.parent_id = parent_id;
      const subagent = mergeSubagent(prev?.subagent, ev.payload.subagent);
      if (subagent !== undefined) merged.subagent = subagent;
      return { ...next, live: placeToolCall(live, merged) };
    }
    case "agent.request":
      return { ...next, pendingRequest: ev.payload, agentStatus: "prompting" };
    case "agent.request_resolved":
      return view.pendingRequest?.request_id === ev.payload.request_id
        ? { ...next, pendingRequest: null }
        : next;
    case "agent.turn_complete":
      return {
        ...next,
        live: emptyTurn(),
        settled: turnIsEmpty(live) ? view.settled : live,
        pendingRequest: null,
        agentStatus: view.agentStatus === null ? null : "connected",
        lastStopReason: ev.payload.stop_reason,
        historyStale: true,
      };
    case "agent.error":
      return { ...next, lastError: ev.payload };
    case "session.status":
      return { ...next, sessionStatus: ev.payload.status };
    case "session.options":
      return { ...next, options: keepRewindHint(next.options, ev.payload.options) };
    case "session.commands":
      return { ...next, commands: ev.payload.commands };
    case "session.usage":
      return { ...next, usage: ev.payload };
    case "session.turn_usage":
      return { ...next, turnUsage: ev.payload };
    default:
      return next;
  }
}
