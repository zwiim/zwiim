// Which workspace Files and Terminal show: the one chosen on this host, while
// the host still lists it, else its first root workspace.

import type { WorkspaceInfo } from "@zwiim/sdk";

export function currentWorkspace(workspaces: readonly WorkspaceInfo[] | null, chosen: string | null): WorkspaceInfo | null {
  const open = (workspaces ?? []).filter((w) => !w.removed);
  return open.find((w) => w.workspace_id === chosen) ?? open.find((w) => w.kind === "root") ?? open[0] ?? null;
}
