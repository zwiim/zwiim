import { ZwiimClientError, type WorkspaceDiff, type WorkspaceInfo } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  browseRows,
  changedFilesText,
  changeItems,
  changeName,
  changeTotals,
  crumbs,
  findFiles,
  highlight,
  hunkLabel,
  syntaxOf,
  fileProblem,
  notGitProblem,
  folderEntries,
  parentPath,
  parsePatch,
  PREVIEW_MAX_LINES,
  previewLines,
} from "../src/files/model";
import { currentWorkspace } from "../src/host/workspace";

describe("browsing", () => {
  it("builds the breadcrumb from the workspace down", () => {
    expect(crumbs("site", "")).toEqual([{ label: "site", path: "" }]);
    expect(crumbs("site", "src/app/")).toEqual([
      { label: "site", path: "" },
      { label: "src", path: "src" },
      { label: "app", path: "src/app" },
    ]);
    expect(parentPath("src/app")).toBe("src");
    expect(parentPath("src")).toBe("");
  });

  it("lists folders first, then files, in the host's order", () => {
    const entries = folderEntries([
      { kind: "file", name: "README.md", path: "README.md" },
      { kind: "dir", name: "src", path: "src", children: [] },
      { kind: "file", name: "a.ts", path: "a.ts" },
      { kind: "dir", name: "docs", path: "docs", children: [] },
    ]);
    expect(entries.map((e) => e.name)).toEqual(["src", "docs", "README.md", "a.ts"]);
  });
});

describe("the tree and its changes", () => {
  const change = (path: string, letter: string, additions = 1, deletions = 0) => ({
    path,
    letter,
    additions,
    deletions,
    binary: false,
    tooLarge: false,
  });

  it("says what each folder holds and marks what changed", () => {
    const rows = browseRows(
      [
        { kind: "file", name: "session.ts", path: "src/session.ts" },
        { kind: "dir", name: "auth", path: "src/auth", children: [{ kind: "file", name: "token.ts", path: "src/auth/token.ts" }] },
        { kind: "dir", name: "archive", path: "src/archive", children: [] },
        { kind: "dir", name: "linked", path: "src/linked", children: [], symlink: true },
        { kind: "dir", name: "host", path: "src/host", children: [
          { kind: "file", name: "a.ts", path: "src/host/a.ts" },
          { kind: "file", name: "b.ts", path: "src/host/b.ts" },
        ] },
      ],
      [change("src/session.ts", "M"), change("src/auth/token.ts", "A"), change("src/hostile.ts", "M")],
    );
    expect(rows.map((r) => [r.node.name, r.meta, r.letter, r.changed])).toEqual([
      ["auth", "1 item", null, true],
      ["archive", "empty", null, false],
      ["linked", null, null, false],
      // A sibling's name starting with "host" isn't inside host/.
      ["host", "2 items", null, false],
      ["session.ts", null, "M", false],
    ]);
  });

  it("finds files by part of their name, anywhere, ignoring case", () => {
    const tree = [
      { kind: "file" as const, name: "README.md", path: "README.md" },
      { kind: "dir" as const, name: "src", path: "src", children: [
        { kind: "dir" as const, name: "auth", path: "src/auth", children: [
          { kind: "file" as const, name: "session.ts", path: "src/auth/session.ts" },
        ] },
      ] },
      { kind: "file" as const, name: "session.test.ts", path: "session.test.ts" },
    ];
    expect(findFiles(tree, " Session ")).toEqual([
      { path: "src/auth/session.ts", name: "session.ts", folder: "src/auth/" },
      { path: "session.test.ts", name: "session.test.ts", folder: "./" },
    ]);
    // Folders aren't results.
    expect(findFiles(tree, "auth")).toEqual([]);
    expect(findFiles(tree, "  ")).toEqual([]);
  });

  it("totals the changes for the Changes row", () => {
    const items = [change("a", "M", 3, 1), { ...change("b.png", "A"), additions: null, deletions: null, binary: true }, change("c", "D", 0, 9)];
    expect(changeTotals(items)).toEqual({ additions: 3, deletions: 10 });
    expect(changedFilesText(1)).toBe("1 changed file");
    expect(changedFilesText(4)).toBe("4 changed files");
    expect(changeName("M")).toBe("Modified");
    expect(changeName("?")).toBe("Untracked");
    expect(changeName("X")).toBe("Changed");
  });

  it("places a hunk by line", () => {
    expect(hunkLabel("@@ -13,4 +14,7 @@ createSession()")).toBe("line 14 · createSession()");
    expect(hunkLabel("@@ -10 +10,2 @@")).toBe("line 10");
    expect(hunkLabel("@@ -0,0 +1 @@")).toBe("new file");
    expect(hunkLabel("@@ -1,3 +0,0 @@")).toBe("deleted file");
    expect(hunkLabel("not a hunk")).toBe("not a hunk");
  });
});

