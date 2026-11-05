// The only home of the rules for moving things in the workspace layout: what
// may go where, and what the result is. Desktop drag and every menu use these.
// The Host only reconciles what it is sent.

import type { LayoutEntry, LayoutWorkspace, WorkspaceLayout } from "../types/protocol";

/** Something in the layout. Worktrees are listed under their repository. */
export interface LayoutRef {
  kind: "group" | "workspace" | "worktree";
  id: string;
  /** A worktree whose repository is gone: it stands alone in the list, but no group can hold it. */
  standalone?: boolean;
}

/** Where to put something. `end` is the end of the top level. */
export type LayoutTarget =
  | { at: "before" | "after"; ref: LayoutRef }
  | { at: "into"; group_id: string }
  | { at: "end" };

interface Holder {
  items: LayoutRef[];
  /** The group holding the items, when one does. */
  group_id: string | null;
}

const same = (a: LayoutRef, b: LayoutRef) => a.kind === b.kind && a.id === b.id;

const workspaceRef = (w: LayoutWorkspace): LayoutRef => ({ kind: "workspace", id: w.workspace_id });

const entryRef = (e: LayoutEntry): LayoutRef =>
  e.kind === "group" ? { kind: "group", id: e.group_id } : workspaceRef(e);

function worktreesHolder(w: LayoutWorkspace, group_id: string | null): Holder {
  return { items: w.worktrees.map((id) => ({ kind: "worktree", id })), group_id };
}

/** What `ref` sits among, or null when it is not in the layout. */
function holderOf(layout: WorkspaceLayout, ref: LayoutRef): Holder | null {
  const top = layout.entries.map(entryRef);
  if (top.some((r) => same(r, ref))) return { items: top, group_id: null };
  for (const entry of layout.entries) {
    if (entry.kind === "workspace") {
      if (ref.kind === "worktree" && entry.worktrees.includes(ref.id)) {
        return worktreesHolder(entry, null);
      }
      continue;
    }
    const members = entry.workspaces.map(workspaceRef);
    if (members.some((r) => same(r, ref))) return { items: members, group_id: entry.group_id };
    const root = entry.workspaces.find((w) => ref.kind === "worktree" && w.worktrees.includes(ref.id));
    if (root) return worktreesHolder(root, entry.group_id);
  }
  return null;
}

function allowed(layout: WorkspaceLayout, source: LayoutRef, target: LayoutTarget): boolean {
  const from = holderOf(layout, source);
  if (!from) return false;
  if (target.at === "end") return source.kind !== "worktree";
  if (target.at === "into") {
    if (source.standalone) return false;
    return source.kind === "workspace" && layout.entries.some((e) => e.kind === "group" && e.group_id === target.group_id);
  }
  const { ref } = target;
  const to = holderOf(layout, ref);
  if (!to || same(ref, source)) return false;
  switch (source.kind) {
    case "group":
      return ref.kind !== "worktree" && to.group_id === null;
    case "workspace":
      return ref.kind !== "worktree" && (!source.standalone || to.group_id === null);
    case "worktree":
      // Only inside its repository.
      return ref.kind === "worktree" && to.items.some((r) => same(r, source));
  }
}

function insert<T>(list: T[], item: T, at: "before" | "after", index: number) {
  list.splice(at === "before" ? index : index + 1, 0, item);
}

/**
 * `layout` with `source` moved to `target`, or null when that move is not
 * allowed, names something no longer there, or would change nothing. A null
 * is never sent to the Host.
 */
