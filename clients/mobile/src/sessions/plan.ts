// A session's plan on the phone (DESIGN.md, plan progress): its words, its pip
// states, and what Continue does. Kept apart from the view so it can be tested
// without one. Nothing here runs by itself: Continue goes to the Host only on a
// tap, and an unknown outcome is shown, never sent again.

import { isClientError, type AgentType, type NowState as PlanNowState, type PlanUi } from "@zwiim/sdk";
import type { NowState } from "./transcript";

export const CONTINUE_LABEL = "Continue";
export const CONTINUE_NOTE = "New session · gets the tasks and full history";

/** The Continue button is taller than a plain target: it is the panel's one action. */
export const CONTINUE_HEIGHT = 48;

export const ROOT_REASON = "Continue needs a worktree. Start a new session from a worktree.";
export const ARCHIVED_REASON = "This worktree was archived. Its tasks can't be continued here.";

/** `Stopped at task 3 · <title>`; the title is absent when the step list is shorter than the stop. */
export const stoppedTitle = (n: number, title: string | undefined) =>
  title ? `Stopped at task ${n} · ${title}` : `Stopped at task ${n}`;

export const doneCount = (completed: number, total: number) => `${completed} of ${total} done`;

export type PipState = "done" | "current" | "stopped" | "pending";

/** One pip per step: done, the one being worked on, the one it stopped at, or still to do. */
export function pipStates(ui: PlanUi): PipState[] {
  if (ui.kind === "working") {
    return ui.progress.steps.map((s) => (s.status === "completed" ? "done" : s.status === "in_progress" ? "current" : "pending"));
  }
  if (ui.kind === "stopped") {
    return ui.plan.steps.map((s, i) => (i === ui.stoppedAt ? "stopped" : s.status === "completed" ? "done" : "pending"));
  }
  return [];
}

/** Most pips a row shows: past that the row is a window, so it never wraps or grows. */
export const MAX_PIPS = 10;

/**
 * The pips to draw. A long plan shows `max` of them, around the step in hand
 * (the one being worked on or stopped at; else the first still to do), so the
 * active pip stays on screen. The count text carries the whole.
 */
export function pipWindow(states: readonly PipState[], max = MAX_PIPS): PipState[] {
  if (states.length <= max) return [...states];
  let focus = states.findIndex((s) => s === "current" || s === "stopped");
  if (focus < 0) focus = states.findIndex((s) => s === "pending");
  if (focus < 0) focus = states.length - 1;
  const start = Math.min(Math.max(focus - Math.floor(max / 2), 0), states.length - max);
  return states.slice(start, start + max);
}

/** What the agent is doing now, as the Now bar says it; null when it is not working. */
export function nowActivity(now: NowState | null): string | null {
  return now?.type === "working" ? now.text : now?.type === "thinking" ? "Thinking" : null;
}

/** `Continuing <agent>'s tasks · N of M done`, with whatever the Host recorded. */
export function continuingLabel(agentName: string | null, completed: number | undefined, total: number | undefined) {
  const head = agentName ? `Continuing ${agentName}'s tasks` : "Continuing the tasks";
  return completed !== undefined && total !== undefined ? `${head} · ${doneCount(completed, total)}` : head;
}

/** The button names the agent only when it is not the session's own. */
export const continueLabel = (own: boolean, agentName: string) => (own ? CONTINUE_LABEL : `${CONTINUE_LABEL} with ${agentName}`);

/** `From <old title>`, on the card at the head of a continuing session. */
export const fromLabel = (title: string | null | undefined) => `From ${title || "the previous session"}`;

/** A session's title as the store lists it, in any workspace; undefined when it is not listed. */
export function listedTitle(
  lists: Record<string, readonly { session_id: string; title?: string | null }[]>,
  session_id: string,
): string | null | undefined {
  for (const list of Object.values(lists)) {
    const found = list.find((x) => x.session_id === session_id);
    if (found) return found.title || null;
  }
  return undefined;
}

/** Where a plan went; a session the store does not list is still named. */
export const continuedInLabel = (title: string | null | undefined) => `Continued in ${title || "new session"} →`;

/** The phone's Now line, read the way the plan meter does (it shows only over plain working). */
export function planNowState(now: Pick<NowState, "type"> | null): PlanNowState {
  if (!now) return "idle";
  return now.type === "thinking" ? "working" : now.type;
}

/**
 * What sits over the composer while the agent is idle. The Host must report
 * progress at all; where a plan went shows even after the can't-reach card, but
 * the resume panel does not (that card is the only one) and needs the continue
 * capability. A panel whose plan was continued from here (`gone`) is gone.
 */
export function planSlot(a: {
  ui: PlanUi;
  showsPlan: boolean;
  canContinue: boolean;
  cardShown: boolean;
  gone: string | null;
}): "none" | "continued" | "resume" {
  if (!a.showsPlan) return "none";
  if (a.ui.kind === "continued") return "continued";
  if (a.ui.kind !== "stopped" || !a.canContinue || a.cardShown || a.gone === a.ui.plan.plan_id) return "none";
  return "resume";
}

/** Why a Continue did not happen, in words. */
export function continueError(e: unknown, hostName: string): string {
  if (isClientError(e, "root_workspace")) return ROOT_REASON;
  if (isClientError(e, "workspace_removed")) return ARCHIVED_REASON;
  if (isClientError(e, "unknown_outcome")) {
    return `The connection to ${hostName} dropped before it answered. Continue may or may not have been applied. Check your sessions before trying again.`;
  }
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't continue. Try again.";
}

/** Why Continue cannot run in this workspace at all, said under the button; null when it can. */
export function continueBlock(workspace: { kind?: string; removed?: boolean } | null | undefined): string | null {
  if (!workspace) return null;
  return workspace.kind === "root" ? ROOT_REASON : workspace.removed ? ARCHIVED_REASON : null;
}

export interface ContinueHost {
  continuePlan(sessionId: string, agent?: AgentType): Promise<string>;
  /** Read the session's history again, so the view shows what the Host holds now. */
  reloadSession(sessionId: string): void;
}

/** A session with a Continue out; a second tap there is refused, not sent. */
const out = new Set<string>();

/**
 * One tap on Continue: the panel holds (`pending`) until the Host answers. A
 * refusal is handed to `set.error` to show, freeing the button. Never repeats
 * the op. `agent` is left out for the session's own agent. Returns the new
 * session's id, or null when none was made. The guard below covers only the
 * call itself; the view keeps the button held (`pending`) until the screen
 * leaves. Two refusals mean the history is stale, not that something failed:
 * - `nothing_to_continue` (another device continued it): the panel goes away
 *   and the history is read again; no error.
 * - `turn_in_progress`: the history is read again, which brings the working
 *   meter, and the panel back if the turn stops short; no error.
 */
export async function runContinue(
  host: ContinueHost,
  view: { session_id: string; agent: string },
  agent: AgentType,
  set: { pending(p: boolean): void; error(text: string | null): void; gone(): void },
  hostName = "the Host",
): Promise<string | null> {
  if (out.has(view.session_id)) return null;
  out.add(view.session_id);
  set.pending(true);
  set.error(null);
  try {
    return await host.continuePlan(view.session_id, agent === view.agent ? undefined : agent);
  } catch (e) {
    set.pending(false);
    if (isClientError(e, "nothing_to_continue")) {
      set.gone();
      host.reloadSession(view.session_id);
    } else if (isClientError(e, "turn_in_progress")) {
      host.reloadSession(view.session_id);
    } else {
      set.error(continueError(e, hostName));
    }
    return null;
  } finally {
    out.delete(view.session_id);
  }
}
