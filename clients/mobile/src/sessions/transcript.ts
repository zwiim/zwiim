// A session as the Session screen reads it (DESIGN.md, Screen 5): the
// transcript as rows, and the Now bar's one line on what the agent is doing.
// Derived from the SDK's `SessionView` alone: the persisted history, a turn
// that finished but isn't in it yet, and the live turn. The SDK drops
// replayed events by sequence, so rebuilding these after a reconnect never
// repeats a message or a tool call.

import {
  clockTime,
  historyLaunches,
  historySubagentCards,
  lateSubagentBlocks,
  liveSubagentCards,
  modelShortName,
  parseUnifiedDiff,
  subagentDescription,
  toolChanges,
  turnDuration,
  turnGroups,
  type AgentImage,
  type AttachmentBlock,
  type ChangedFile,
  type FileDiff,
  type HistoryBlock,
  type HistoryTurn,
  type LiveBlock,
  type LiveTurn,
  type SessionBackground,
  type SessionView,
  type SubagentCard,
  type ToolCall,
  type ToolCallStatus,
} from "@zwiim/sdk";

export type ToolState = "running" | "done" | "failed" | "idle";

export interface ToolItem {
  /** What it is, when the agent says: Run, Read, Edit… */
  verb: string | null;
  /** What it works on: a command, a path, a query. */
  target: string;
  state: ToolState;
  /** What went in and came out, as the agent reports them. */
  input: string | null;
  output: string | null;
  /** Lines added and removed, when the output is a diff. */
  added: number;
  removed: number;
  /** The one file it changed, as the agent names it; opens in the viewer. */
  path: string | null;
}

/** A history message the Host can put the session back to (it is `revertible`). */
export interface RevertPoint {
  turn_id: string;
  /** The history turn before it, which a chat rewind goes back to; null on the first message. */
  before: HistoryTurn | null;
}

/** One sub-agent of a turn, as its card shows it (collapsed by default). */
export interface SubagentItem {
  /** The model's short name ("Sonnet 5.5"); null when the agent names none. */
  model: string | null;
  /** What it was asked to do, else its launch's title when that says more than "Agent". */
  description: string | null;
  /** Interrupted: its turn ended before it did, and nothing says it finished. */
  state: ToolState | "interrupted";
  /** How long it ran ("1m 46s"), once known. */
  duration: string | null;
  /** Tool calls it made itself. */
  tools: number;
  prompt: string | null;
  /** What it did, in order, as rows of its own (never top-level rows). */
  rows: TranscriptRow[];
}

export type TranscriptRow =
  | { type: "user"; key: string; text: string; files: AttachmentBlock[]; revert?: RevertPoint }
  | { type: "agent"; key: string; text: string; streaming: boolean }
  | { type: "thought"; key: string; text: string; streaming: boolean }
  | { type: "tool"; key: string; tool: ToolItem }
  | { type: "tools"; key: string; tools: ToolItem[] }
  | { type: "subagent"; key: string; agent: SubagentItem }
  | { type: "error"; key: string; message: string }
  | { type: "image"; key: string; image: AgentImage }
  | { type: "footer"; key: string; text: string; stopped: boolean; files: ChangedFile[] };

/** Consecutive tool calls fold into one row from this many. */
export const FOLD_AT = 3;

const VERBS: Record<string, string> = {
  read: "Read",
  edit: "Edit",
  delete: "Delete",
  move: "Move",
  search: "Search",
  execute: "Run",
  fetch: "Fetch",
  think: "Think",
};

/**
 * A transcript names the agent's tool and passes its input as JSON; read
 * both as a live call reads: a verb and what it works on.
 */
