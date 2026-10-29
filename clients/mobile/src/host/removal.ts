// Removing a worktree, or a whole workspace, from the phone. The Host decides
// whether it is safe:
// it refuses while agents run there (`workspace_busy`, naming the sessions
// whose agents run there; `force` never overrides it; terminals it kills) and, unless
// forced, while the checkout has uncommitted changes (`workspace_dirty`).
// Each answer becomes the next step shown; stopping the named agents asks the
// Host again only once every stop succeeded. Removing a workspace asks the same
// of its root and every worktree, and archives the worktrees; its folder on
// the host is never touched.

import { busyDetails, dirtyDetails, isClientError, type WorkspaceBusyDetails } from "@zwiim/sdk";

export interface RemovalHost {
  removeWorktree(workspace_id: string, opts: { force?: boolean }): Promise<unknown>;
  removeWorkspace(workspace_id: string, opts: { force?: boolean }): Promise<unknown>;
  stopSession(session_id: string): Promise<void>;
}

export type RemovalStep =
  | { step: "removed" }
  /** Still running there, as the Host listed it. */
  | { step: "busy"; busy: WorkspaceBusyDetails }
  /** Uncommitted changes a forced removal discards. */
  | { step: "dirty"; changes: number };

/** Ask the Host to remove the worktree; its refusals become the next step. Other errors throw. */
export async function removeWorktree(host: RemovalHost, workspace_id: string, force = false): Promise<RemovalStep> {
  return answered(() => host.removeWorktree(workspace_id, force ? { force: true } : {}));
}

/** Ask the Host to remove a workspace and archive its worktrees; refused like `removeWorktree`. */
export async function removeWorkspace(host: RemovalHost, workspace_id: string, force = false): Promise<RemovalStep> {
  return answered(() => host.removeWorkspace(workspace_id, force ? { force: true } : {}));
}

async function answered(remove: () => Promise<unknown>): Promise<RemovalStep> {
  try {
    await remove();
    return { step: "removed" };
  } catch (e) {
    const busy = busyDetails(e);
    if (busy) return { step: "busy", busy };
    const dirty = dirtyDetails(e);
    if (dirty) return { step: "dirty", changes: dirty.changes };
    throw e;
  }
}

/**
 * Stop the agents of `session_ids`, one after another, then ask the Host to
 * remove the worktree (or, with `remove`, the workspace) again. A stop that
 * fails (or whose outcome is unknown) throws before anything is removed; it is
 * never retried here.
 */
export async function stopThenRemove(
  host: RemovalHost,
  workspace_id: string,
  session_ids: readonly string[],
  force = false,
  remove = removeWorktree,
): Promise<RemovalStep> {
  for (const id of session_ids) await host.stopSession(id);
  return remove(host, workspace_id, force);
}

/** What still blocks removal besides the named sessions, in words; null when nothing does. */
export function otherBlockers(busy: WorkspaceBusyDetails): string | null {
  const parts: string[] = [];
  const starting = busy.live_sessions - busy.session_ids.length;
  if (starting > 0) parts.push(starting === 1 ? "1 agent starting" : `${starting} agents starting`);
  if (busy.terminals > 0) parts.push(busy.terminals === 1 ? "1 terminal running" : `${busy.terminals} terminals running`);
  return parts.length ? parts.join(" · ") : null;
}

/** What the Host said still runs in the worktree, as the PR sheet shows it: stop it first. */
export function archiveBusyText(busy: WorkspaceBusyDetails): string {
  const parts: string[] = [];
  if (busy.live_sessions > 0) parts.push(busy.live_sessions === 1 ? "1 agent running" : `${busy.live_sessions} agents running`);
  if (busy.terminals > 0) parts.push(busy.terminals === 1 ? "1 terminal open" : `${busy.terminals} terminals open`);
  const one = busy.live_sessions + busy.terminals === 1;
  return `${parts.join(", ") || "Work is still running"} — stop ${one ? "it" : "them"} to archive.`;
}

export function removalError(e: unknown, hostName: string): string {
  if (isClientError(e, "unknown_outcome")) return `Not sure that went through. Check ${hostName} before trying again.`;
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  if (isClientError(e, "invalid_input")) return "A to-do task is still working in this worktree. Finish or cancel it first.";
  return "Couldn't archive the worktree. Try again.";
}

export function workspaceRemovalError(e: unknown, hostName: string): string {
  if (isClientError(e, "unknown_outcome")) return `Not sure that went through. Check ${hostName} before trying again.`;
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  if (isClientError(e, "invalid_input")) return "A to-do task is still working in one of its worktrees. Finish or cancel it first.";
  return "Couldn't remove the workspace. Try again.";
}

export function stopAgentError(e: unknown, hostName: string): string {
  if (isClientError(e, "unknown_outcome")) return `Not sure the agent stopped. Check it on ${hostName} before trying again.`;
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't stop the agent. Try again.";
}
