import { describe, expect, it } from "vitest";
import { editChanges, toolChanges } from "../src/changes/diff";
import { fileSegments, fullFile, isMarkdown, revealStep, type FileLine } from "../src/changes/fullfile";
import { workspaceFile } from "../src/changes/paths";
import { clockTime, mergeChanges, turnChanges, turnDuration, turnGroups } from "../src/changes/turns";
import type { HistoryBlock, HistoryTurn } from "../src/types/protocol";

const kinds = (lines: FileLine[]) => lines.map((l) => `${l.kind[0]}${l.old ?? "-"}/${l.new ?? "-"}:${l.text}`);

describe("fullFile", () => {
  it("marks a change in the middle and fills the rest from the content", () => {
    const content = "a\nb\nX\nd\ne\n";
    const patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -2,3 +2,3 @@\n b\n-c\n+X\n d\n";
    expect(kinds(fullFile(patch, content))).toEqual([
      "c1/1:a",
      "c2/2:b",
      "d3/-:c",
      "a-/3:X",
      "c4/4:d",
      "c5/5:e",
    ]);
  });

  it("shifts old numbers past an added line", () => {
    const content = "a\nnew\nb\nc\n";
    const patch = "@@ -1,1 +1,2 @@\n a\n+new\n";
    expect(kinds(fullFile(patch, content))).toEqual(["c1/1:a", "a-/2:new", "c2/3:b", "c3/4:c"]);
  });

  it("reads a new file as all added", () => {
    const patch = "--- /dev/null\n+++ b/n\n@@ -0,0 +1,2 @@\n+one\n+two\n";
    expect(kinds(fullFile(patch, "one\ntwo\n"))).toEqual(["a-/1:one", "a-/2:two"]);
  });

  it("reads a deleted file as its removed lines", () => {
    const patch = "--- a/n\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-one\n-two\n";
    expect(kinds(fullFile(patch, null))).toEqual(["d1/-:one", "d2/-:two"]);
  });

  it("reads a file with no patch as unchanged", () => {
    expect(kinds(fullFile(null, "a\nb"))).toEqual(["c1/1:a", "c2/2:b"]);
  });

  it("keeps a last hunk with no trailing newline", () => {
    const patch = "@@ -2 +2 @@\n-b\n\\ No newline at end of file\n+B\n\\ No newline at end of file\n";
    expect(kinds(fullFile(patch, "a\nB"))).toEqual(["c1/1:a", "d2/-:b", "a-/2:B"]);
  });

  it("places a pure deletion after the line it follows", () => {
    const patch = "@@ -2,1 +1,0 @@\n-gone\n";
    expect(kinds(fullFile(patch, "a\nc\n"))).toEqual(["c1/1:a", "d2/-:gone", "c3/2:c"]);
  });

  it("reads CRLF content and patches", () => {
    const patch = "@@ -1,2 +1,2 @@\r\n a\r\n-b\r\n+B\r\n";
    expect(kinds(fullFile(patch, "a\r\nB\r\nc\r\n"))).toEqual(["c1/1:a", "d2/-:b", "a-/2:B", "c3/3:c"]);
  });

  it("stops filling where shorter content ends", () => {
    const patch = "@@ -5,1 +5,1 @@\n-e\n+E\n";
    expect(kinds(fullFile(patch, "a\nb\n"))).toEqual(["c1/1:a", "c2/2:b", "d5/-:e", "a-/5:E"]);
  });
});

const ctx = (n: number): FileLine[] =>
  Array.from({ length: n }, (_, i) => ({ kind: "ctx", text: `l${i}`, old: i + 1, new: i + 1 }));

