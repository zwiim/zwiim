// The can't-reach card's logic (DESIGN.md, Screen 4): its words, its look, and
// what a tap does. Kept apart from the view so it can be tested without one.
// Nothing here runs by itself: an op goes to the Host only on a tap, and an
// unknown outcome is shown, never sent again.

import { isClientError } from "@zwiim/sdk";

export type RetryChoice = "retry" | "new_chat";

export const RETRY_LABEL = "Retry →";
export const RETRY_NEW_CHAT_LABEL = "Retry in new chat →";

/** Each button is a full touch target, not just its text. */
export const RETRY_TARGET = 44;

/** What the card says: the agent that could not reach its provider, and why. */
export const retryTitle = (agentName: string, message: string) => `${agentName} error: ${message}`;

/** Both buttons are off while an op is out, and while the phone cannot reach the Host. */
export const retryBusy = (pending: RetryChoice | null, online: boolean) => pending !== null || !online;

/** `surface` card, `err` left edge, buttons in `muted`, a refusal in `err`. */
export function retryColors(p: { surface: string; err: string; mut: string }) {
  return { background: p.surface, edge: p.err, action: p.mut, refusal: p.err };
}

/** Why a retry did not happen, in words. */
export function retryError(e: unknown, hostName: string): string {
  if (isClientError(e, "nothing_to_retry")) return "There is nothing to retry.";
  if (isClientError(e, "turn_in_progress")) return "The agent is still working on the previous message. Wait for it to finish, or stop it.";
  if (isClientError(e, "unknown_outcome")) {
    return `The connection to ${hostName} dropped before it answered. The retry may or may not have been applied. Check the session before trying again.`;
  }
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't retry. Try again.";
}

/**
 * One tap: both buttons hold until `op` answers, and a refusal is handed to
 * `set.error` to show (freeing the buttons). Never repeats `op`. When `op`
 * went through, the buttons stay held: the card is still up until the new
 * turn clears the error, and a second tap there would only be refused.
 * `after` gets the result once the op is done, outside the refusal handling,
 * so a throw there is never shown as a failed retry.
 */
export async function runRetry<T>(
  which: RetryChoice,
  op: () => Promise<T>,
  set: { pending(p: RetryChoice | null): void; error(text: string | null): void },
  opts: { hostName: string; after?(result: T): void },
): Promise<void> {
  set.pending(which);
  set.error(null);
  let result: T;
  try {
    result = await op();
  } catch (e) {
    set.error(retryError(e, opts.hostName));
    set.pending(null);
    return;
  }
  opts.after?.(result);
}
