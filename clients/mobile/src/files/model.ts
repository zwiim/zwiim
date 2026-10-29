// What the Files tab shows (DESIGN.md, Screen 6), from the host's answers:
// a folder's entries and the path above it, the files a search finds, the
// workspace's changes and where they sit in the tree, a file's patch as
// numbered lines, and a file's text in color. Pure; screens render it.

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

/** A folder's entry as Browse lists it: what is under a folder, and what changed. */
export interface BrowseRow {
  node: FileTreeNode;
  /** Folders: how many entries are inside ("3 items", "empty"); null when the host didn't say. */
  meta: string | null;
  /** Files: the change's letter. */
  letter: string | null;
  /** Folders: something under it changed. */
  changed: boolean;
}

/** One folder's rows, folders first; `changes` marks what differs from the last commit. */
export function browseRows(nodes: readonly FileTreeNode[], changes: readonly ChangeItem[]): BrowseRow[] {
  const letters = new Map(changes.map((c) => [c.path, c.letter]));
  return folderEntries(nodes).map((node) => {
    if (node.kind === "file") return { node, meta: null, letter: letters.get(node.path) ?? null, changed: false };
    // A symlinked folder is listed unexpanded: its count isn't known.
    const count = node.symlink ? null : node.children.length;
    const dir = `${node.path}/`;
    return {
      node,
      meta: count === null ? null : count === 0 ? "empty" : `${count} ${count === 1 ? "item" : "items"}`,
      letter: null,
      changed: changes.some((c) => c.path.startsWith(dir)),
    };
  });
}

export interface FoundFile {
  path: string;
  name: string;
  /** The folder it is in, `src/auth/`, or `./` at the top. */
  folder: string;
}

/** Files anywhere in `tree` whose name contains `query`, ignoring case; nothing for an empty query. */
export function findFiles(tree: readonly FileTreeNode[], query: string): FoundFile[] {
  const q = query.trim().toLowerCase();
  if (!q) return [];
  const found: FoundFile[] = [];
  const walk = (nodes: readonly FileTreeNode[]) => {
    for (const n of nodes) {
      if (n.kind === "dir") walk(n.children);
      else if (n.name.toLowerCase().includes(q)) {
        const folder = parentPath(n.path);
        found.push({ path: n.path, name: n.name, folder: folder ? `${folder}/` : "./" });
      }
    }
  };
  walk(tree);
  return found;
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

/** A diff entry's letter when `git.status` doesn't list it. */
export function changeLetter(f: DiffFile): string {
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

/** A change's letter in words. */
export function changeName(letter: string): string {
  return (
    { M: "Modified", A: "Added", D: "Deleted", "?": "Untracked", R: "Renamed", C: "Copied", U: "Conflicted", T: "Type changed" }[letter] ??
    "Changed"
  );
}

/** The workspace's `+n −n`, over the files that have counts. */
export function changeTotals(items: readonly ChangeItem[]): { additions: number; deletions: number } {
  return items.reduce(
    (t, c) => ({ additions: t.additions + (c.additions ?? 0), deletions: t.deletions + (c.deletions ?? 0) }),
    { additions: 0, deletions: 0 },
  );
}

/** "4 changed files". */
export function changedFilesText(n: number): string {
  return `${n} changed ${n === 1 ? "file" : "files"}`;
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

/** A hunk's header as a place in the file: "line 14 · createSession()". */
export function hunkLabel(header: string): string {
  const m = /^@@ -(\d+(?:,\d+)?) \+(\d+)(?:,\d+)? @@ ?(.*)$/.exec(header);
  if (!m) return header;
  if (m[1] === "0,0") return "new file";
  if (/ \+0,0 @@/.test(header)) return "deleted file";
  const where = `line ${m[2]}`;
  return m[3]?.trim() ? `${where} · ${m[3].trim()}` : where;
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
    return { title: "This worktree was archived", body: "Choose another workspace." };
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

export type SyntaxKind = "plain" | "keyword" | "string" | "number" | "comment" | "heading";

export interface SyntaxSpan {
  text: string;
  kind: SyntaxKind;
}

/** The extension a file is colored by: `ts`, `md`, or the name of a dotfile (`gitignore`). */
export function syntaxOf(path: string): string {
  const name = baseName(path);
  const dot = name.lastIndexOf(".");
  return dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
}

const HASH_COMMENTS = new Set(["sh", "bash", "zsh", "py", "rb", "toml", "yaml", "yml", "gitignore", "env", "dockerignore"]);
const MARKDOWN = new Set(["md", "markdown", "mdx"]);
const TOKEN =
  /(\/\/.*$|'(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*"|`(?:[^`\\]|\\.)*`|\b(?:import|from|export|default|const|let|var|return|if|else|for|while|function|fn|async|await|new|interface|type|enum|struct|impl|pub|use|mod|match|throw|try|catch|class|extends|private|public|readonly|constructor|super|this|self|true|false|null|undefined|def|echo|cd|set)\b|\b\d[\d_.]*\b)/g;

/** One line in a muted, keyword-level syntax color; a guess by extension, never a parser. */
export function highlight(line: string, ext: string): SyntaxSpan[] {
  if (!line) return [];
  if (MARKDOWN.has(ext)) return [{ text: line, kind: /^#/.test(line) ? "heading" : "plain" }];
  if (HASH_COMMENTS.has(ext) && /^\s*#/.test(line)) return [{ text: line, kind: "comment" }];
  const spans: SyntaxSpan[] = [];
  let last = 0;
  for (const m of line.matchAll(TOKEN)) {
    const at = m.index ?? 0;
    if (at > last) spans.push({ text: line.slice(last, at), kind: "plain" });
    const s = m[0];
    spans.push({ text: s, kind: s.startsWith("//") ? "comment" : /^['"`]/.test(s) ? "string" : /^\d/.test(s) ? "number" : "keyword" });
    last = at + s.length;
  }
  if (last < line.length) spans.push({ text: line.slice(last), kind: "plain" });
  return spans;
}