describe("fileSegments", () => {
  it("folds unchanged runs beyond three lines of context", () => {
    const lines = ctx(20);
    lines[10] = { kind: "add", text: "x", old: null, new: 11 };
    expect(fileSegments(lines, {})).toEqual([
      { kind: "fold", at: 0, count: 7 },
      { kind: "lines", from: 7, to: 14 },
      { kind: "fold", at: 14, count: 6 },
    ]);
  });

  it("shows a hidden run of fewer than four lines", () => {
    const lines = ctx(8);
    lines[6] = { kind: "add", text: "x", old: null, new: 7 };
    expect(fileSegments(lines, {})).toEqual([{ kind: "lines", from: 0, to: 8 }]);
  });

  it("reveals from a fold's top and keeps its key", () => {
    const lines = ctx(20);
    lines[19] = { kind: "add", text: "x", old: null, new: 20 };
    expect(fileSegments(lines, { 0: 5 })).toEqual([
      { kind: "lines", from: 0, to: 5 },
      { kind: "fold", at: 0, count: 11 },
      { kind: "lines", from: 16, to: 20 },
    ]);
    expect(fileSegments(lines, { 0: 16 })).toEqual([{ kind: "lines", from: 0, to: 20 }]);
  });

  it("folds nothing when nothing changed", () => {
    expect(fileSegments(ctx(500), {})).toEqual([{ kind: "lines", from: 0, to: 500 }]);
    expect(fileSegments([], {})).toEqual([]);
  });

  it("reveals all of a short fold, else 100", () => {
    expect(revealStep(200)).toBe(200);
    expect(revealStep(451)).toBe(100);
  });

  it("knows Markdown", () => {
    expect(isMarkdown("docs/A.MD")).toBe(true);
    expect(isMarkdown("x.mdx")).toBe(true);
    expect(isMarkdown("x.markdown")).toBe(true);
    expect(isMarkdown("md.ts")).toBe(false);
  });
});

const turn = (role: string, timestamp: string, blocks: HistoryBlock[] = [], completed_at: string | null = null): HistoryTurn => ({
  turn_id: `${role}-${timestamp}`,
  role,
  blocks,
  timestamp,
  completed_at,
  duration_ms: null,
  model: null,
  usage: null,
});
const call = (id: string, title: string, input: unknown): HistoryBlock => ({
  type: "tool_call",
  tool_call_id: id,
  title,
  input_preview: JSON.stringify(input),
  status: "completed",
});
const result = (id: string, is_error = false): HistoryBlock => ({ type: "tool_result", tool_call_id: id, output_preview: "ok", is_error });

