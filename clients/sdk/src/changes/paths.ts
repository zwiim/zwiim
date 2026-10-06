// Where an agent's file path lands inside its workspace.

import type { WorkspaceInfo } from "../types/protocol";

/**
 * The workspace file a link points at, relative to the workspace, or null.
 * Agents link files relative to the workspace or by their absolute path; the
 * host never shares its paths, so an absolute one resolves only past the
 * workspace's own folder (a root's home-relative path when known, else its
 * folder name). A line suffix (`:12`, `#L12`) is dropped.
 */
export function workspaceFile(
  href: string | null | undefined,
  workspace: Pick<WorkspaceInfo, "display_path" | "name">,
): string | null {
  if (!href || (/^[a-z][a-z0-9+.-]*:/i.test(href) && !/^file:/i.test(href))) return null;
  let path: string;
  try {
    path = decodeURIComponent(href.replace(/^file:\/\//i, ""));
  } catch {
    return null;
  }
  path = path.replace(/#.*$/, "").replace(/(:\d+)+$/, "");
  const segments = path.split("/").filter((s) => s !== "" && s !== ".");
  if (segments.includes("..")) return null;
  let rest = segments;
  if (path.startsWith("/") || path.startsWith("~")) {
    const home = workspace.display_path?.split("/").slice(1) ?? [];
    const anchor = home.length > 0 ? home : [workspace.name];
    const at = segments.findIndex((_, i) => anchor.every((a, j) => segments[i + j] === a));
    if (at < 0) return null;
    rest = segments.slice(at + anchor.length);
  }
  return rest.length > 0 ? rest.join("/") : null;
}
