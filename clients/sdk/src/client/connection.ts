// Connection lifecycle of one host:
//
//   disconnected → connecting → handshaking → live
//   live → reconnecting → handshaking → live
//   (any) → unauthenticated | error        terminal; `start()` again to retry
//
// `connecting`/`reconnecting` cover discovery + `host.hello`; `handshaking`
// is the event socket opening; `live` means the socket is open and every
// subscription has been re-sent.

export type ConnectionStatus =
  | "disconnected"
  | "connecting"
  | "handshaking"
  | "live"
  | "reconnecting"
  | "unauthenticated"
  | "error";

export interface ConnectionState {
  status: ConnectionStatus;
  /** Consecutive failed attempts since the last `live`. */
  attempt: number;
  hostId: string | null;
  error: { code: string; message: string } | null;
  /** Delay before the next attempt while `reconnecting`. */
  retryInMs: number | null;
}

export interface BackoffOptions {
  baseMs: number;
  maxMs: number;
}

export const DEFAULT_BACKOFF: BackoffOptions = { baseMs: 500, maxMs: 30_000 };

/**
 * Exponential backoff with ±20% jitter: ~0.5s, 1s, 2s, … capped at `maxMs`.
 * `attempt` is 0 for the first retry.
 */
export function backoffDelay(
  attempt: number,
  random: () => number = Math.random,
  { baseMs, maxMs }: BackoffOptions = DEFAULT_BACKOFF,
): number {
  const exp = Math.min(maxMs, baseMs * 2 ** Math.min(attempt, 30));
  return Math.round(Math.min(maxMs, exp * (0.8 + 0.4 * random())));
}