describe("turnGroups", () => {
  const history = [
    turn("user", "2026-10-06T10:00:00Z"),
    turn("assistant", "2026-10-06T10:00:05Z", [
      call("1", "Edit", { file_path: "/w/a.ts", old_string: "x", new_string: "y\nz" }),
      result("1"),
    ]),
    turn("assistant", "2026-10-06T10:01:00Z", [
      call("2", "Write", { file_path: "/w/b.md", content: "hi\n" }),
      call("3", "Edit", { file_path: "/w/a.ts", old_string: "q", new_string: "r" }),
      call("4", "Edit", { file_path: "/w/c.ts", old_string: "q", new_string: "r" }),
      call("5", "Bash", { command: "git diff" }),
      result("2"),
      result("3"),
      result("4", true),
      result("5"),
    ], "2026-10-06T10:09:31Z"),
    turn("user", "2026-10-06T10:10:00Z"),
    turn("assistant", "2026-10-06T10:10:42Z"),
    turn("user", "2026-10-06T10:11:00Z"),
  ];

  it("groups each answered prompt with its agent turns", () => {
    const groups = turnGroups(history);
    expect(groups.map((g) => [g.start, g.end])).toEqual([
      [0, 2],
      [3, 4],
    ]);
    expect(groups[0]!.durationMs).toBe(571_000);
    expect(groups[0]!.completedAt).toBe("2026-10-06T10:09:31Z");
    expect(groups[1]!.durationMs).toBe(42_000);
  });

  it("sums a turn's edits by file and skips failed ones and commands", () => {
    const [first, second] = turnGroups(history);
    expect(first!.files).toEqual([
      { path: "/w/a.ts", added: 3, removed: 2 },
      { path: "/w/b.md", added: 1, removed: 0 },
    ]);
    expect(second!.files).toEqual([]);
  });

  it("counts a sub-agent's edits, at any depth, as the turn's", () => {
    const agent = (id: string, blocks: HistoryBlock[]): HistoryBlock => ({
      type: "tool_call",
      tool_call_id: id,
      title: "Agent",
      input_preview: "{}",
      status: null,
      subagent: { blocks },
    });
    const turns = [
      turn("assistant", "2026-10-06T10:00:05Z", [
        call("1", "Edit", { file_path: "/w/a.ts", old_string: "x", new_string: "y" }),
        result("1"),
        agent("2", [
          call("3", "Write", { file_path: "/w/b.md", content: "hi\n" }),
          result("3"),
          call("4", "Edit", { file_path: "/w/c.ts", old_string: "q", new_string: "r" }),
          result("4", true),
          agent("5", [call("6", "Edit", { file_path: "/w/a.ts", old_string: "p", new_string: "s" }), result("6")]),
        ]),
        result("2"),
      ]),
    ];
    expect(turnChanges(turns)).toEqual([
      { path: "/w/a.ts", added: 2, removed: 2 },
      { path: "/w/b.md", added: 1, removed: 0 },
    ]);
  });

  it("merges by first change", () => {
    const d = (path: string, added: number, removed: number) => ({ path, lines: [], added, removed });
    expect(mergeChanges([d("b", 1, 0), d("a", 2, 1), d("b", 3, 3)])).toEqual([
      { path: "b", added: 4, removed: 3 },
      { path: "a", added: 2, removed: 1 },
    ]);
  });
});

describe("turn time", () => {
  it("reads durations", () => {
    expect(turnDuration(42_000)).toBe("42s");
    expect(turnDuration(571_000)).toBe("9m 31s");
    expect(turnDuration(3_840_000)).toBe("1h 4m");
  });

  it("reads a clock time, or nothing", () => {
    expect(clockTime("2026-10-06T10:09:31Z")).toMatch(/\d{1,2}:\d{2}/);
    expect(clockTime("nope")).toBe("");
  });
});

describe("editChanges / toolChanges", () => {
  it("reads Edit, Write and apply_patch inputs", () => {
    expect(editChanges("Edit", JSON.stringify({ file_path: "a", old_string: "x", new_string: "y" }))?.[0]?.path).toBe("a");
    expect(editChanges("Write", JSON.stringify({ file_path: "b", content: "1\n2\n" }))?.[0]?.added).toBe(2);
    const patch = "*** Begin Patch\n*** Update File: c.ts\n@@\n-a\n+b\n*** End Patch";
    expect(editChanges("apply_patch", patch)?.[0]?.path).toBe("c.ts");
    expect(editChanges("Edit", "not json")).toBeNull();
  });

  it("counts only edits that did not fail", () => {
    const input = JSON.stringify({ file_path: "a", old_string: "x", new_string: "y" });
    expect(toolChanges("Edit", input, null, true)).toBeNull();
    expect(toolChanges("Bash", null, "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n", false)).toBeNull();
    expect(toolChanges("edit", input, "--- a/z\n+++ b/z\n@@ -1 +1 @@\n-a\n+b\n", false)?.[0]?.path).toBe("z");
  });
});

describe("workspaceFile", () => {
  const ws = { display_path: "~/code/app", name: "app" };
  it("resolves relative and absolute paths inside the workspace", () => {
    expect(workspaceFile("src/a.ts", ws)).toBe("src/a.ts");
    expect(workspaceFile("/Users/me/code/app/src/a.ts:12", ws)).toBe("src/a.ts");
    expect(workspaceFile("/elsewhere/a.ts", ws)).toBeNull();
    expect(workspaceFile("../a.ts", ws)).toBeNull();
    expect(workspaceFile(null, ws)).toBeNull();
  });
});
