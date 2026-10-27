// Sub-agent cards: one reading, shared by Desktop and Mobile, of what each
// sub-agent of a turn did, live and after reload.
//
// A sub-agent runs inside one tool call of the turn (its launch). The Host
// keeps everything the sub-agent produced apart from the reply (Host
// capability `agent.subagents`): live, the session reducer collects it in
// `LiveTurn.subagents` by launch; on reload it is nested in the launch's
// `subagent.blocks`. A sub-agent's own launches nest the same way, so the
// cards of a card's `blocks` are its sub-agents.
//
// A sub-agent launched to run in the background answers its launch at once,
// so the launch reads finished while the sub-agent still works. Given the
// session's background work (Host capability `session.background`), a card
// whose launch is the `tool_call_id` of an outstanding task reads running,
// and counts that task's tokens so far (Host capability `background.tokens`).
//
// A background sub-agent can keep working after its turn ended, and after the
// transcript was loaded. What it does then arrives live, under a launch that
// only the loaded transcript holds: `lateSubagentBlocks` is that tail, shown
// under the reloaded card, and the reply leaves those groups out
// (`liveSubagentCards`'s `earlier`).

import type { LiveTurn } from "../state/session";
import type {
  HistoryBlock,
  HistoryTurn,
  LiveBlock,
  SessionBackground,
  SubagentInfo,
  ToolCallStatus,
} from "../types/protocol";

/**
 * A card's status: its launch's, or `interrupted` when the sub-agent stopped
 * before it finished (the turn ended, or the session stopped, without its
 * result). `interrupted` is the SDK's reading, never sent by the Host.
 */
export type SubagentCardStatus = ToolCallStatus | "interrupted";

/** What one sub-agent's card shows. Unknown fields are null. */
export interface SubagentCard<B> {
  /** The tool call that launched it. */
  parent_id: string;
  description: string | null;
  model: string | null;
  prompt: string | null;
  /**
   * The launch's status; `in_progress` (running) while the sub-agent is an
   * outstanding background task, `interrupted` when it stopped unfinished.
   */
  status: SubagentCardStatus | null;
  duration_ms: number | null;
  /**
   * When it started, in ms since the epoch, from the Host's `started_at`
   * (Host capability `subagent.started_at`); null when the Host did not say.
   */
  started_at: number | null;
  /**
   * Tokens it spent, once its result says; while it runs in the background,
   * its task's count so far (Host capability `background.tokens`).
   */
  tokens: number | null;
  /**
   * Tool calls the sub-agent itself made, not counting its sub-agents': its
   * result's count once it has one, else the calls seen so far.
   */
  tool_count: number;
  /** Everything it produced, in order. */
  blocks: B[];
}

/** Launches of the background tasks still running, each with its task's tokens so far. */
function runningLaunches(background: SessionBackground | null | undefined): Map<string, number | undefined> {
  const ids = new Map<string, number | undefined>();
  if (!background || background.outstanding === 0) return ids;
  for (const t of background.tasks) if (t.tool_call_id !== undefined) ids.set(t.tool_call_id, t.tokens);
  return ids;
}

/** A `started_at` in ms since the epoch, or null when absent or unreadable. */
function startedAt(at: string | undefined): number | null {
  if (at === undefined) return null;
  const ms = Date.parse(at);
  return Number.isNaN(ms) ? null : ms;
}

function card<B extends { type: string }>(
  parent_id: string,
  info: SubagentInfo | undefined,
  status: SubagentCardStatus | null,
  blocks: readonly B[],
  running?: { tokens: number | undefined },
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
    started_at: startedAt(info?.started_at),
    tokens: running?.tokens ?? info?.tokens ?? null,
    tool_count: info?.tool_uses ?? ids.size,
    blocks: [...blocks],
  };
}

/**
 * The cards of the sub-agents launched in the reply, in launch order, or,
 * given a card's `blocks`, of the sub-agents that sub-agent launched. A
 * launch is a tool call that says it launches one or that a sub-agent has
 * spoken under. For the reply, a sub-agent whose launch is nowhere in the
 * turn comes last, so nothing it did is hidden, unless its launch is in
 * `earlier` (`historyLaunches`): its card is there.
 */