const TOOL_VERBS: Record<string, string> = {
  bash: "Run",
  shell: "Run",
  exec_command: "Run",
  read: "Read",
  edit: "Edit",
  multiedit: "Edit",
  write: "Write",
  grep: "Search",
  glob: "Search",
  websearch: "Search",
  webfetch: "Fetch",
  // Claude titles it so live.
  exitplanmode: "Exited Plan Mode",
};
const TARGET_KEYS = ["command", "cmd", "file_path", "path", "notebook_path", "pattern", "query", "url", "description"];

function historyTool(name: string, input: string | null): { verb: string | null; target: string; input: string | null } {
  let args: Record<string, unknown> | null = null;
  try {
    const parsed: unknown = input ? JSON.parse(input) : null;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) args = parsed as Record<string, unknown>;
  } catch {
    // Not JSON (or cut short): shown as it is.
  }
  const value = (key: string) => {
    const v = args?.[key];
    return Array.isArray(v) ? v.join(" ") : typeof v === "string" ? v : null;
  };
  const key = TARGET_KEYS.find((k) => value(k)?.trim());
  const target = key ? value(key)!.trim().split("\n")[0]! : null;
  const verb = TOOL_VERBS[name.toLowerCase()] ?? (target ? name : null);
  const shown = value("command") ?? value("cmd") ?? (args ? JSON.stringify(args, null, 2) : input);
  // A verb that names the whole call (Exited Plan Mode) reads as live: alone.
  if (!target && verb) return { verb: null, target: verb, input: shown };
  return { verb, target: target ?? name, input: shown };
}

/** A title without the backticks agents wrap commands in. */
const unquote = (title: string) => {
  const t = title.trim();
  return t.length > 1 && t.startsWith("`") && t.endsWith("`") ? t.slice(1, -1) : t;
};

const looksLikeDiff = (text: string) => text.startsWith("--- ") && text.includes("\n+++ ");

function diffStats(output: string | null): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  if (output && looksLikeDiff(output)) {
    for (const line of output.split("\n")) {
      if (line.startsWith("+") && !line.startsWith("+++")) added++;
      else if (line.startsWith("-") && !line.startsWith("---")) removed++;
    }
  }
  return { added, removed };
}

/** Still going (the SDK decodes every spelling to `in_progress`). */
const active = (status: ToolCallStatus | null | undefined) => status === "in_progress" || status === "pending";

function toolState(status: ToolCallStatus | null, running: boolean): ToolState {
  if (status === "completed") return "done";
  if (status === "failed") return "failed";
  if (active(status) && running) return "running";
  return "idle";
}

/** The most of a tool's input or output the transcript shows. More (an
 * image's base64, a huge log) takes seconds to lay out and is not read. */
const SHOWN_CHARS = 20_000;

/** A tool's text cut to what the transcript shows, saying how much is left out. */
export function clipToolText(text: string): string {
  if (text.length <= SHOWN_CHARS) return text;
  return `${text.slice(0, SHOWN_CHARS)}\n… ${(text.length - SHOWN_CHARS).toLocaleString("en-US")} more characters`;
}

function toolItem(p: {
  kind: string | null | undefined;
  verb?: string | null;
  title: string | null;
  state: ToolState;
  input: string | null;
  output: string | null;
  changes?: FileDiff[] | null;
}): ToolItem {
  const title = unquote(p.title ?? "") || "Tool call";
  let verb = p.verb ?? (p.kind ? (VERBS[p.kind] ?? null) : null);
  // The agent's title often starts with the verb already ("Read src/a.ts").
  if (verb && title.toLowerCase().startsWith(verb.toLowerCase() + " ")) verb = null;
  const changes = p.changes ?? (p.output ? parseUnifiedDiff(p.output) : null);
  const path = changes?.length === 1 ? changes[0]!.path : null;
  const input = p.input && clipToolText(p.input);
  const output = p.output && clipToolText(p.output);
  return { verb, target: title, state: p.state, input, output, ...diffStats(p.output), path };
}

