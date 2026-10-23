// Resetting a session to before one of its messages (`session.revert`). The
// Host restores the files the message started with and, where it and the
// agent can, takes the chat back to the reply before it. The shared rule and words
// are the SDK's; these are the Session screen's own.

import { isClientError, revertRefusalText, type WorkspaceInfo } from "@zwiim/sdk";

/** A removed worktree or the repository folder has nothing to reset. */
export function canReset(workspace: WorkspaceInfo | null | undefined): boolean {
  return !!workspace && !workspace.removed && workspace.kind !== "root";
}

/**
 * One reset at a time: a tap while one is still pending is dropped (resolves
 * to `undefined`) rather than sent twice, even before the screen re-renders.
 */
export function oneReset<T>(): (run: () => Promise<T>) => Promise<T | undefined> {
  let pending = false;
  return async (run) => {
    if (pending) return undefined;
    pending = true;
    try {
      return await run();
    } finally {
      pending = false;
    }
  };
}

export interface RevertCopy {
  title: string;
  body: string;
  /** The files checkbox's label; null on a files-only reset, which has none. */
  filesOption: string | null;
  confirm: string;
}

/** The sheet's words. A files-only reset says nothing about messages going. */
export function revertCopy(chat: boolean): RevertCopy {
  return chat
    ? { title: "Reset chat to here?", body: "Later messages will be removed.", filesOption: "Also reset files", confirm: "Reset" }
    : {
        title: "Reset files to here?",
        body: "Files go back to how they were before this message. The chat stays as it is.",
        filesOption: null,
        confirm: "Reset files",
      };
}

/** Why a reset did not happen, in words. */
export function revertError(e: unknown, hostName: string): string {
  const refused = revertRefusalText(e);
  if (refused) return refused;
  if (isClientError(e, "unknown_outcome")) return `Not sure the reset went through. Check the session on ${hostName} before trying again.`;
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't reset. Try again.";
}
