// The Host's hierarchy as the phone shows it (DESIGN.md, Workspace context):
// a workspace (a root) and, inside it, the worktree Files works in. The root
// itself is never a target: the Host runs sessions and terminals in worktrees. Both levels come from the SDK's
// workspace tree; the selection is the leaf, the level above is derived.

import {
  isClientError,
  openTarget,
  type HostState,
  type TerminalMeta,
  type WorkspaceInfo,
  type WorkspaceNode,
} from "@zwiim/sdk";

export function workspaceLabel(w: WorkspaceInfo): string {
  return w.alias ?? w.name;
}

/**
 * The worktree Files shows, never a root (the Host runs nothing there): the
 * chosen worktree while it is live; for a chosen root, the worktree it opens
 * on (Desktop's `openTarget`), else none; for a removed worktree, its root's.
 * Only with nothing live chosen: the first root's that has one, else any live
 * worktree. Null when there is none to show.
 */
export function currentWorkspace(
  workspaces: readonly WorkspaceInfo[] | null,
  chosen: string | null,
  sessionsByWorkspace: HostState["sessionsByWorkspace"] = {},
): WorkspaceInfo | null {
  const all = workspaces ?? [];
  const open = all.filter((w) => !w.removed);
  const state = { workspaces: open, sessionsByWorkspace };
  const picked = all.find((w) => w.workspace_id === chosen);
  const root = picked && (picked.removed ? open.find((w) => w.workspace_id === picked.parent_workspace_id) : picked);
  if (root) return root.kind === "worktree" ? root : openTarget(state, root.workspace_id);
  for (const r of open) {
    const first = r.kind === "root" ? openTarget(state, r.workspace_id) : null;
    if (first) return first;
  }
  return open.find((w) => w.kind === "worktree") ?? null;
}

/**
 * Where something lives, in words: its workspace and its worktree (a branch,
 * or "root"). A plain folder (not a Git repository) has no worktree.
 */
export interface ContextLabel {
  workspace: string;
  worktree: string | null;
}

export function contextLabel(workspaces: readonly WorkspaceInfo[] | null, id: string | null): ContextLabel | null {
  const list = workspaces ?? [];
  const w = list.find((x) => x.workspace_id === id);
  if (!w) return null;
  const root = w.parent_workspace_id ? list.find((x) => x.workspace_id === w.parent_workspace_id) : undefined;
  if (w.kind === "root" || !w.parent_workspace_id) {
    return { workspace: workspaceLabel(w), worktree: w.is_git_repo ? (w.branch ?? "root") : null };
  }
  return { workspace: root ? workspaceLabel(root) : workspaceLabel(w), worktree: w.branch ?? workspaceLabel(w) };
}

/** "zwiim · fix-auth", or "notes" for a plain folder. */
export function contextText(label: ContextLabel | null): string | null {
  if (!label) return null;
  return label.worktree === null ? label.workspace : `${label.workspace} · ${label.worktree}`;
}

/** The workspace a leaf belongs to: its root, or itself. */
export function rootIdOf(w: WorkspaceInfo): string {
  return w.parent_workspace_id ?? w.workspace_id;
}

/** Level one: the host's live roots, matching `query` by name or `~/…` path. */
export function workspaceRoots(tree: readonly WorkspaceNode[], query: string): WorkspaceNode[] {
  const q = query.trim().toLowerCase();
  return tree.filter((n) => {
    const w = n.workspace;
    if (w.kind !== "root" || w.removed) return false;
    if (!q) return true;
    return [w.name, w.alias, w.display_path].some((s) => s?.toLowerCase().includes(q));
  });
}

/** A workspace with worktrees to choose between. One with none opens straight into a new worktree, as Desktop's click does. */
export function hasWorktreeLevel(node: WorkspaceNode): boolean {
  return node.worktrees.some((n) => !n.workspace.removed);
}

/** Level two: a root's live worktrees, never the root itself (the Host runs nothing there). */
export function worktreeChoices(tree: readonly WorkspaceNode[], root_id: string): WorkspaceNode[] {
  const root = tree.find((n) => n.workspace.workspace_id === root_id);
  return root ? root.worktrees.filter((n) => !n.workspace.removed) : [];
}

/** What a workspace row says after its name: "no Git", "2 worktrees", "1 working". */
export function workspaceCounts(node: WorkspaceNode): string[] {
  const w = node.workspace;
  const worktrees = node.worktrees.filter((n) => !n.workspace.removed).length;
  const working = node.working + node.worktrees.reduce((n, t) => n + t.working, 0);
  return [
    w.is_git_repo ? null : "no Git",
    worktrees ? `${worktrees} ${worktrees === 1 ? "worktree" : "worktrees"}` : null,
    working ? `${working} working` : null,
  ].filter((s): s is string => s !== null);
}

/** Where a worktree branched from: "main@1a2b3c4", or null for a root. */
export function worktreeBase(w: WorkspaceInfo): string | null {
  if (w.base_branch && w.base_commit) return `${w.base_branch}@${w.base_commit.slice(0, 7)}`;
  return w.base_branch ?? w.base_commit?.slice(0, 7) ?? null;
}

/** The workspace tile open when the tree opens: the one holding `selected` while it's live, else the first. */
export function openTile(tree: readonly WorkspaceNode[], selected: string | null): string | null {
  const roots = workspaceRoots(tree, "");
  const holds = roots.find(
    (n) =>
      n.workspace.workspace_id === selected ||
      worktreeChoices(tree, n.workspace.workspace_id).some((c) => c.workspace.workspace_id === selected),
  );
  return (holds ?? roots[0])?.workspace.workspace_id ?? null;
}

/** A worktree's terminals; the others stay running on the host, listed again on switching back. */
export function terminalsIn(terminals: Readonly<Record<string, TerminalMeta>>, workspace_id: string): TerminalMeta[] {
  return Object.values(terminals).filter((t) => t.workspace_id === workspace_id);
}

/** What the Terminals dock says of a worktree's terminals: "dev running · tests exited", or "none in fix-auth". */
export function terminalSummary(list: readonly TerminalMeta[], scope: string): string {
  if (!list.length) return `none in ${scope}`;
  return list.map((t) => `${t.title ?? "terminal"} ${t.alive ? "running" : "exited"}`).join(" · ");
}

/** Why New worktree didn't work, in words. */
export function worktreeError(e: unknown, root: WorkspaceInfo, hostName: string): string {
  if (isClientError(e, "not_a_git_repository")) return `${workspaceLabel(root)} isn't a Git repository, so it can't have worktrees.`;
  // From a root with no base given, the host's only other refusal: nothing committed to branch from.
  if (isClientError(e, "invalid_input")) return `${workspaceLabel(root)} has no commits yet. Commit once on ${hostName}, then try again.`;
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  if (isClientError(e, "unknown_outcome")) return "Not sure that went through. Refreshed from your Mac.";
  return "Couldn't create a worktree. Try again.";
}

/** Why a folder was added but Files has no worktree of it: the workspace stays, so say so first. */
export function addedWithoutWorktree(e: unknown, root: WorkspaceInfo, hostName: string): string {
  const said = worktreeError(e, root, hostName);
  if (said === "Couldn't create a worktree. Try again.") {
    return `Added ${workspaceLabel(root)}, but couldn't create a worktree in it. Try New worktree in Files.`;
  }
  return `Added ${workspaceLabel(root)}. ${said}`;
}