describe("coloring a file", () => {
  it("picks the language by extension", () => {
    expect(syntaxOf("src/auth/session.ts")).toBe("ts");
    expect(syntaxOf(".gitignore")).toBe("gitignore");
    expect(syntaxOf("Makefile")).toBe("");
  });

  it("colors keywords, strings, numbers and comments", () => {
    expect(highlight("const ttl = 15 * 60; // minutes", "ts")).toEqual([
      { text: "const", kind: "keyword" },
      { text: " ttl = ", kind: "plain" },
      { text: "15", kind: "number" },
      { text: " * ", kind: "plain" },
      { text: "60", kind: "number" },
      { text: "; ", kind: "plain" },
      { text: "// minutes", kind: "comment" },
    ]);
    expect(highlight("import x from './x';", "ts").map((s) => s.kind)).toEqual(["keyword", "plain", "keyword", "plain", "string", "plain"]);
    expect(highlight("# Title", "md")).toEqual([{ text: "# Title", kind: "heading" }]);
    expect(highlight("# nightly backup", "sh")).toEqual([{ text: "# nightly backup", kind: "comment" }]);
    expect(highlight("", "ts")).toEqual([]);
  });
});

describe("changes", () => {
  const diff = (files: Partial<WorkspaceDiff["files"][number]>[]): WorkspaceDiff => ({
    base_commit: "abc",
    truncated: false,
    files: files.map((f) => ({
      path: "x",
      change: "modified",
      additions: 1,
      deletions: 0,
      binary: false,
      too_large: false,
      untracked: false,
      ...f,
    })),
  });

  it("letters each changed file as git.status does, with its counts", () => {
    const items = changeItems(
      [
        { status: "MM", file: "src/a.ts" },
        { status: "??", file: "new.txt" },
        { status: "D", file: "old.txt" },
      ],
      diff([
        { path: "src/a.ts", additions: 3, deletions: 1 },
        { path: "new.txt", change: "added", untracked: true, additions: 2 },
        { path: "old.txt", change: "deleted", additions: 0, deletions: 9 },
        { path: "logo.png", change: "added", additions: null, deletions: null, binary: true },
      ]),
    );
    expect(items).toEqual([
      { path: "src/a.ts", letter: "M", additions: 3, deletions: 1, binary: false, tooLarge: false },
      { path: "new.txt", letter: "?", additions: 2, deletions: 0, binary: false, tooLarge: false },
      { path: "old.txt", letter: "D", additions: 0, deletions: 9, binary: false, tooLarge: false },
      // Not in git.status (say, listed a moment later): lettered by the diff.
      { path: "logo.png", letter: "A", additions: null, deletions: null, binary: true, tooLarge: false },
    ]);
  });

  it("reads a patch into numbered hunks", () => {
    const patch = [
      "diff --git a/a.ts b/a.ts",
      "index 1..2 100644",
      "--- a/a.ts",
      "+++ b/a.ts",
      "@@ -1,3 +1,3 @@ fn main",
      " keep",
      "-\told",
      "+\tnew",
      " end",
      "@@ -10 +10,2 @@",
      " x",
      "+y",
      "\\ No newline at end of file",
      "",
    ].join("\n");
    expect(parsePatch(patch)).toEqual([
      {
        header: "@@ -1,3 +1,3 @@ fn main",
        lines: [
          { kind: "ctx", text: "keep", old: 1, new: 1 },
          { kind: "del", text: "    old", old: 2, new: null },
          { kind: "add", text: "    new", old: null, new: 2 },
          { kind: "ctx", text: "end", old: 3, new: 3 },
        ],
      },
      {
        header: "@@ -10 +10,2 @@",
        lines: [
          { kind: "ctx", text: "x", old: 10, new: 10 },
          { kind: "add", text: "y", old: null, new: 11 },
          { kind: "note", text: "No newline at end of file", old: null, new: null },
        ],
      },
    ]);
    expect(parsePatch("")).toEqual([]);
  });
});