/** A live tool call as a row. */
export function liveTool(tc: ToolCall | undefined, running: boolean): ToolItem {
  return toolItem({
    kind: tc?.kind,
    title: tc?.title ?? null,
    state: toolState(tc?.status ?? null, running),
    input: null,
    output: typeof tc?.content === "string" && tc.content ? tc.content : null,
  });
}

const userText = (blocks: readonly { type: string; text?: string }[]) =>
  blocks
    .map((b) => (b.type === "text" ? (b.text ?? "") : ""))
    .join("")
    .trim();

const userFiles = (blocks: readonly { type: string }[]) =>
  blocks.filter((b): b is AttachmentBlock => b.type === "attachment");

/** What identifies a user message across history and live: its text and the files it carried. */
const userKey = (blocks: readonly { type: string; text?: string }[]) =>
  [userText(blocks), ...userFiles(blocks).map((f) => f.attachment_id)].join("\u0000");

type Piece = Exclude<TranscriptRow, { type: "tools" }>;

const imageRows = (key: string, images: AgentImage[] | undefined): Piece[] =>
  (images ?? []).map((image, i) => ({ type: "image", key: `${key}:img${i}`, image }));

/** A sub-agent's card: still going reads running, whether or not the turn is. */
function subagentItem<B>(card: SubagentCard<B>, title: string | null, rows: TranscriptRow[]): SubagentItem {
  return {
    model: modelShortName(card.model),
    description: subagentDescription(card.description, title),
    state:
      card.status === "completed"
        ? "done"
        : card.status === "failed"
          ? "failed"
          : card.status === "interrupted"
            ? "interrupted"
            : active(card.status)
              ? "running"
              : "idle",
    duration: card.duration_ms !== null ? turnDuration(card.duration_ms) : null,
    tools: card.tool_count,
    prompt: card.prompt?.trim() || null,
    rows,
  };
}

/** What a screen reader says of a card: never a status it does not know. */
export function subagentLabel(agent: SubagentItem): string {
  const tools = agent.tools > 0 ? `${agent.tools} ${agent.tools === 1 ? "tool" : "tools"}` : null;
  const state = agent.state === "idle" ? null : agent.state;
  return ["Agent", agent.model, agent.description, tools, agent.duration, state].filter(Boolean).join(", ");
}

/** What an open card shows; a collapsed one shows nothing, so nothing in it is drawn. */
export function subagentShown(
  agent: SubagentItem,
  open: boolean,
  promptOpen: boolean,
): { promptToggle: boolean; prompt: string | null; rows: TranscriptRow[] } {
  if (!open) return { promptToggle: false, prompt: null, rows: [] };
  return { promptToggle: agent.prompt !== null, prompt: promptOpen ? agent.prompt : null, rows: agent.rows };
}

/**
 * A turn of the history as rows. `ended` is false while it may still be
 * running; `live` is given when a sub-agent it launched still adds to the
 * live turn, and that work ends the sub-agent's card.
 */
function historyRows(
  turn: HistoryTurn,
  before: HistoryTurn | undefined,
  background: SessionBackground | null,
  ended: boolean,
  live: LiveTurn | undefined,
): Piece[] {
  if (turn.role === "user") {
    const text = userText(turn.blocks);
    const files = userFiles(turn.blocks);
    if (!text && files.length === 0) return [];
    const revert = turn.revertible ? { turn_id: turn.turn_id, before: before ?? null } : null;
    return [{ type: "user", key: turn.turn_id, text, files, ...(revert ? { revert } : {}) }];
  }
  return historyBlockRows(turn.blocks, turn.turn_id, background, ended, live);
}

