// A turn as its end-of-turn row reads it: the user's prompt and every agent
// turn after it, up to the next prompt; how long it ran, when it ended, and
// the files its edits changed.

import type { HistoryBlock, HistoryTurn } from "../types/protocol";
import { toolChanges, type FileDiff } from "./diff";

/** One file a turn changed, its counts summed over the turn's edits. */
export interface ChangedFile {
  path: string;
  added: number;
  removed: number;
}

export interface TurnGroup {
  /** The user turn's index in the history. */
  start: number;
  /** The index of the turn's last agent turn: the row goes under it. */
  end: number;
  /** From the prompt to the turn's end; null when the times don't say. */
  durationMs: number | null;
  /** When the turn ended, as the agent recorded it. */
  completedAt: string | null;
  files: ChangedFile[];
}

/** Changes by file, in the order first changed, counts summed. */
export function mergeChanges(diffs: readonly FileDiff[]): ChangedFile[] {
  const files = new Map<string, ChangedFile>();
  for (const d of diffs) {
    if (!d.path) continue;
    const f = files.get(d.path);
    if (f) {
      f.added += d.added;
      f.removed += d.removed;
    } else files.set(d.path, { path: d.path, added: d.added, removed: d.removed });
  }
  return [...files.values()];
}

type ToolResult = Extract<HistoryBlock, { type: "tool_result" }>;

/** `blocks` and, in place of each sub-agent launch, the launch then its sub-agent's blocks. */
function withSubagentBlocks(blocks: readonly HistoryBlock[]): HistoryBlock[] {
  return blocks.flatMap((b) =>
    b.type === "tool_call" && b.subagent?.blocks ? [b, ...withSubagentBlocks(b.subagent.blocks)] : [b],
  );
}

/**
 * What the edits in these turns changed; a failed edit changed nothing. A
 * sub-agent's edits count as the turn's: they changed the turn's files, and
 * reverting the turn puts them back too.
 */
export function turnChanges(turns: readonly HistoryTurn[]): ChangedFile[] {
  const blocks = turns.flatMap((t) => withSubagentBlocks(t.blocks));
  const results = new Map<string, ToolResult>();
  for (const b of blocks) if (b.type === "tool_result" && b.tool_call_id) results.set(b.tool_call_id, b);
  const diffs: FileDiff[] = [];
  for (const b of blocks) {
    if (b.type !== "tool_call") continue;
    const r = b.tool_call_id ? results.get(b.tool_call_id) : undefined;
    const failed = b.status === "failed" || !!r?.is_error;
    diffs.push(...(toolChanges(b.title, b.input_preview, r?.output_preview ?? null, failed) ?? []));
  }
  return mergeChanges(diffs);
}

/** Each prompt the agent answered, as one turn. A prompt not yet answered has none. */
export function turnGroups(history: readonly HistoryTurn[]): TurnGroup[] {
  const groups: TurnGroup[] = [];
  let i = 0;
  while (i < history.length) {
    if (history[i]!.role !== "user") {
      i++;
      continue;
    }
    const start = i;
    let end = -1;
    for (i = start + 1; i < history.length && history[i]!.role !== "user"; i++) end = i;
    if (end < 0) continue;
    const last = history[end]!;
    const completedAt = last.completed_at ?? last.timestamp ?? null;
    const ms = completedAt ? Date.parse(completedAt) - Date.parse(history[start]!.timestamp) : NaN;
    groups.push({
      start,
      end,
      durationMs: Number.isFinite(ms) && ms >= 0 ? ms : null,
      completedAt,
      files: turnChanges(history.slice(start + 1, end + 1)),
    });
  }
  return groups;
}

/** "42s", "9m 31s", "1h 4m". */
export function turnDuration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** A moment as a local clock time: "6:42 AM". Empty when it can't be read. */
export function clockTime(iso: string): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? "" : at.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}
