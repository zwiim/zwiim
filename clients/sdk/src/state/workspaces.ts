// The workspace tree — groups, roots, their worktrees, and each workspace's
// sessions with a working-agent count — derived from the host state feed's
// workspaces, layout and sessions and live session views. Nothing here is
// stored separately.

import type {
  LayoutWorkspace,
  SessionSummary,
  WorkspaceGroup,
  WorkspaceInfo,
} from "../types/protocol";
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

/** What the sidebar lists, in Host order. */
export type SidebarEntry =
  | { kind: "group"; group: WorkspaceGroup; repos: WorkspaceNode[]; working: number }
  | { kind: "repo"; node: WorkspaceNode };

type TreeState = Pick<HostState, "workspaces" | "sessionsByWorkspace" | "sessions"> &
  Partial<Pick<HostState, "layout">>;

function listedRoots(state: TreeState): WorkspaceNode[] {
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

/** `node` with its worktrees in `order`; ones `order` does not name go last. */
function ordered(node: WorkspaceNode, order: string[]): WorkspaceNode {
  const at = new Map(order.map((id, i) => [id, i]));
  const rank = (n: WorkspaceNode) => at.get(n.workspace.workspace_id) ?? order.length;
  return { ...node, worktrees: [...node.worktrees].sort((a, b) => rank(a) - rank(b)) };
}

const rollup = (repos: WorkspaceNode[]) =>
  repos.reduce((n, r) => n + r.working + r.worktrees.reduce((m, w) => m + w.working, 0), 0);

/**
 * The tree in the Host's layout. With no layout the list stays flat. A listed
 * row the layout does not name yet goes last in its place, so a
 * `workspace.changed` that arrives ahead of the layout hides nothing.
 */
export function buildSidebarTree(state: TreeState): SidebarEntry[] {
  const roots = listedRoots(state);
  const layout = state.layout;
  if (!layout) return roots.map((node) => ({ kind: "repo", node }));
  const unplaced = new Map(roots.map((n) => [n.workspace.workspace_id, n]));
  const take = (w: LayoutWorkspace): WorkspaceNode[] => {
    const node = unplaced.get(w.workspace_id);
    if (!node) return [];
    unplaced.delete(w.workspace_id);
    return [ordered(node, w.worktrees)];
  };
  const entries = layout.entries.flatMap((e): SidebarEntry[] => {
    if (e.kind === "workspace") return take(e).map((node) => ({ kind: "repo", node }));
    const repos = e.workspaces.flatMap(take);
    const group = { group_id: e.group_id, name: e.name };
    return [{ kind: "group", group, repos, working: rollup(repos) }];
  });
  for (const node of unplaced.values()) entries.push({ kind: "repo", node });
  return entries;
}

/** Every repository in sidebar order, without its groups. */
export function buildWorkspaceTree(state: TreeState): WorkspaceNode[] {
  return buildSidebarTree(state).flatMap((e) => (e.kind === "group" ? e.repos : [e.node]));
}

/**
 * Where clicking root `rootId` opens: its live worktree whose newest session
 * was prompted last, else its last live worktree in host order, else null
 * (create one). Never the root itself.
 */
export function openTarget(
  state: Pick<HostState, "workspaces" | "sessionsByWorkspace">,
  rootId: string,
): WorkspaceInfo | null {
  const worktrees = (state.workspaces ?? []).filter(
    (w) => w.parent_workspace_id === rootId && !w.removed,
  );
  let best: WorkspaceInfo | null = null;
  let bestAt = "";
  for (const w of worktrees) {
    for (const s of state.sessionsByWorkspace[w.workspace_id] ?? []) {
      if (s.updated_at > bestAt) {
        best = w;
        bestAt = s.updated_at;
      }
    }
  }
  return best ?? worktrees.at(-1) ?? null;
}

let memo: { key: unknown[]; entries: SidebarEntry[]; tree: WorkspaceNode[] } | null = null;

function selectTrees(state: HostState) {
  const key = [state.workspaces, state.layout, state.sessionsByWorkspace, state.sessions];
  if (memo && memo.key.every((k, i) => k === key[i])) return memo;
  const entries = buildSidebarTree(state);
  const tree = entries.flatMap((e) => (e.kind === "group" ? e.repos : [e.node]));
  memo = { key, entries, tree };
  return memo;
}

/** `buildSidebarTree`, returning the same array while its inputs are unchanged. */
export function selectSidebarTree(state: HostState): SidebarEntry[] {
  return selectTrees(state).entries;
}

/** `buildWorkspaceTree`, returning the same array while its inputs are unchanged. */
export function selectWorkspaceTree(state: HostState): WorkspaceNode[] {
  return selectTrees(state).tree;
}
