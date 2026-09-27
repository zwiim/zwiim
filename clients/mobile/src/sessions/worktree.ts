// The sessions of one worktree (DESIGN.md, Screen 5, session strip): a
// worktree is where work runs, a session one task in it, and a worktree may
// have many. The strip over a session shows its worktree's sessions in the
// order they were started (Desktop's tabs), switches between them, and `+`
// starts another there. A session belongs to the worktree it started in for
// good; choosing another worktree shows that one's sessions and never moves
// one. Files keeps its own selection throughout.

import type { HostState } from "@zwiim/sdk";
import { contextLabel } from "../host/workspace";
import { toItem, workspaceNames, type SessionItem } from "./model";

export interface WorktreeSessions {
  workspace_id: string;
  /** "⎇ fix-auth", or a plain folder's name. */
  label: string;
  /** Removed: its sessions stay readable, and nothing new starts there. */
  removed: boolean;
  /** Oldest first, as they were started. */
  sessions: SessionItem[];
}

/** A worktree's sessions; null when the host doesn't list the worktree. */
export function worktreeSessions(
  state: Pick<HostState, "workspaces" | "sessionsByWorkspace" | "sessions">,
  workspace_id: string,
  seenAt: (request_id: string) => number | null = () => null,
): WorktreeSessions | null {
  const w = state.workspaces?.find((x) => x.workspace_id === workspace_id);
  if (!w) return null;
  const names = workspaceNames(state.workspaces);
  const sessions = (state.sessionsByWorkspace[workspace_id] ?? [])
    .map((s) => toItem(s, state.sessions[s.session_id], names, seenAt))
    .sort((a, b) => a.createdAt - b.createdAt || a.session_id.localeCompare(b.session_id));
  const label = contextLabel(state.workspaces, workspace_id)!;
  return {
    workspace_id,
    label: label.worktree === null ? label.workspace : `⎇ ${label.worktree}`,
    removed: w.removed,
    sessions,
  };
}

/**
 * The session to show on switching to a worktree: the one last on screen
 * there while the host still lists it, else the newest; null when it has none.
 */
export function sessionToOpen(sessions: readonly Pick<SessionItem, "session_id">[], remembered: string | null): string | null {
  if (remembered && sessions.some((s) => s.session_id === remembered)) return remembered;
  return sessions.at(-1)?.session_id ?? null;
}
