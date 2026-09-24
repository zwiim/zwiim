// The workspace tree — roots, their worktrees, and each workspace's sessions
// with a working-agent count — derived from the host state feed's workspaces
// and sessions and live session views. Nothing here is stored separately.

import type { SessionSummary, WorkspaceInfo } from "../types/protocol";
import type { SessionView } from "./session";
import type { HostState } from "./store";

export interface WorkspaceNode {
  workspace: WorkspaceInfo;
  /** The host's session list; null until loaded. */
  sessions: SessionSummary[] | null;
  /** Sessions whose agent is running a turn right now. */
  working: number;
  /** A root's worktrees, in host order. */
  worktrees: WorkspaceNode[];
}

/**
 * Whether a session's agent is running a turn. An open session's live
 * stream is fresher than the session summary, so it wins while attached.
 */
export function isSessionWorking(summary: SessionSummary, view?: SessionView): boolean {
  if (view?.attach === "live" && view.agentStatus !== null) return view.agentStatus === "prompting";
  return summary.live && summary.status === "in_progress";
}

export function buildWorkspaceTree(
  state: Pick<HostState, "workspaces" | "sessionsByWorkspace" | "sessions">,
): WorkspaceNode[] {
  const node = (workspace: WorkspaceInfo): WorkspaceNode => {
    const sessions = state.sessionsByWorkspace[workspace.workspace_id] ?? null;
    const working = (sessions ?? []).filter((s) =>
      isSessionWorking(s, state.sessions[s.session_id]),
    ).length;
    return { workspace, sessions, working, worktrees: [] };
  };
  const all = state.workspaces ?? [];
  const byId = new Map(all.map((w) => [w.workspace_id, node(w)]));
  const roots: WorkspaceNode[] = [];
  for (const w of all) {
    const n = byId.get(w.workspace_id)!;
    const parent = w.parent_workspace_id ? byId.get(w.parent_workspace_id) : undefined;
    // A worktree whose root is not listed is still shown, at the top.
    if (parent && parent !== n) parent.worktrees.push(n);
    else roots.push(n);
  }
  return roots;
}

let memo: { key: unknown[]; tree: WorkspaceNode[] } | null = null;

/** `buildWorkspaceTree`, returning the same array while its inputs are unchanged. */
export function selectWorkspaceTree(state: HostState): WorkspaceNode[] {
  const key = [state.workspaces, state.sessionsByWorkspace, state.sessions];
  if (memo && memo.key.every((k, i) => k === key[i])) return memo.tree;
  memo = { key, tree: buildWorkspaceTree(state) };
  return memo.tree;
}