export function liveSubagentCards(
  live: LiveTurn,
  blocks?: readonly LiveBlock[],
  background?: SessionBackground | null,
  earlier?: ReadonlySet<string>,
): SubagentCard<LiveBlock>[] {
  const groups = live.subagents ?? {};
  const running = runningLaunches(background);
  const cardOf = (parent_id: string) => {
    const call = live.toolCalls[parent_id];
    const task = running.has(parent_id) ? { tokens: running.get(parent_id) } : undefined;
    const status = task ? "in_progress" : (call?.status ?? null);
    return card(parent_id, call?.subagent, status, groups[parent_id]?.blocks ?? [], task);
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
  for (const parent_id of Object.keys(groups)) {
    if (!reachable.has(parent_id) && !earlier?.has(parent_id)) cards.push(cardOf(parent_id));
  }
  return cards;
}

type HistoryToolCall = Extract<HistoryBlock, { type: "tool_call" }>;
type HistoryResult = Extract<HistoryBlock, { type: "tool_result" }>;
type HistoryLaunch = HistoryToolCall & { tool_call_id: string; subagent: NonNullable<HistoryToolCall["subagent"]> };

const isLaunch = (b: HistoryBlock): b is HistoryLaunch =>
  b.type === "tool_call" && b.subagent !== undefined && b.tool_call_id !== null;

const BACKGROUND_STATUS = { completed: "completed", failed: "failed", stopped: "interrupted" } as const;

/**
 * The cards of the sub-agents launched in a reloaded turn's `blocks` (or a
 * card's), in launch order. The status is the launch's, else its result's,
 * unless the sub-agent is still running in the background; a background
 * launch says how its task ended. `ended` is whether the turn (or the card
 * holding `blocks`) is over: a sub-agent left unfinished then reads
 * `interrupted`. Pass false for a turn that may still be running.
 */
export function historySubagentCards(
  blocks: readonly HistoryBlock[],
  background?: SessionBackground | null,
  ended = true,
): SubagentCard<HistoryBlock>[] {
  const running = runningLaunches(background);
  const results = new Map<string, HistoryResult>();
  for (const b of blocks) if (b.type === "tool_result" && b.tool_call_id) results.set(b.tool_call_id, b);
  return blocks.filter(isLaunch).map((b) => {
    const result = results.get(b.tool_call_id);
    const task = running.has(b.tool_call_id) ? { tokens: running.get(b.tool_call_id) } : undefined;
    let status: SubagentCardStatus | null;
    if (task) {
      status = "in_progress";
    } else if (result?.background !== undefined) {
      // Not running and the transcript never said how it ended: it died
      // with the session. Unknown when the session's work is unknown.
      status =
        result.background === null
          ? ended && background
            ? "interrupted"
            : null
          : BACKGROUND_STATUS[result.background];
    } else {
      status = b.status ?? (result === undefined ? null : result.is_error ? "failed" : "completed");
      if (ended && result === undefined && (status === null || status === "pending" || status === "in_progress")) {
        status = "interrupted";
      }
    }
    return card(b.tool_call_id, b.subagent, status, b.subagent.blocks ?? [], task);
  });
}

/** Every launch in `turns`, sub-agents' own included. */
export function historyLaunches(turns: readonly HistoryTurn[]): Set<string> {
  const ids = new Set<string>();
  const visit = (blocks: readonly HistoryBlock[]) => {
    for (const b of blocks) {
      if (!isLaunch(b)) continue;
      ids.add(b.tool_call_id);
      visit(b.subagent.blocks ?? []);
    }
  };
  for (const t of turns) visit(t.blocks);
  return ids;
}

/**
 * What the sub-agent of a reloaded `card` did after its transcript was
 * loaded, arriving live; empty when nothing did. Tool calls the card already
 * shows are left out.
 */
export function lateSubagentBlocks(card: SubagentCard<HistoryBlock>, live: LiveTurn): LiveBlock[] {
  const group = live.subagents?.[card.parent_id];
  if (!group) return [];
  const shown = new Set<string>();
  for (const b of card.blocks) if (b.type === "tool_call" && b.tool_call_id) shown.add(b.tool_call_id);
  return group.blocks.filter((b) => b.type !== "tool_call" || !shown.has(b.tool_call_id));
}