/** A reply's blocks (or a sub-agent's) as rows; each sub-agent is one card row. */
function historyBlockRows(
  blocks: readonly HistoryBlock[],
  prefix: string,
  background: SessionBackground | null,
  ended: boolean,
  live: LiveTurn | undefined,
): Piece[] {
  // A result is shown with its call; one without a call stands alone if it failed.
  const results = new Map<string, Extract<HistoryBlock, { type: "tool_result" }>>();
  const called = new Set<string>();
  for (const b of blocks) {
    if (b.type === "tool_result" && b.tool_call_id) results.set(b.tool_call_id, b);
    if (b.type === "tool_call" && b.tool_call_id) called.add(b.tool_call_id);
  }
  const cards = new Map(historySubagentCards(blocks, background, ended).map((c) => [c.parent_id, c]));
  // Cards are keyed by launch order, never by id: ids can differ after a reload.
  let launches = 0;
  const rows: Piece[] = [];
  blocks.forEach((b, i) => {
    const key = `${prefix}:${i}`;
    switch (b.type) {
      case "text":
        if (b.text.trim()) rows.push({ type: "agent", key, text: b.text.trim(), streaming: false });
        return;
      case "thought":
        if (b.text.trim()) rows.push({ type: "thought", key, text: b.text.trim(), streaming: false });
        return;
      case "tool_call": {
        const card = b.tool_call_id ? cards.get(b.tool_call_id) : undefined;
        if (card) {
          const at = `${prefix}:agent${launches++}`;
          // Its result's text repeats the sub-agent's last reply; its images, after what it did, do not.
          const images = imageRows(at, results.get(card.parent_id)?.images);
          const going = card.status === "in_progress";
          const late = live ? lateSubagentBlocks(card, live) : [];
          const inner = fold([
            ...historyBlockRows(card.blocks, at, background, ended && !going, live),
            ...(live && late.length > 0 ? liveBlockRows(live, late, `${at}:late`, going, background) : []),
            ...images,
          ]);
          rows.push({ type: "subagent", key: at, agent: subagentItem(card, b.title, inner) });
          return;
        }
        const r = b.tool_call_id ? results.get(b.tool_call_id) : undefined;
        // Transcripts often omit the status; a result means it ran.
        const status = r?.is_error ? "failed" : (b.status ?? (r ? "completed" : null));
        const call = historyTool(b.title, b.input_preview);
        rows.push({
          type: "tool",
          key,
          tool: toolItem({
            kind: null,
            verb: call.verb,
            title: call.target,
            state: toolState(status, false),
            input: call.input,
            output: r?.output_preview ?? null,
            changes: toolChanges(b.title, b.input_preview, r?.output_preview ?? null, status === "failed"),
          }),
        });
        rows.push(...imageRows(key, r?.images));
        return;
      }
      case "tool_result":
        if (b.tool_call_id && called.has(b.tool_call_id)) return;
        rows.push(...imageRows(key, b.images));
        if (b.is_error && b.output_preview) rows.push({ type: "error", key, message: b.output_preview });
        return;
      case "agent_image":
        rows.push({ type: "image", key, image: b });
        return;
      default:
        return;
    }
  });
  return rows;
}

function liveRows(
  turn: LiveTurn,
  prefix: string,
  running: boolean,
  background: SessionBackground | null,
  earlier: ReadonlySet<string>,
): Piece[] {
  const rows: Piece[] = [];
  const turnKey = `${prefix}:${turn.userMessage?.message_id ?? turn.message?.message_id ?? "turn"}`;
  const text = turn.userMessage ? userText(turn.userMessage.blocks) : "";
  const files = turn.userMessage ? userFiles(turn.userMessage.blocks) : [];
  if (text || files.length > 0) rows.push({ type: "user", key: `${turnKey}:user`, text, files });
  rows.push(...liveBlockRows(turn, undefined, turnKey, running, background, earlier));
  return rows;
}

/**
 * The live reply's blocks, or given a card's `blocks` that sub-agent's, as
 * rows; each sub-agent is one card row. For the reply, a sub-agent whose
 * launch is not in it comes last, unless its launch is in `earlier` (the
 * history's card shows it).
 */