describe("previewing a file", () => {
  it("splits lines, keeping blank ones and dropping the final newline", () => {
    expect(previewLines("a\r\n\tb\n\nc\n")).toEqual({ lines: ["a", "    b", "", "c"], total: 4 });
    expect(previewLines("")).toEqual({ lines: [""], total: 1 });
  });

  it("shows the start of a very long file", () => {
    const { lines, total } = previewLines("x\n".repeat(PREVIEW_MAX_LINES + 10));
    expect(lines).toHaveLength(PREVIEW_MAX_LINES);
    expect(total).toBe(PREVIEW_MAX_LINES + 10);
  });

  it("says a plain folder has no changes, without calling it an error", () => {
    const plain: WorkspaceInfo = {
      workspace_id: "w",
      name: "notes",
      alias: null,
      kind: "root",
      parent_workspace_id: null,
      is_git_repo: false,
      branch: null,
      base_branch: null,
      base_commit: null,
      removed: false,
      display_path: "~/notes",
    };
    expect(notGitProblem(plain)).toEqual({
      title: "Not a Git repository",
      body: "notes is a plain folder, so there are no changes to show. Files, terminals and sessions work as usual.",
    });
  });

  it("says why a file can't be shown, from the host's error", () => {
    const err = (code: string, message = "m") => new ZwiimClientError(code as never, message);
    expect(fileProblem(err("not_found", "File does not exist"), "Mac", "file").title).toBe("This file is gone");
    expect(fileProblem(err("not_found", "Directory not found"), "Mac", "folder").title).toBe("This folder is gone");
    expect(fileProblem(err("permission_denied"), "Mac", "file").title).toBe("Can't read this file");
    expect(fileProblem(err("invalid_input", "Binary files are not supported in preview"), "Mac", "file").title).toBe("Binary file");
    expect(fileProblem(err("invalid_input", "File is too large to open in editor"), "Mac", "file")).toEqual({
      title: "Too large to show",
      body: "Open it on Mac.",
    });
    expect(fileProblem(err("not_a_git_repository"), "Mac", "changes").title).toBe("Not a Git repository");
    expect(fileProblem(err("network"), "Mac", "file").title).toBe("Not connected to Mac");
    expect(fileProblem(new Error("boom"), "Mac", "file").title).toBe("Couldn't load this file");
  });
});

describe("the workspace Files and Terminal show", () => {
  const ws = (id: string, over: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
    workspace_id: id,
    name: id,
    alias: null,
    kind: "root",
    parent_workspace_id: null,
    branch: "main",
    base_branch: null,
    base_commit: null,
    removed: false,
    is_git_repo: true,
    display_path: null,
    ...over,
  });

  it("is never a root: a chosen root opens its own worktree, if it has one", () => {
    const wt = (id: string, root: string, over: Partial<WorkspaceInfo> = {}) =>
      ws(id, { kind: "worktree", parent_workspace_id: root, ...over });
    const list = [
      ws("plain", { is_git_repo: false }),
      ws("a"),
      wt("a1", "a"),
      wt("a2", "a"),
      wt("a3", "a", { removed: true }),
      ws("b"),
      wt("b1", "b"),
      wt("b2", "b", { removed: true }),
      ws("gone", { removed: true }),
    ];
    const at = (updated_at: string) => [{ updated_at }] as never;
    expect(currentWorkspace(list, "b1")?.workspace_id).toBe("b1");
    // A root opens the worktree prompted last, else its last live one.
    expect(currentWorkspace(list, "a")?.workspace_id).toBe("a2");
    expect(currentWorkspace(list, "a", { a1: at("2026-10-30T10:00:00Z") })?.workspace_id).toBe("a1");
    // A chosen root with no worktree shows none: never another workspace's worktree.
    expect(currentWorkspace(list, "plain")).toBeNull();
    // A removed worktree: its own root's.
    expect(currentWorkspace(list, "a3")?.workspace_id).toBe("a2");
    expect(currentWorkspace(list, "b2")?.workspace_id).toBe("b1");
    // Nothing, or nothing live, chosen: the first root that has one.
    expect(currentWorkspace(list, null)?.workspace_id).toBe("a2");
    expect(currentWorkspace(list, "missing")?.workspace_id).toBe("a2");
    expect(currentWorkspace(list, "gone")?.workspace_id).toBe("a2");
    // A worktree whose root isn't listed still counts.
    expect(currentWorkspace([ws("plain", { is_git_repo: false }), wt("x", "elsewhere")], null)?.workspace_id).toBe("x");
    // Only roots without worktrees: nothing to show.
    expect(currentWorkspace([ws("a"), ws("plain", { is_git_repo: false })], "a")).toBeNull();
    expect(currentWorkspace([], null)).toBeNull();
    expect(currentWorkspace(null, "a")).toBeNull();
  });
});
