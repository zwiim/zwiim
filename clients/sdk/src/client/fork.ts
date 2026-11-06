// Starting a new session from one of a session's turns (`session.fork`): which
// turn a turn group's actions act on, and the words every client shows around
// it. Platform wording (offline, unknown outcome, fallbacks) stays in each app.

import type { TurnGroup } from "../changes/turns";
import type { ForkTarget, HistoryTurn } from "../types/protocol";
import { hasCapability, type Compatibility } from "./compat";
import { isClientError } from "./errors";
import { revertRefusalText } from "./revert";

/** What each fork action is called. */
export const FORK_LABELS: Record<ForkTarget, string> = {
  current: "Fork to new tab",
  new_worktree: "Fork to new worktree",
};

/** Why fork actions are off while the session's agent is running. */
export const FORK_BUSY_NOTE = "Wait for the agent to finish.";

/** Why fork actions are off while the Host can't be reached. */
export const FORK_OFFLINE_NOTE = "Reconnect to fork.";

/** Forking an older turn in place: the agent goes back, the files don't. */
export const FORK_OLDER_TURN_NOTE = "Files stay as they are now; the new session is told so.";

/** Whether this Host forks sessions at all. */
export function offersFork(compat: Compatibility | null): boolean {
  return hasCapability(compat, "session.fork");
}

/**
 * The turn in `group` the Host offers to fork from, null when it offers none.
 * Not always `group.end`: a system turn can follow the agent's last reply.
 */
export function forkTurnOf(history: readonly HistoryTurn[], group: TurnGroup): HistoryTurn | null {
  for (let i = group.end; i > group.start; i--) {
    const turn = history[i]!;
    if (turn.fork_targets?.length) return turn;
  }
  return null;
}

/** Why the Host refused a fork, in words; null for any other failure. */
export function forkRefusalText(e: unknown): string | null {
  if (isClientError(e, "fork_unavailable")) return "This turn can't be forked.";
  if (isClientError(e, "checkpoint_unavailable")) return "The files at this point are no longer available.";
  if (isClientError(e, "root_workspace")) return "This session ran in the repository folder, so it can only fork to a new worktree.";
  if (isClientError(e, "not_found")) return "This session is gone.";
  return revertRefusalText(e);
}