function liveBlockRows(
  turn: LiveTurn,
  blocks: readonly LiveBlock[] | undefined,
  prefix: string,
  running: boolean,
  background: SessionBackground | null,
  earlier?: ReadonlySet<string>,
): Piece[] {
  const scope = blocks ?? turn.message?.content ?? [];
  const all = liveSubagentCards(turn, blocks, background, earlier);
  const cards = new Map(all.map((c) => [c.parent_id, c]));
  const inScope = new Set(scope.flatMap((b) => (b.type === "tool_call" ? [b.tool_call_id] : [])));
  // Cards are keyed by launch order, never by id: ids can differ after a reload.
  let launches = 0;
  const cardRow = (card: SubagentCard<LiveBlock>): Piece => {
    const key = `${prefix}:agent${launches++}`;
    const going = active(card.status);
    const images = imageRows(key, turn.toolCalls[card.parent_id]?.images);
    const inner = fold([...liveBlockRows(turn, card.blocks, key, going, background), ...images]);
    return { type: "subagent", key, agent: subagentItem(card, turn.toolCalls[card.parent_id]?.title ?? null, inner) };
  };
  const rows: Piece[] = [];
  scope.forEach((b, i) => {
    const key = `${prefix}:${i}`;
    const last = running && i === scope.length - 1;
    if (b.type === "text") {
      if (b.text.trim() || last) rows.push({ type: "agent", key, text: b.text.trim(), streaming: last });
    } else if (b.type === "thought") {
      if (b.text.trim() || last) rows.push({ type: "thought", key, text: b.text.trim(), streaming: last });
    } else if (b.type === "agent_image") {
      rows.push({ type: "image", key, image: b });
    } else {
      const card = cards.get(b.tool_call_id);
      if (card) {
        rows.push(cardRow(card));
        return;
      }
      const tc = turn.toolCalls[b.tool_call_id];
      rows.push({ type: "tool", key, tool: liveTool(tc, running) });
      rows.push(...imageRows(key, tc?.images));
    }
  });
  for (const card of all) if (!inScope.has(card.parent_id)) rows.push(cardRow(card));
  return rows;
}

/** Runs of `FOLD_AT` or more tool calls become one row. */
function fold(rows: Piece[]): TranscriptRow[] {
  const out: TranscriptRow[] = [];
  let run: Extract<Piece, { type: "tool" }>[] = [];
  const flush = () => {
    if (run.length >= FOLD_AT) out.push({ type: "tools", key: run[0]!.key, tools: run.map((r) => r.tool) });
    else out.push(...run);
    run = [];
  };
  for (const r of rows) {
    if (r.type === "tool") run.push(r);
    else {
      flush();
      out.push(r);
    }
  }
  flush();
  return out;
}

/** "3m 12s", "42s", "1h 5m". */
export const duration = turnDuration;

function lastIndex<T>(list: readonly T[], match: (item: T) => boolean): number {
  for (let i = list.length - 1; i >= 0; i--) if (match(list[i]!)) return i;
  return -1;
}

const STOPPED: Record<string, string> = {
  cancelled: "Stopped",
  max_tokens: "Stopped · reply too long",
  max_turn_requests: "Stopped · step limit reached",
  refusal: "The agent declined to continue",
};

/** Why the last turn stopped early; null when it simply finished. */
function stopText(view: SessionView): string | null {
  const reason = view.lastStopReason;
  if (!reason || reason === "end_turn") return null;
  return STOPPED[reason] ?? `Stopped · ${reason.replace(/_/g, " ")}`;
}

/**
 * The history without the turns shown live. An agent writes its transcript
 * as it goes, so a history read during a turn (after a reconnect, say) can
 * already hold the start of the turn the live view is showing.
 */
