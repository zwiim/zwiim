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
  SessionErrorInfo,
  SessionEvent,
  SessionOptions,
  SessionSnapshot,
  SessionStatus,
  ToolCall,
  Usage,
  WireError,
} from "../types/protocol";

export type AttachState = "idle" | "attaching" | "live" | "detached" | "error";

/** The in-flight turn: what the user sent and what the agent has produced. */
export interface LiveTurn {
  userMessage: PendingUserMessage | null;
  message: LiveMessage | null;
  toolCalls: Record<string, ToolCall>;
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
  usage: Usage | null;
  lastStopReason: string | null;
  /**
   * Model / effort / mode, as the host last reported them. Kept while the
   * agent is away: the host re-applies them when it respawns.
   */
  options: SessionOptions | null;
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
    lastStopReason: null,
    options: null,
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
      return { ...view, options: frame.options };
  }
}

function applySnapshot(
  view: SessionView,
  epoch: string,
  seq: number,
  s: SessionSnapshot,
): SessionView {
  const toolCalls: Record<string, ToolCall> = {};
  for (const tc of s.tool_calls) toolCalls[tc.tool_call_id] = tc;
  return {
    ...view,
    attach: "live",
    detachReason: null,
    attachError: null,
    epoch,
    seq,
    agentStatus: s.status,
    live: { userMessage: s.pending_user_message, message: s.live_message, toolCalls },
    // The snapshot is authoritative for the live turn; anything that
    // finished while we were away is in the transcript.
    settled: null,
    pendingRequest: s.pending_request,
    lastError: s.last_error,
    usage: s.usage,
    // A freshly spawned agent may not have advertised its options yet; its
    // `session.options` event follows.
    options: s.options ?? view.options,
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

function appendText(message: LiveMessage | null, type: "text" | "thought", text: string) {
  const base: LiveMessage = message ?? { message_id: null, role: "assistant", content: [] };
  const content = [...base.content];
  const last = content[content.length - 1];
  if (last && last.type === type) {
    content[content.length - 1] = { type, text: last.text + text };
  } else {
    content.push({ type, text });
  }
  return { ...base, content };
}

function withToolRef(message: LiveMessage | null, tool_call_id: string): LiveMessage {
  const base: LiveMessage = message ?? { message_id: null, role: "assistant", content: [] };
  if (base.content.some((b) => b.type === "tool_call" && b.tool_call_id === tool_call_id)) {
    return base;
  }
  const ref: LiveBlock = { type: "tool_call", tool_call_id };
  return { ...base, content: [...base.content, ref] };
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
      return {
        ...next,
        live: {
          ...live,
          message: withToolRef(live.message, ev.payload.tool_call_id),
          toolCalls: { ...live.toolCalls, [ev.payload.tool_call_id]: ev.payload },
        },
      };
    case "agent.tool_call_update": {
      const prev = live.toolCalls[ev.payload.tool_call_id];
      const merged: ToolCall = {
        tool_call_id: ev.payload.tool_call_id,
        kind: prev?.kind ?? null,
        title: ev.payload.title ?? prev?.title ?? null,
        status: ev.payload.status ?? prev?.status ?? null,
        content: ev.payload.content ?? prev?.content ?? null,
      };
      return {
        ...next,
        live: {
          ...live,
          message: withToolRef(live.message, merged.tool_call_id),
          toolCalls: { ...live.toolCalls, [merged.tool_call_id]: merged },
        },
      };
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
      return { ...next, options: ev.payload.options };
    default:
      return next;
  }
}
