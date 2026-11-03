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
  PlanContinues,
  Progress,
  SavedPlan,
  SessionBackground,
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
import { historyLaunches } from "../subagents/cards";

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
  /**
   * Counts the errors `lastError` has held: it moves on for each error event
   * and for any other error that replaces the one shown, and stays put when
   * the same one is seen again. Clients key an error's card on it, so a new
   * error gets a fresh card even if it reads the same as the last.
   */
  errorSerial: number;
  /** The agent is reconnecting after losing its server. */
  reconnecting: boolean;
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
  /**
   * The Host's history generation `history` was read at; null when the Host
   * did not say (older Host, or no agent attached then).
   */
  historyGeneration: number | null;
  /** The transcript changed on the host since `history` was loaded. */
  historyStale: boolean;
  /**
   * Background work the agent launched and is still running, and since when
   * it has only been waiting on it. Null until the Host reports it.
   */
  background: SessionBackground | null;
  /** The running turn's steps; null between turns and on a Host without `agent.progress`. */
  progress: Progress | null;
  /** The newest saved plan, as the last history read reported it. */
  savedPlan: SavedPlan | null;
  /** The plan this session was started to finish. */
  continues: PlanContinues | null;
  /** The new session that took this session's plan over. */
  continuedIn: string | null;
}

export type SessionFrame =
  | { type: "attaching" }
  | { type: "snapshot"; epoch: string; seq: number; snapshot: SessionSnapshot }
  | { type: "replay"; epoch: string; events: SessionEvent[]; high_water_seq: number }
  | { type: "event"; event: SessionEvent }
  | { type: "detached"; reason: DetachReason }
  | { type: "error"; error: WireError }
  | {
      type: "history";
      turns: HistoryTurn[];
      generation?: number | null;
      /** The Host's saved can't-reach error; shown only while no live agent reports its own. */
      lastError?: SessionErrorInfo | null;
      /** The saved plan and its links, as `session.history` sends them. */
      savedPlan?: SavedPlan | null;
      continues?: PlanContinues | null;
      continuedIn?: string | null;
    }
  | { type: "options"; options: SessionOptions };

const emptyTurn = (): LiveTurn => ({ userMessage: null, message: null, toolCalls: {} });

const turnIsEmpty = (t: LiveTurn) =>
  t.userMessage === null &&
  t.message === null &&
  Object.keys(t.toolCalls).length === 0 &&
  Object.keys(t.subagents ?? {}).length === 0;

/**
 * The sub-agents in `t` that the reply does not reach, at any depth: ones
 * launched in an earlier turn, still working in the background.
 */
function launchedEarlier(t: LiveTurn): Set<string> {
  const groups = t.subagents ?? {};
  const reached = new Set<string>();
  const visit = (blocks: readonly LiveBlock[]) => {
    for (const b of blocks) {
      if (b.type !== "tool_call" || reached.has(b.tool_call_id)) continue;
      reached.add(b.tool_call_id);
      visit(groups[b.tool_call_id]?.blocks ?? []);
    }
  };
  visit(t.message?.content ?? []);
  return new Set(Object.keys(groups).filter((id) => !reached.has(id)));
}

/** `t` without sub-agents `ids`, and those sub-agents (with their tool calls) as a turn of their own. */
function takeSubagents(t: LiveTurn, ids: ReadonlySet<string>): [rest: LiveTurn, taken: LiveTurn] {
  const taken = emptyTurn();
  if (ids.size === 0) return [t, taken];
  const kept: Record<string, LiveSubagent> = {};
  const takenGroups: Record<string, LiveSubagent> = {};
  const toolCalls = { ...t.toolCalls };
  for (const [id, group] of Object.entries(t.subagents ?? {})) {
    if (!ids.has(id)) {
      kept[id] = group;
      continue;
    }
    takenGroups[id] = group;
    for (const b of group.blocks) {
      const call = b.type === "tool_call" ? toolCalls[b.tool_call_id] : undefined;
      if (call === undefined) continue;
      taken.toolCalls[call.tool_call_id] = call;
      delete toolCalls[call.tool_call_id];
    }
  }
  const rest: LiveTurn = { ...t, toolCalls };
  if (Object.keys(kept).length > 0) rest.subagents = kept;
  else delete rest.subagents;
  if (Object.keys(takenGroups).length > 0) taken.subagents = takenGroups;
  return [rest, taken];
}

/**
 * A finished turn split in two: its own part, to settle, and what sub-agents
 * launched earlier did in it, which stays live into the next turn.
 */
const settleTurn = (t: LiveTurn) => takeSubagents(t, launchedEarlier(t));

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
  const i = sentAt(turns, settled.userMessage);
  if (i < 0) return false;
  if (settled.message === null) return true;
  return turns.slice(i + 1).some((t) => t.role === "assistant");
}

/** Where in `turns` the message sent live is: the last user turn with its text, or -1. */
function sentAt(turns: readonly HistoryTurn[], message: PendingUserMessage): number {
  const text = blockText(message.blocks);
  let i = turns.length - 1;
  while (i >= 0 && !(turns[i]!.role === "user" && blockText(turns[i]!.blocks) === text)) i--;
  return i;
}

