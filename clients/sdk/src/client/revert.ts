// Resetting a session to before one of its messages (`session.revert`): the
// rule for whether the chat goes back too, and the words every client shows
// around it. Platform wording (offline, unknown outcome, fallbacks) stays in
// each app.

import type { HistoryTurn, SessionOptions, SessionRevertResult } from "../types/protocol";
import { hasCapability, type Compatibility } from "./compat";
import { busyDetails, isClientError } from "./errors";

/** What a reset asks the Host to put back. */
export interface RevertChoice {
  files: boolean;
  chat: boolean;
}

/** The note after a reset that left commits made since the message in place. */
export const HEAD_MOVED_NOTE = "Commits made after this point stay; their changes are undone as uncommitted edits.";

/**
 * Whether a reset to before a message takes the chat back too: the Host
 * offers it, the agent can (not known yet counts as yes; the Host says so if
 * not), and `before` (the history turn before the message) is a reply. The
 * first message has nothing to go back to.
 */
export function offersChatRewind(
  before: Pick<HistoryTurn, "role"> | null | undefined,
  options: SessionOptions | null | undefined,
  compat: Compatibility | null,
): boolean {
  return hasCapability(compat, "session.revert.chat") && options?.can_rewind_chat !== false && !!before && before.role !== "user";
}

const BUSY_OPERATION: Record<string, string> = {
  merge: "a merge is in progress. Finish it, then try again.",
  rebase: "a rebase is in progress. Finish it, then try again.",
  cherry_pick: "a cherry-pick is in progress. Finish it, then try again.",
  revert: "a Git revert is in progress. Finish it, then try again.",
  conflict: "it has unresolved conflicts. Resolve them, then try again.",
  index_lock: "another Git command is running in it. Try again in a moment.",
};

/** Why the Host refused a reset, in words; null for any other failure. */
export function revertRefusalText(e: unknown): string | null {
  if (isClientError(e, "checkpoint_unavailable")) return "This point can't be restored.";
  const busy = busyDetails(e);
  if (busy) {
    const why =
      (busy.operation && BUSY_OPERATION[busy.operation]) ??
      (busy.live_sessions > 0
        ? "another agent is working in it. Try again when it's done."
        : "another Git operation is in progress. Finish it, then try again.");
    return `The workspace is busy: ${why}`;
  }
  if (isClientError(e, "turn_in_progress")) return "The agent is still working. Wait for it to finish, or stop it, then try again.";
  if (isClientError(e, "root_workspace")) return "This session ran in the repository folder, so it can't be reset.";
  return null;
}

/** What to tell the user once a reset is done; nothing when it did all it was asked. */
export function revertNotes(result: SessionRevertResult, asked: RevertChoice): string[] {
  const notes: string[] = [];
  if (asked.chat && !result.chat_rewound) {
    notes.push(
      result.files_restored
        ? "The files were reset, but the chat could not be taken back. Its later messages stay."
        : "The chat could not be taken back. Its later messages stay.",
    );
  }
  if (result.head_moved) notes.push(HEAD_MOVED_NOTE);
  return notes;
}
