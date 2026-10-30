// A workspace's merge conflicts as the clients show them: one line, the files,
// and whether a tap can hand them to an agent. The Host checks the branch
// against its base and says which session's turn is resolving them; nothing
// here asks git.

import { hasCapability } from "../client/compat";
import { isClientError } from "../client/errors";
import type { ConflictCheck } from "../types/protocol";
import type { HostState } from "./store";
import { isSessionWorking } from "./workspaces";

/** What a tap on the conflict button does. */
export type ResolveAction =
  | { state: "ready"; session_id: string }
  | { state: "resolving"; session_id: string }
  | { state: "blocked"; reason: string };

export interface ConflictView {
  /**
   * `3 files conflict with main`, or `Conflicts with main on GitHub` when only
   * GitHub sees them; null once cleared while a turn still resolves.
   */
  line: string | null;
  files: string[];
  /** Null: nothing to resolve here (a root, an older Host, nothing unresolved). */
  resolve: ResolveAction | null;
}

const BUSY = "An agent is working here. Wait for it or stop it, then resolve conflicts.";
const NO_SESSION = "Start a chat here first, then resolve conflicts.";

const count = (n: number) => `${n} ${n === 1 ? "file" : "files"}`;

function conflictLine(check: Exclude<ConflictCheck, { state: "clean" }>): string {
  const n = check.files.length;
  if (check.state === "conflicts") {
    return `${count(n)} ${n === 1 ? "conflicts" : "conflict"} with ${check.base.replace(/^origin\//, "")}`;
  }
  const op = check.operation === "merge" ? "Merge" : check.operation === "rebase" ? "Rebase" : null;
  if (n === 0) return op ? `${op} in progress` : "Nothing unresolved";
  return op ? `${op} in progress: ${count(n)} unresolved` : `${count(n)} unresolved`;
}

/**
 * The base GitHub says the open pull request conflicts with, when the branch
 * merges cleanly here: a resolution that was never pushed. The Host's resolve
 * turn then brings the base in and pushes.
 */
function githubConflictBase(state: HostState, workspace_id: string): string | null {
  if (!hasCapability(state.connection.compat, "workspace.resolve_conflicts.github")) return null;
  const pr = state.prs[workspace_id]?.pr;
  if (!pr || (pr.state !== "open" && pr.state !== "draft") || pr.merge.status !== "dirty") return null;
  return pr.base;
}

/**
 * The conflicts of `workspace_id`, or null when there are none to show.
 * `shown`: the session on screen, resolved in when it is in this workspace;
 * otherwise the newest one is.
 */
export function conflictView(state: HostState, workspace_id: string, shown?: string | null): ConflictView | null {
  const compat = state.connection.compat;
  if (!hasCapability(compat, "workspace.conflicts")) return null;
  const check = state.conflicts[workspace_id];
  const resolving = state.resolving[workspace_id];
  const open = check && check.state !== "clean" ? check : null;
  const github = open ? null : githubConflictBase(state, workspace_id);
  if (!open && !github && !resolving) return null;
  const line = open ? conflictLine(open) : github ? `Conflicts with ${github} on GitHub` : null;
  const view: ConflictView = { line, files: open?.files ?? [], resolve: null };
  const workspace = state.workspaces?.find((w) => w.workspace_id === workspace_id);
  if (workspace?.kind !== "worktree" || !hasCapability(compat, "workspace.resolve_conflicts")) return view;
  if (resolving) return { ...view, resolve: { state: "resolving", session_id: resolving } };
  if (view.files.length === 0 && !github) return view;
  const sessions = state.sessionsByWorkspace[workspace_id] ?? [];
  if (sessions.some((s) => isSessionWorking(s, state.sessions[s.session_id]))) {
    return { ...view, resolve: { state: "blocked", reason: BUSY } };
  }
  const target = sessions.find((s) => s.session_id === shown) ?? sessions[0];
  if (!target) return { ...view, resolve: { state: "blocked", reason: NO_SESSION } };
  return { ...view, resolve: { state: "ready", session_id: target.session_id } };
}

/** The conflict button: `Merge conflicts` where Merge was, else `Resolve conflicts`. */
export function resolveLabel(action: ResolveAction, hasPr: boolean): string {
  if (action.state === "resolving") return "Resolving conflicts…";
  return hasPr ? "Merge conflicts" : "Resolve conflicts";
}

/** Why the Host refused to resolve, in words; null for codes the caller words itself. */
export function resolveRefusalText(e: unknown): string | null {
  if (isClientError(e, "turn_in_progress") || isClientError(e, "worktree_busy")) return BUSY;
  if (isClientError(e, "no_conflicts")) return "Nothing to resolve: the branch merges cleanly now.";
  if (isClientError(e, "root_workspace")) return "Conflicts are resolved in a worktree, not the main checkout.";
  if (isClientError(e, "not_found")) return "That chat is gone. Open one here, then resolve conflicts.";
  return null;
}