export function historyBefore(history: readonly HistoryTurn[], view: Pick<SessionView, "live" | "settled">): HistoryTurn[] {
  let turns = [...history];
  for (const turn of [view.live, view.settled]) {
    if (!turn?.userMessage) continue;
    const key = userKey(turn.userMessage.blocks);
    const at = lastIndex(turns, (t) => t.role === "user");
    if (key && at >= 0 && userKey(turns[at]!.blocks) === key) turns = turns.slice(0, at);
  }
  return turns;
}

/** The transcript, oldest first. */
export function transcriptRows(view: SessionView | undefined): TranscriptRow[] {
  return transcriptBuilder()(view);
}

/** What the history's rows depend on; the same inputs give the same rows. */
type HistoryInputs = readonly unknown[];

const sameInputs = (a: HistoryInputs, b: HistoryInputs) => a.length === b.length && a.every((x, i) => x === b[i]);

/**
 * `transcriptRows`, keeping the history's rows from one call to the next
 * while only the live turn changes, so a list drawing them redraws only the
 * live rows.
 */
export function transcriptBuilder(): (view: SessionView | undefined) => TranscriptRow[] {
  let kept: { inputs: HistoryInputs; pieces: Piece[]; rows: TranscriptRow[] } | null = null;
  let shown: { inputs: HistoryInputs; turns: HistoryTurn[]; launches: Map<string, number> } | null = null;
  return (view) => {
    if (!view) return [];
    const running = view.agentStatus === "prompting";
    const busy = running || !!view.pendingRequest;
    const stopped = busy ? null : stopText(view);
    const background = view.background ?? null;
    const shownLive = !!view.settled || !!view.live.userMessage || !!view.live.message;
    const turnInputs = [
      view.history,
      view.live.userMessage && userKey(view.live.userMessage.blocks),
      view.settled?.userMessage && userKey(view.settled.userMessage.blocks),
    ];
    if (!shown || !sameInputs(shown.inputs, turnInputs)) {
      const turns = historyBefore(view.history ?? [], view);
      // Each launch in the history, by its turn: a background sub-agent can
      // keep working after its turn, and its live work shows on its card there.
      const launches = new Map(turns.flatMap((t, i) => [...historyLaunches([t])].map((id) => [id, i] as const)));
      shown = { inputs: turnInputs, turns, launches };
    }
    const { turns, launches } = shown;
    const lateTurns = new Set(Object.keys(view.live.subagents ?? {}).flatMap((id) => launches.get(id) ?? []));
    const inputs = [
      ...turnInputs,
      background,
      shownLive,
      busy,
      stopped,
      // Only while a history card takes live work, so the rest of the time
      // the live turn streaming leaves the history's rows alone.
      lateTurns.size > 0 ? view.live.subagents : null,
      lateTurns.size > 0 ? view.live.toolCalls : null,
    ];
    if (!kept || !sameInputs(kept.inputs, inputs)) {
      const pieces = historyPieces(view, turns, lateTurns, busy, stopped, shownLive, background);
      kept = { inputs, pieces, rows: fold(pieces) };
    }
    const earlier = new Set(launches.keys());
    const live: Piece[] = [];
    if (view.settled) live.push(...liveRows(view.settled, "settled", false, background, earlier));
    live.push(...liveRows(view.live, "live", running, background, earlier));
    // A run of tool calls across the history's end and the live turn folds as one.
    const joined = kept.pieces.at(-1)?.type === "tool" && live[0]?.type === "tool";
    const rows = joined ? fold([...kept.pieces, ...live]) : [...kept.rows, ...fold(live)];
    if (view.lastError) rows.push({ type: "error", key: "last-error", message: view.lastError.message });
    // A turn not yet in the history gets its time once the history has it.
    if (stopped && shownLive) rows.push({ type: "footer", key: "footer", text: stopped, stopped: true, files: [] });
    return rows;
  };
}

