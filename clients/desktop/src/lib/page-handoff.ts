// What a person hands from a browser page to a chat's next message (browser
// design 10.6): picked elements, held per chat until sent or removed. The
// block itself says it is page content, never an instruction.

import { useSyncExternalStore } from "react";

/** One handed-over thing: `label` for its chip, `text` for the agent. */
export interface PageBlock {
  id: string;
  label: string;
  text: string;
}

const EMPTY: readonly PageBlock[] = [];
/** Most blocks one message carries; the oldest goes first. */
const MAX_BLOCKS = 10;
const held = new Map<string, readonly PageBlock[]>();
const listeners = new Set<() => void>();

function set(sessionId: string, blocks: readonly PageBlock[]) {
  if (blocks.length) held.set(sessionId, blocks);
  else held.delete(sessionId);
  for (const l of [...listeners]) l();
}

/** Add `block` to chat `sessionId`'s next message. */
export function handToChat(sessionId: string, block: Omit<PageBlock, "id">): void {
  const blocks = [...(held.get(sessionId) ?? EMPTY), { ...block, id: crypto.randomUUID() }];
  set(sessionId, blocks.slice(-MAX_BLOCKS));
}

/** Take block `id` back out of chat `sessionId`'s next message. */
export function removeFromChat(sessionId: string, id: string): void {
  set(sessionId, (held.get(sessionId) ?? EMPTY).filter((b) => b.id !== id));
}

/** Chat `sessionId`'s message was sent with `sent`: they go. */
export function sentToChat(sessionId: string, sent: readonly PageBlock[]): void {
  const ids = new Set(sent.map((b) => b.id));
  set(sessionId, (held.get(sessionId) ?? EMPTY).filter((b) => !ids.has(b.id)));
}

/** What chat `sessionId`'s next message carries from pages; none without a chat. */
export function useHandedBlocks(sessionId: string | null | undefined): readonly PageBlock[] {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => (sessionId ? (held.get(sessionId) ?? EMPTY) : EMPTY),
  );
}
