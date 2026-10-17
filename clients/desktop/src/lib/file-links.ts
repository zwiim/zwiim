import { createContext } from "react";
import type { WorkspaceInfo } from "@zwiim/sdk";

/** Where a conversation's file links open: its workspace's files, beside it. */
export interface FileLinks {
  workspace: WorkspaceInfo;
  open(path: string): void;
  /** Opens a file with its uncommitted changes marked. */
  openDiff(path: string): void;
}

// Its own module, so the chips that read it and Markdown don't import each
// other: a hot update to one would otherwise make a second context.
export const FileLinksContext = createContext<FileLinks | null>(null);
