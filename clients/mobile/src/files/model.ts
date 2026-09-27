// What the Files tab shows (DESIGN.md, Screen 6), from the host's answers:
// a folder's entries and the path above it, the workspace's changes, a
// file's patch as numbered lines, and a file's text. Pure; screens render it.

import { isClientError, type DiffFile, type FileTreeNode, type GitStatusEntry, type WorkspaceDiff, type WorkspaceInfo } from "@zwiim/sdk";

export interface Crumb {
  label: string;
  /** Relative to the workspace; `""` is the workspace itself. */
  path: string;
}

/** The workspace, then each folder down to `path`. */
export function crumbs(workspace: string, path: string): Crumb[] {
  const parts = path.split("/").filter(Boolean);
  return [{ label: workspace, path: "" }, ...parts.map((label, i) => ({ label, path: parts.slice(0, i + 1).join("/") }))];
}

export const parentPath = (path: string) => path.split("/").filter(Boolean).slice(0, -1).join("/");
export const baseName = (path: string) => path.split("/").filter(Boolean).at(-1) ?? path;

/** One folder's entries: folders first, then files, each in the host's order. */
export function folderEntries(nodes: readonly FileTreeNode[]): FileTreeNode[] {
  return [...nodes.filter((n) => n.kind === "dir"), ...nodes.filter((n) => n.kind === "file")];
}

export interface ChangeItem {
  path: string;
  /** Git's letter: M, A, D, ? (untracked), R, U… */
  letter: string;
  additions: number | null;
  deletions: number | null;
  binary: boolean;
  tooLarge: boolean;
}

/** `git status --porcelain`'s two columns as one letter. */
function statusLetter(status: string): string {
  const s = status.trim();
  return s === "??" ? "?" : (s[0] ?? "M");
}

function changeLetter(f: DiffFile): string {
  if (f.untracked) return "?";
  return f.change === "added" ? "A" : f.change === "deleted" ? "D" : "M";
}

/** The diff's files, lettered as `git.status` has them. */
export function changeItems(status: readonly GitStatusEntry[], diff: WorkspaceDiff): ChangeItem[] {
  const letters = new Map(status.map((e) => [e.file, statusLetter(e.status)]));
  return diff.files.map((f) => ({
    path: f.path,
    letter: letters.get(f.path) ?? changeLetter(f),
    additions: f.additions,
    deletions: f.deletions,
    binary: f.binary,
    tooLarge: f.too_large,
  }));
}

export interface DiffLine {
  kind: "add" | "del" | "ctx" | "note";
  text: string;
  /** Line numbers before and after; null where the line isn't on that side. */
  old: number | null;
  new: number | null;
}

export interface DiffHunk {
  header: string;
  lines: DiffLine[];
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

/** A unified patch as hunks of numbered lines; file headers are dropped. */
export function parsePatch(patch: string): DiffHunk[] {
  const hunks: DiffHunk[] = [];
  let hunk: DiffHunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  const lines = patch.split("\n");
  if (lines.at(-1) === "") lines.pop();
  for (const raw of lines) {
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
    const m = HUNK.exec(line);
    if (m) {
      hunk = { header: line, lines: [] };
      hunks.push(hunk);
      oldNo = Number(m[1]);
      newNo = Number(m[2]);
      continue;
    }
    if (!hunk) continue;
    const text = expandTabs(line.slice(1));
    switch (line[0]) {
      case "+":
        hunk.lines.push({ kind: "add", text, old: null, new: newNo++ });
        break;
      case "-":
        hunk.lines.push({ kind: "del", text, old: oldNo++, new: null });
        break;
      case " ":
        hunk.lines.push({ kind: "ctx", text, old: oldNo++, new: newNo++ });
        break;
      case "\\":
        hunk.lines.push({ kind: "note", text: line.slice(2), old: null, new: null });
        break;
      default:
        // The next file's headers.
        hunk = null;
    }
  }
  return hunks;
}

/** Lines past this are not shown; the file is for reading on a phone. */
export const PREVIEW_MAX_LINES = 5000;
/** Characters past this in one line are cut. */
export const PREVIEW_MAX_LINE = 1000;

export function expandTabs(text: string): string {
  return text.includes("\t") ? text.replace(/\t/g, "    ") : text;
}

/** A file's text as lines to show, and how many it has. */
export function previewLines(content: string): { lines: string[]; total: number } {
  const all = content.split(/\r?\n/);
  if (all.length > 1 && all.at(-1) === "") all.pop();
  const lines = all
    .slice(0, PREVIEW_MAX_LINES)
    .map((l) => expandTabs(l.length > PREVIEW_MAX_LINE ? `${l.slice(0, PREVIEW_MAX_LINE)}…` : l));
  return { lines, total: all.length };
}

export interface Problem {
  title: string;
  body: string;
}

/** Changes for a plain folder: nothing to diff, and nothing wrong. */
export function notGitProblem(workspace: WorkspaceInfo): Problem {
  return {
    title: "Not a Git repository",
    body: `${workspace.alias ?? workspace.name} is a plain folder, so there are no changes to show. Files, terminals and sessions work as usual.`,
  };
}

/** Why a folder, file or diff can't be shown, and what to do. */
export function fileProblem(e: unknown, hostName: string, what: "file" | "folder" | "changes"): Problem {
  if (isClientError(e, "network") || isClientError(e, "timeout") || isClientError(e, "unknown_outcome")) {
    return { title: `Not connected to ${hostName}`, body: "Try again when it's back." };
  }
  if (isClientError(e, "workspace_removed")) {
    return { title: "This worktree was removed", body: "Choose another workspace." };
  }
  if (isClientError(e, "not_found")) {
    return what === "folder"
      ? { title: "This folder is gone", body: `${hostName} no longer has it.` }
      : { title: "This file is gone", body: `${hostName} no longer has it.` };
  }
  if (isClientError(e, "permission_denied")) {
    return { title: `Can't read this ${what === "folder" ? "folder" : "file"}`, body: `${hostName} doesn't allow reading it.` };
  }
  if (isClientError(e, "not_a_git_repository")) {
    return { title: "Not a Git repository", body: "There are no changes to show here. Browse the files instead." };
  }
  const message = e instanceof Error ? e.message : "";
  if (isClientError(e, "invalid_input") && /binary/i.test(message)) {
    return { title: "Binary file", body: "It can't be shown as text." };
  }
  if (isClientError(e, "invalid_input") && /too large/i.test(message)) {
    return { title: "Too large to show", body: `Open it on ${hostName}.` };
  }
  if (isClientError(e, "invalid_input") && /not a file/i.test(message)) {
    return { title: "Not a file", body: "It can't be opened here." };
  }
  return { title: `Couldn't load this ${what === "changes" ? "diff" : what}`, body: "Try again." };
}