export function moveInLayout(
  layout: WorkspaceLayout,
  source: LayoutRef,
  target: LayoutTarget,
): WorkspaceLayout | null {
  if (!allowed(layout, source, target)) return null;
  const copy = (w: LayoutWorkspace): LayoutWorkspace => ({
    workspace_id: w.workspace_id,
    worktrees: [...w.worktrees],
  });
  const entries: LayoutEntry[] = layout.entries.map((e) =>
    e.kind === "group"
      ? { ...e, workspaces: e.workspaces.map(copy) }
      : { kind: "workspace", ...copy(e) },
  );
  const memberLists = entries.flatMap((e) => (e.kind === "group" ? [e.workspaces] : []));
  const roots = entries.flatMap((e) => (e.kind === "group" ? e.workspaces : [e]));

  if (source.kind === "worktree") {
    const root = roots.find((w) => w.worktrees.includes(source.id))!;
    root.worktrees.splice(root.worktrees.indexOf(source.id), 1);
    if (target.at === "before" || target.at === "after") {
      insert(root.worktrees, source.id, target.at, root.worktrees.indexOf(target.ref.id));
    }
    return changed(layout, entries);
  }

  const topIndex = (ref: LayoutRef) => entries.findIndex((e) => same(entryRef(e), ref));
  if (source.kind === "group") {
    const [group] = entries.splice(topIndex(source), 1);
    place(entries, group!, target, topIndex);
    return changed(layout, entries);
  }

  // A repository: out of wherever it is, then into a group or the top level.
  let moved: LayoutWorkspace;
  const at = topIndex(source);
  if (at >= 0) {
    moved = copy(entries.splice(at, 1)[0] as LayoutWorkspace);
  } else {
    const list = memberLists.find((l) => l.some((w) => w.workspace_id === source.id))!;
    moved = list.splice(list.findIndex((w) => w.workspace_id === source.id), 1)[0]!;
  }
  if (target.at === "into") {
    const group = entries.find((e) => e.kind === "group" && e.group_id === target.group_id);
    if (group?.kind === "group") group.workspaces.push(moved);
    return changed(layout, entries);
  }
  if (target.at !== "end") {
    const ref = target.ref;
    const list = memberLists.find((l) => l.some((w) => same(workspaceRef(w), ref)));
    if (list) {
      insert(list, moved, target.at, list.findIndex((w) => same(workspaceRef(w), ref)));
      return changed(layout, entries);
    }
  }
  place(entries, { kind: "workspace", ...moved }, target, topIndex);
  return changed(layout, entries);
}

function place(
  entries: LayoutEntry[],
  entry: LayoutEntry,
  target: LayoutTarget,
  topIndex: (ref: LayoutRef) => number,
) {
  if (target.at === "before" || target.at === "after") {
    insert(entries, entry, target.at, topIndex(target.ref));
  } else {
    entries.push(entry);
  }
}

function changed(layout: WorkspaceLayout, entries: LayoutEntry[]): WorkspaceLayout | null {
  return JSON.stringify(entries) === JSON.stringify(layout.entries) ? null : { entries };
}

/**
 * Where Move up (-1) or Move down (1) takes `source`: past the neighbor in its
 * container, a whole group at a time. Null at the ends.
 */
export function stepTarget(layout: WorkspaceLayout, source: LayoutRef, by: -1 | 1): LayoutTarget | null {
  const holder = holderOf(layout, source);
  if (!holder) return null;
  const neighbor = holder.items[holder.items.findIndex((r) => same(r, source)) + by];
  return neighbor ? { at: by < 0 ? "before" : "after", ref: neighbor } : null;
}

/** The group holding `ref`, when one does. */
export function groupOf(layout: WorkspaceLayout, ref: LayoutRef): string | null {
  return holderOf(layout, ref)?.group_id ?? null;
}

/** Where `source` sits now, as a target that puts it back there later. */
export function placeOf(layout: WorkspaceLayout, source: LayoutRef): LayoutTarget | null {
  const holder = holderOf(layout, source);
  if (!holder) return null;
  const index = holder.items.findIndex((r) => same(r, source));
  const next = holder.items[index + 1];
  if (next) return { at: "before", ref: next };
  if (source.kind === "worktree") {
    const previous = holder.items[index - 1];
    return previous ? { at: "after", ref: previous } : null;
  }
  return holder.group_id ? { at: "into", group_id: holder.group_id } : { at: "end" };
}
