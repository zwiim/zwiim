// Terminal stream reconstruction. Host terminal `seq` is contiguous per
// terminal (+1 per output chunk); `0` means the host could not number the
// chunk. The host drops frames for a lagging socket, so a jump in `seq` is
// data loss and must be repaired from `terminal.snapshot`. Pure: the client
// feeds snapshots and events in and performs the returned outputs.

import type { TerminalEvent, TerminalSnapshot } from "../types/protocol";

/** Events buffered while waiting for a snapshot before resyncing again. */
export const MAX_PENDING_EVENTS = 1024;

export interface TerminalCursor {
  /** Seq of the last chunk applied to the screen. */
  seq: number;
  /** A snapshot has been applied and `seq` is trustworthy. */
  synced: boolean;
  /** Events that arrived before the snapshot, in arrival order. */
  pending: TerminalEvent[];
  /** Pending buffer overflowed; the next snapshot is already stale. */
  overflowed: boolean;
  alive: boolean;
}

/** What a terminal view must do. */
export type TerminalOutput =
  /** Clear the screen and write `data`. */
  | { kind: "reset"; data: string }
  /** Write `data` after what is on screen. */
  | { kind: "append"; data: string }
  | { kind: "exit" }
  /** Output was lost: fetch a fresh snapshot. */
  | { kind: "resync" };

export interface TerminalStep {
  cursor: TerminalCursor;
  outputs: TerminalOutput[];
}

export function initialTerminalCursor(): TerminalCursor {
  return { seq: 0, synced: false, pending: [], overflowed: false, alive: true };
}

/** Forget the stream position; the caller must request a snapshot. */
export function desyncTerminal(cursor: TerminalCursor): TerminalCursor {
  return { ...cursor, synced: false, pending: [], overflowed: false };
}

export function applyTerminalSnapshot(
  cursor: TerminalCursor,
  snap: TerminalSnapshot,
): TerminalStep {
  if (!snap.alive) {
    // Gone on the host: keep what is on screen, just mark it ended.
    return {
      cursor: { seq: cursor.seq, synced: true, pending: [], overflowed: false, alive: false },
      outputs: cursor.alive ? [{ kind: "exit" }] : [],
    };
  }
  if (cursor.overflowed) {
    return { cursor: desyncTerminal(cursor), outputs: [{ kind: "resync" }] };
  }
  let step: TerminalStep = {
    cursor: { seq: snap.seq, synced: true, pending: [], overflowed: false, alive: true },
    outputs: [{ kind: "reset", data: snap.data }],
  };
  for (const ev of cursor.pending) {
    const next = applyTerminalEvent(step.cursor, ev);
    step = { cursor: next.cursor, outputs: [...step.outputs, ...next.outputs] };
    if (!step.cursor.synced) break;
  }
  return step;
}

export function applyTerminalEvent(cursor: TerminalCursor, ev: TerminalEvent): TerminalStep {
  if (ev.type === "terminal.exit") {
    return {
      cursor: { ...cursor, alive: false },
      outputs: cursor.alive ? [{ kind: "exit" }] : [],
    };
  }
  if (!cursor.synced) {
    if (cursor.pending.length >= MAX_PENDING_EVENTS) {
      return { cursor: { ...cursor, pending: [], overflowed: true }, outputs: [] };
    }
    return { cursor: { ...cursor, pending: [...cursor.pending, ev] }, outputs: [] };
  }
  if (ev.seq === 0) {
    // Unnumbered chunk: cannot be deduplicated, apply as-is.
    return { cursor, outputs: [{ kind: "append", data: ev.payload.data }] };
  }
  if (ev.seq <= cursor.seq) return { cursor, outputs: [] };
  if (ev.seq > cursor.seq + 1) {
    return { cursor: desyncTerminal(cursor), outputs: [{ kind: "resync" }] };
  }
  return {
    cursor: { ...cursor, seq: ev.seq },
    outputs: [{ kind: "append", data: ev.payload.data }],
  };
}
