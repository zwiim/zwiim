import type { PendingFile } from "@/attachments/pending";

/** What was left in a composer: the message and the files chosen for it. */
export interface ComposerDraft {
  text: string;
  files: PendingFile[];
}

export const EMPTY_DRAFT: ComposerDraft = { text: "", files: [] };

/** What was in a session's composer when it forked, for the fork's composer. */
const drafts = new Map<string, ComposerDraft>();

export function keepForkedDraft(sessionId: string, draft: ComposerDraft): void {
  if (draft.text || draft.files.length > 0) drafts.set(sessionId, draft);
}

/** The draft waiting for session `sessionId`; it stays until taken back with `dropForkedDraft`. */
export function forkedDraft(sessionId: string): ComposerDraft {
  return drafts.get(sessionId) ?? EMPTY_DRAFT;
}

export function dropForkedDraft(sessionId: string): void {
  drafts.delete(sessionId);
}
