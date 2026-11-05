// Forking on the phone: what the SDK leaves to each app. No tooltips here, so a
// fork's warning is a sheet before it runs and its refusal a sheet after.

import {
  FORK_BUSY_NOTE,
  FORK_OFFLINE_NOTE,
  FORK_OLDER_TURN_NOTE,
  forkRefusalText,
  isClientError,
  type ForkTarget,
} from "@zwiim/sdk";

/** A turn's fork actions, as the Host offers them. */
export interface TurnFork {
  turn_id: string;
  targets: readonly ForkTarget[];
  /** A newer turn has finished since. */
  older: boolean;
}

/** What to say before this fork runs; null when it just runs. */
export function forkNote(fork: TurnFork, target: ForkTarget): string | null {
  return fork.older && target === "current" ? FORK_OLDER_TURN_NOTE : null;
}

/** Why fork actions are off; null when they're on. */
export function forkDisabledNote(online: boolean, busy: boolean): string | null {
  if (!online) return FORK_OFFLINE_NOTE;
  return busy ? FORK_BUSY_NOTE : null;
}

/** What to say about a failed fork. */
export function forkError(e: unknown, hostName: string): string {
  const refused = forkRefusalText(e);
  if (refused) return refused;
  if (isClientError(e, "unknown_outcome")) return `Not sure the fork went through. Check your sessions on ${hostName} before trying again.`;
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't fork. Try again.";
}

/** What to say when the Host refused a fork's first message; null for any other failure. */
export function forkedSendError(e: unknown): string | null {
  return isClientError(e, "transcript_missing") ? "This session's transcript is gone. Send again to go on without it." : null;
}

/** The link back from a fork; `title` is null when the session is gone or untitled. */
export const forkedFromLabel = (title: string | null) => `Forked from ${title || "a session that is gone"}`;
