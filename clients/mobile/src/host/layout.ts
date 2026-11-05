// The workspace layout as the phone shows it (DESIGN.md, Workspace context):
// groups at level one, the picker in New session, and where a row sits. What
// may move where is the SDK's; the Host owns the layout, the phone only asks.

import { isClientError, type LayoutRef, type SidebarEntry, type WorkspaceGroup, type WorkspaceLayout, type WorkspaceNode } from "@zwiim/sdk";
import { workspaceRoots } from "./workspace";

/** A group the Host just made, added to a layout that may not list it yet. */
export function withGroup(layout: WorkspaceLayout, group: WorkspaceGroup | undefined): WorkspaceLayout {
  if (!group || layout.entries.some((e) => e.kind === "group" && e.group_id === group.group_id)) return layout;
  return { entries: [...layout.entries, { kind: "group", ...group, workspaces: [] }] };
}

/** The group of that name, if nothing is in it. Names match as the Host matches them. */
export function emptyGroup(layout: WorkspaceLayout, name: string): WorkspaceGroup | undefined {
  const wanted = name.toLowerCase();
  for (const e of layout.entries) {
    if (e.kind === "group" && e.name.toLowerCase() === wanted && e.workspaces.length === 0) {
      return { group_id: e.group_id, name: e.name };
    }
  }
  return undefined;
}

function liveEntries(entries: readonly SidebarEntry[]): SidebarEntry[] {
  return entries.flatMap((e): SidebarEntry[] => {
    if (e.kind === "group") return [{ ...e, repos: workspaceRoots(e.repos, "") }];
    return workspaceRoots([e.node], "").length ? [e] : [];
  });
}

/** Level one: the groups and their members, or a flat list while searching. */
export function workspaceSections(entries: readonly SidebarEntry[], query: string): SidebarEntry[] {
  const live = liveEntries(entries);
  if (!query.trim()) return live;
  const flat = live.flatMap((e) => (e.kind === "group" ? e.repos : [e.node]));
  return workspaceRoots(flat, query).map((node) => ({ kind: "repo", node }));
}

/** New session's picker, in the same order: a group with nothing to pick here is left out. */
export function pickerSections(entries: readonly SidebarEntry[], listed: (node: WorkspaceNode) => boolean): SidebarEntry[] {
  return liveEntries(entries).flatMap((e): SidebarEntry[] => {
    if (e.kind === "repo") return listed(e.node) ? [e] : [];
    const repos = e.repos.filter(listed);
    return repos.length ? [{ ...e, repos }] : [];
  });
}

/** Where a row sits: its group's name (none at the top level) and its place among its neighbors. */
export interface LayoutPlace {
  group: string | null;
  index: number;
  count: number;
}

export function layoutPlace(layout: WorkspaceLayout, ref: LayoutRef): LayoutPlace | null {
  const top = layout.entries.findIndex((e) =>
    ref.kind === "group" ? e.kind === "group" && e.group_id === ref.id : e.kind === "workspace" && e.workspace_id === ref.id,
  );
  if (top >= 0) return { group: null, index: top, count: layout.entries.length };
  for (const e of layout.entries) {
    if (e.kind !== "group") continue;
    const index = e.workspaces.findIndex((w) => w.workspace_id === ref.id);
    if (index >= 0) return { group: e.name, index, count: e.workspaces.length };
  }
  return null;
}

/** A place in words: "backend · 2 of 3". */
export function layoutPlaceText(place: LayoutPlace): string {
  return `${place.group ?? "Top level"} · ${place.index + 1} of ${place.count}`;
}

/** What deleting a group does, as Desktop says it. */
export function deleteGroupText(members: number): string {
  const moved = members === 0 ? "" : members === 1 ? "Its repository moves to the top level, in place. " : `Its ${members} repositories move to the top level, in place. `;
  return `${moved}Nothing on disk changes.`;
}

/** A refused or lost layout write, in words. */
export function layoutError(e: unknown, hostName: string): string {
  if (isClientError(e, "unknown_outcome")) return `Not sure that went through. Check ${hostName} before trying again.`;
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  if (isClientError(e, "invalid_input")) return e.message;
  return "Couldn't make that change. Try again.";
}