/** The reply in a loaded transcript to a message sent live, as `historyCovers` matches it. */
export function historyReply(turns: readonly HistoryTurn[], message: PendingUserMessage): HistoryTurn | undefined {
  const i = sentAt(turns, message);
  if (i < 0) return undefined;
  for (const t of turns.slice(i + 1)) {
    if (t.role === "user") return undefined;
    if (t.role === "assistant") return t;
  }
  return undefined;
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
    errorSerial: 0,
    reconnecting: false,
    usage: null,
    turnUsage: null,
    lastStopReason: null,
    options: null,
    commands: null,
    history: null,
    historyGeneration: null,
    historyStale: true,
    background: null,
    progress: null,
    savedPlan: null,
    continues: null,
    continuedIn: null,
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
    case "history": {
      // Sub-agents of earlier turns: what they did so far is in the history now.
      const launches = historyLaunches(frame.turns);
      const covered = new Set([...launchedEarlier(view.live)].filter((id) => launches.has(id)));
      return {
        ...view,
        live: takeSubagents(view.live, covered)[0],
        history: frame.turns,
        historyGeneration: frame.generation ?? null,
        historyStale: false,
        settled: null,
        savedPlan: frame.savedPlan ?? null,
        continues: frame.continues ?? null,
        continuedIn: frame.continuedIn ?? null,
        ...withError(view, historyLastError(view, frame.lastError ?? null)),
      };
    }
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
    ...withError(view, s.last_error),
    reconnecting: s.reconnecting ?? false,
    usage: s.usage,
    turnUsage: s.turn_usage ?? null,
    // A freshly spawned agent may not have advertised its options yet; its
    // `session.options` event follows.
    options: s.options ? keepRewindHint(view.options, s.options) : view.options,
    // Likewise its commands, announced shortly after it starts.
    commands: s.commands && s.commands.length > 0 ? s.commands : view.commands,
    // The history on screen is current only if it was read at the Host's
    // present generation; an older Host does not say, so always re-read.
    historyStale:
      view.historyStale ||
      view.history === null ||
      s.history_generation === undefined ||
      s.history_generation !== view.historyGeneration,
    background: s.background ?? null,
    progress: s.progress ?? null,
  };
}

/** `view`'s error fields after `lastError` is read from the Host: a different error moves the serial on. */
function withError(view: SessionView, lastError: SessionErrorInfo | null) {
  const same = view.lastError?.message === lastError?.message && view.lastError?.code === lastError?.code;
  return { lastError, errorSerial: same ? view.errorSerial : view.errorSerial + 1 };
}

/**
 * The error to show after a history read. A live agent's snapshot is the
 * authority, so a live view keeps its own. Otherwise the Host's saved error
 * wins; with none saved, only a stale can't-reach error goes (a retry
 * succeeded), so another error the user was looking at stays.
 *
 * The one exception is the first read after a teardown: the Host writes the
 * saved turn just after the agent goes, so a read that beat it finds nothing,
 * and the card the view had already shown must not drop. `historyStale` is
 * still set from the detach until that first read lands.
 */
function historyLastError(view: SessionView, saved: SessionErrorInfo | null): SessionErrorInfo | null {
  if (view.attach === "live") return view.lastError;
  if (saved !== null) return saved;
  if (view.attach === "detached" && view.historyStale) return view.lastError;
  return view.lastError?.code === "agent_unreachable" ? null : view.lastError;
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
    reconnecting: false,
    historyStale: true,
    // Its background work went with it.
    background: null,
    // Its turn is over; the saved plan comes from the next history read.
    progress: null,
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

/**
 * Whether `ev` says the Host's history moved past the one on screen: a
 * `session.background` carrying a generation other than the history's. Turns
 * that land outside a turn move it with no `agent.turn_complete`.
 */
export function backgroundMovedHistory(ev: SessionEvent, view: SessionView): boolean {
  if (ev.type !== "session.background") return false;
  const generation = ev.payload.history_generation;
  return view.history !== null && generation !== undefined && generation !== view.historyGeneration;
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
      const [own, earlier] = settleTurn(live);
      return {
        ...next,
        settled: turnIsEmpty(own) ? view.settled : own,
        live: { ...earlier, userMessage: ev.payload },
        lastStopReason: null,
        // A new prompt starts a new error scope on the Host too.
        lastError: null,
        // Status travels only in snapshots; turn boundaries are its events.
        agentStatus: "prompting",
        // The last turn's steps do not carry into this one.
        progress: null,
      };
    }
    case "agent.steered": {
      // The agent took it after its turn ended: the transcript has it.
      if (view.agentStatus !== "prompting") return { ...next, historyStale: true };
      // Mid-turn, what the agent says next answers it, so the turn splits
      // here like at a new prompt, still running.
      const [own, earlier] = settleTurn(live);
      return {
        ...next,
        settled: turnIsEmpty(own) ? view.settled : own,
        live: { ...earlier, userMessage: ev.payload },
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
    case "agent.turn_complete": {
      const [own, earlier] = settleTurn(live);
      return {
        ...next,
        live: earlier,
        settled: turnIsEmpty(own) ? view.settled : own,
        pendingRequest: null,
        reconnecting: false,
        agentStatus: view.agentStatus === null ? null : "connected",
        // The turn is over; what it left is the saved plan, read with history.
        progress: null,
        lastStopReason: ev.payload.stop_reason,
        historyStale: true,
      };
    }
    case "agent.error":
      return { ...next, lastError: ev.payload, errorSerial: view.errorSerial + 1 };
    case "agent.progress":
      return { ...next, progress: ev.payload };
    case "session.reconnecting":
      return { ...next, reconnecting: ev.payload.reconnecting };
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
    case "session.background": {
      const { history_generation: _, ...background } = ev.payload;
      return {
        ...next,
        background,
        historyStale: next.historyStale || backgroundMovedHistory(ev, view),
      };
    }
    default:
      return next;
  }
}
