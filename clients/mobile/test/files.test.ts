import { ZwiimClientError, type WorkspaceDiff, type WorkspaceInfo } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  changeItems,
  crumbs,
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

  it("is the chosen one while listed, else the first root", () => {
    const list = [ws("wt", { kind: "worktree" }), ws("a"), ws("b"), ws("gone", { removed: true })];
    expect(currentWorkspace(list, "b")?.workspace_id).toBe("b");
    expect(currentWorkspace(list, null)?.workspace_id).toBe("a");
    expect(currentWorkspace(list, "gone")?.workspace_id).toBe("a");
    expect(currentWorkspace(list, "missing")?.workspace_id).toBe("a");
    expect(currentWorkspace([], null)).toBeNull();
    expect(currentWorkspace(null, "a")).toBeNull();
  });
});