function historyPieces(
  view: SessionView,
  turns: readonly HistoryTurn[],
  lateTurns: ReadonlySet<number>,
  busy: boolean,
  stopped: string | null,
  shownLive: boolean,
  background: SessionBackground | null,
): Piece[] {
  // Under each answered prompt: how long it ran, when it ended, what it changed.
  const groups = new Map(turnGroups(turns).map((g) => [g.end, g]));
  const pieces: Piece[] = [];
  turns.forEach((turn, i) => {
    const last = i === turns.length - 1 && !shownLive;
    const live = lateTurns.has(i) ? view.live : undefined;
    pieces.push(...historyRows(turn, turns[i - 1], background, !(last && busy), live));
    const g = groups.get(i);
    // The agent may still be answering the history's last prompt.
    if (!g || (last && busy)) return;
    const time = [g.durationMs !== null ? turnDuration(g.durationMs) : "", g.completedAt ? clockTime(g.completedAt) : ""];
    const stop = last ? stopped : null;
    const text = [stop, ...time].filter(Boolean).join(" · ");
    pieces.push({ type: "footer", key: `footer:${turn.turn_id}`, text, stopped: !!stop, files: g.files });
  });
  return pieces;
}

// ─── the Now bar ─────────────────────────────────────────────────────────

export type NowState =
  | { type: "working"; text: string }
  | { type: "thinking" }
  | { type: "needs_you" }
  | { type: "reconnecting" }
  | { type: "disconnected" }
  /** Idle, while background work the agent started is still outstanding; `since` in ms. */
  | { type: "waiting"; count: number; since: number };

/** Background work the idle agent waits on; null unless the Host says it waits. */
function waiting(background: SessionBackground | null | undefined): NowState | null {
  if (!background || background.outstanding <= 0 || !background.waiting_since) return null;
  const since = Date.parse(background.waiting_since);
  return Number.isNaN(since) ? null : { type: "waiting", count: background.outstanding, since };
}

/** What the agent is doing, from the live turn alone; null when there is nothing to say. */
export function nowState(view: SessionView | undefined, connected: boolean): NowState | null {
  if (!connected) return { type: "reconnecting" };
  if (!view) return null;
  if (view.pendingRequest) return { type: "needs_you" };
  if (view.attach !== "live") return null;
  switch (view.agentStatus) {
    case "connecting":
      return { type: "working", text: "Starting the agent" };
    case "disconnected":
      return { type: "disconnected" };
    case "prompting":
      break;
    default:
      return waiting(view.background);
  }
  const calls = Object.values(view.live.toolCalls);
  for (let i = calls.length - 1; i >= 0; i--) {
    const tc = calls[i]!;
    if (active(tc.status)) {
      const t = liveTool(tc, true);
      return { type: "working", text: t.verb ? `${VERB_ING[t.verb] ?? t.verb} ${t.target}` : t.target };
    }
  }
  const last = view.live.message?.content.at(-1);
  if (last?.type === "thought") return { type: "thinking" };
  if (last?.type === "text") return { type: "working", text: "Responding" };
  return { type: "working", text: "Working" };
}

const VERB_ING: Record<string, string> = {
  Read: "Reading",
  Edit: "Editing",
  Delete: "Deleting",
  Move: "Moving",
  Search: "Searching",
  Run: "Running",
  Fetch: "Fetching",
  Think: "Thinking about",
};

/** The Stop button shows while a turn runs. */
export function turnRunning(view: SessionView | undefined): boolean {
  return view?.attach === "live" && view.agentStatus === "prompting";
}

/** What identifies the running turn, for timing it on this phone. */
export function turnKey(session_id: string, view: SessionView | undefined): string | null {
  if (!turnRunning(view)) return null;
  return `${session_id}:${view!.live.userMessage?.message_id ?? "turn"}`;
}

/** A running timer in mono: "0:42", "12:05", "1:02:10". */
export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const two = (n: number) => String(n).padStart(2, "0");
  if (s < 3600) return `${Math.floor(s / 60)}:${two(s % 60)}`;
  return `${Math.floor(s / 3600)}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}`;
}
