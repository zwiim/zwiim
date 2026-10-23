// Sub-agent cards: one reading, shared by Desktop and Mobile, of what each
// sub-agent of a turn did, live and after reload.
//
// A sub-agent runs inside one tool call of the turn (its launch). The Host
// keeps everything the sub-agent produced apart from the reply (Host
// capability `agent.subagents`): live, the session reducer collects it in
// `LiveTurn.subagents` by launch; on reload it is nested in the launch's
// `subagent.blocks`. A sub-agent's own launches nest the same way, so the
// cards of a card's `blocks` are its sub-agents.

import type { LiveTurn } from "../state/session";
import type { HistoryBlock, LiveBlock, SubagentInfo, ToolCallStatus } from "../types/protocol";

/** What one sub-agent's card shows. Unknown fields are null. */
export interface SubagentCard<B> {
  /** The tool call that launched it. */
  parent_id: string;
  description: string | null;
  model: string | null;
  prompt: string | null;
  /** The launch's status. */
  status: ToolCallStatus | null;
  duration_ms: number | null;
  /** Tool calls the sub-agent itself made, not counting its sub-agents'. */
  tool_count: number;
  /** Everything it produced, in order. */
  blocks: B[];
}

function card<B extends { type: string }>(
  parent_id: string,
  info: SubagentInfo | undefined,
  status: ToolCallStatus | null,
  blocks: readonly B[],
): SubagentCard<B> {
  const ids = new Set<string>();
  for (const b of blocks) {
    const id = (b as { tool_call_id?: unknown }).tool_call_id;
    if (b.type === "tool_call" && typeof id === "string") ids.add(id);
  }
  return {
    parent_id,
    description: info?.description ?? null,
    model: info?.model ?? null,
    prompt: info?.prompt ?? null,
    status,
    duration_ms: info?.duration_ms ?? null,
    tool_count: ids.size,
    blocks: [...blocks],
  };
}

/**
 * The cards of the sub-agents launched in the reply, in launch order, or,
 * given a card's `blocks`, of the sub-agents that sub-agent launched. A
 * launch is a tool call that says it launches one or that a sub-agent has
 * spoken under. For the reply, a sub-agent whose launch is nowhere in the
 * turn comes last, so nothing it did is hidden.
 */
export function liveSubagentCards(live: LiveTurn, blocks?: readonly LiveBlock[]): SubagentCard<LiveBlock>[] {
  const groups = live.subagents ?? {};
  const cardOf = (parent_id: string) => {
    const call = live.toolCalls[parent_id];
    return card(parent_id, call?.subagent, call?.status ?? null, groups[parent_id]?.blocks ?? []);
  };
  const scope = blocks ?? live.message?.content ?? [];
  const cards: SubagentCard<LiveBlock>[] = [];
  const seen = new Set<string>();
  for (const b of scope) {
    if (b.type !== "tool_call" || seen.has(b.tool_call_id)) continue;
    if (live.toolCalls[b.tool_call_id]?.subagent === undefined && groups[b.tool_call_id] === undefined) continue;
    seen.add(b.tool_call_id);
    cards.push(cardOf(b.tool_call_id));
  }
  if (blocks !== undefined) return cards;
  // Launches the reply reaches, at any depth; the rest are orphans.
  const reachable = new Set<string>();
  const visit = (bs: readonly LiveBlock[]) => {
    for (const b of bs) {
      if (b.type !== "tool_call" || reachable.has(b.tool_call_id)) continue;
      reachable.add(b.tool_call_id);
      visit(groups[b.tool_call_id]?.blocks ?? []);
    }
  };
  visit(scope);
  for (const parent_id of Object.keys(groups)) if (!reachable.has(parent_id)) cards.push(cardOf(parent_id));
  return cards;
}

type HistoryToolCall = Extract<HistoryBlock, { type: "tool_call" }>;
type HistoryLaunch = HistoryToolCall & { tool_call_id: string; subagent: NonNullable<HistoryToolCall["subagent"]> };

/**
 * The cards of the sub-agents launched in a reloaded turn's `blocks` (or a
 * card's), in launch order. The status is the launch's, else its result's.
 */
export function historySubagentCards(blocks: readonly HistoryBlock[]): SubagentCard<HistoryBlock>[] {
  const isError = new Map<string, boolean>();
  for (const b of blocks) if (b.type === "tool_result" && b.tool_call_id) isError.set(b.tool_call_id, b.is_error);
  return blocks
    .filter((b): b is HistoryLaunch => b.type === "tool_call" && b.subagent !== undefined && b.tool_call_id !== null)
    .map((b) => {
      const error = isError.get(b.tool_call_id);
      const status = b.status ?? (error === undefined ? null : error ? "failed" : "completed");
      return card(b.tool_call_id, b.subagent, status, b.subagent.blocks ?? []);
    });
}
