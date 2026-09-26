// A session as the Session screen reads it (DESIGN.md, Screen 5): the
// transcript as rows, and the Now bar's one line on what the agent is doing.
// Derived from the SDK's `SessionView` alone: the persisted history, a turn
// that finished but isn't in it yet, and the live turn. The SDK drops
// replayed events by sequence, so rebuilding these after a reconnect never
// repeats a message or a tool call.

import type { HistoryBlock, HistoryTurn, LiveTurn, SessionView, ToolCall, ToolCallStatus } from "@zwiim/sdk";

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
}

export type TranscriptRow =
  | { type: "user"; key: string; text: string }
  | { type: "agent"; key: string; text: string; streaming: boolean }
  | { type: "thought"; key: string; text: string; streaming: boolean }
  | { type: "tool"; key: string; tool: ToolItem }
  | { type: "tools"; key: string; tools: ToolItem[] }
  | { type: "error"; key: string; message: string }
  | { type: "footer"; key: string; text: string; stopped: boolean };

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

function toolItem(p: {
  kind: string | null | undefined;
  verb?: string | null;
  title: string | null;
  state: ToolState;
  input: string | null;
  output: string | null;
}): ToolItem {
  const title = unquote(p.title ?? "") || "Tool call";
  let verb = p.verb ?? (p.kind ? (VERBS[p.kind] ?? null) : null);
  // The agent's title often starts with the verb already ("Read src/a.ts").
  if (verb && title.toLowerCase().startsWith(verb.toLowerCase() + " ")) verb = null;
  return { verb, target: title, state: p.state, input: p.input, output: p.output, ...diffStats(p.output) };
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

type Piece = Exclude<TranscriptRow, { type: "tools" | "footer" }>;

function historyRows(turn: HistoryTurn): Piece[] {
  if (turn.role === "user") {
    const text = userText(turn.blocks);
    return text ? [{ type: "user", key: turn.turn_id, text }] : [];
  }
  // A result is shown with its call; one without a call stands alone if it failed.
  const results = new Map<string, Extract<HistoryBlock, { type: "tool_result" }>>();
  const called = new Set<string>();
  for (const b of turn.blocks) {
    if (b.type === "tool_result" && b.tool_call_id) results.set(b.tool_call_id, b);
    if (b.type === "tool_call" && b.tool_call_id) called.add(b.tool_call_id);
  }
  const rows: Piece[] = [];
  turn.blocks.forEach((b, i) => {
    const key = `${turn.turn_id}:${i}`;
    switch (b.type) {
      case "text":
        if (b.text.trim()) rows.push({ type: "agent", key, text: b.text.trim(), streaming: false });
        return;
      case "thought":
        if (b.text.trim()) rows.push({ type: "thought", key, text: b.text.trim(), streaming: false });
        return;
      case "tool_call": {
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
          }),
        });
        return;
      }
      case "tool_result":
        if (b.is_error && b.output_preview && !(b.tool_call_id && called.has(b.tool_call_id))) {
          rows.push({ type: "error", key, message: b.output_preview });
        }
        return;
      default:
        return;
    }
  });
  return rows;
}

function liveRows(turn: LiveTurn, prefix: string, running: boolean): Piece[] {
  const rows: Piece[] = [];
  const turnKey = `${prefix}:${turn.userMessage?.message_id ?? turn.message?.message_id ?? "turn"}`;
  const text = turn.userMessage ? userText(turn.userMessage.blocks) : "";
  if (text) rows.push({ type: "user", key: `${turnKey}:user`, text });
  const content = turn.message?.content ?? [];
  content.forEach((b, i) => {
    const key = `${turnKey}:${i}`;
    const last = running && i === content.length - 1;
    if (b.type === "text") {
      if (b.text.trim() || last) rows.push({ type: "agent", key, text: b.text.trim(), streaming: last });
    } else if (b.type === "thought") {
      if (b.text.trim() || last) rows.push({ type: "thought", key, text: b.text.trim(), streaming: last });
    } else {
      rows.push({ type: "tool", key, tool: liveTool(turn.toolCalls[b.tool_call_id], running) });
    }
  });
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
export function duration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

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

/** How the last turn ended: `✓ Turn finished · 3m 12s · 14 tool calls`. */
function footer(view: SessionView, rows: Piece[]): TranscriptRow | null {
  const reason = view.lastStopReason;
  if (reason && reason !== "end_turn") {
    return { type: "footer", key: "footer", text: STOPPED[reason] ?? `Stopped · ${reason.replace(/_/g, " ")}`, stopped: true };
  }
  const history = view.history ?? [];
  const lastUser = lastIndex(history, (t) => t.role === "user");
  const after = history.slice(lastUser + 1);
  const answered = view.settled ? view.settled.message !== null : after.some((t) => t.role !== "user");
  if (!answered || rows.length === 0) return null;
  const parts = ["Turn finished"];
  if (!view.settled && lastUser >= 0) {
    const end = after.at(-1)!;
    const ms = Date.parse(end.completed_at ?? end.timestamp) - Date.parse(history[lastUser]!.timestamp);
    if (ms > 0) parts.push(duration(ms));
  }
  const lastUserRow = lastIndex(rows, (r) => r.type === "user");
  const tools = rows.slice(lastUserRow + 1).filter((r) => r.type === "tool").length;
  if (tools > 0) parts.push(tools === 1 ? "1 tool call" : `${tools} tool calls`);
  return { type: "footer", key: "footer", text: parts.join(" · "), stopped: false };
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
    const text = userText(turn.userMessage.blocks);
    const at = lastIndex(turns, (t) => t.role === "user");
    if (text && at >= 0 && userText(turns[at]!.blocks) === text) turns = turns.slice(0, at);
  }
  return turns;
}

/** The transcript, oldest first. */
export function transcriptRows(view: SessionView | undefined): TranscriptRow[] {
  if (!view) return [];
  const running = view.agentStatus === "prompting";
  const pieces: Piece[] = [];
  for (const turn of historyBefore(view.history ?? [], view)) pieces.push(...historyRows(turn));
  if (view.settled) pieces.push(...liveRows(view.settled, "settled", false));
  pieces.push(...liveRows(view.live, "live", running));
  const rows = fold(pieces);
  if (view.lastError) rows.push({ type: "error", key: "last-error", message: view.lastError.message });
  const end = running || view.pendingRequest ? null : footer(view, pieces);
  if (end) rows.push(end);
  return rows;
}

// ─── the Now bar ─────────────────────────────────────────────────────────

export type NowState =
  | { type: "working"; text: string }
  | { type: "thinking" }
  | { type: "needs_you" }
  | { type: "reconnecting" }
  | { type: "disconnected" };

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
      return null;
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
