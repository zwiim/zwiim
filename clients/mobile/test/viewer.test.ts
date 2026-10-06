import { ZwiimClientError, type DiffFile, type FileContent, type WorkspaceDiff } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import { loadViewed, viewedFile } from "../src/files/viewer";

const changed = (path: string, extra: Partial<DiffFile> = {}): DiffFile => ({
  path,
  change: "modified",
  additions: 1,
  deletions: 1,
  binary: false,
  too_large: false,
  untracked: false,
  ...extra,
});

const diff = (file: DiffFile, patch: string | null = "@@ -2 +2 @@\n-old\n+new\n"): WorkspaceDiff => ({
  base_commit: null,
  truncated: false,
  files: [file],
  patch,
});

const read = (content: string): FileContent => ({ path: "a.txt", content, etag: "e", mtime_ms: 0, readonly: false, line_ending: "lf" });
const refused = (code: "not_found" | "invalid_input" | "network", message: string) => new ZwiimClientError(code, message, 400);

describe("the file viewer", () => {
  it("marks the changes in the whole file", () => {
    const v = viewedFile("a.txt", diff(changed("a.txt")), { content: "one\nnew\nthree\n", refused: null });
    expect(v.kind === "lines" && v.lines.map((l) => `${l.kind}:${l.text}`)).toEqual(["ctx:one", "del:old", "add:new", "ctx:three"]);
    expect(v.kind === "lines" && v.content).toBe("one\nnew\nthree\n");
  });

  it("shows a deleted file's old text with nothing to copy", () => {
    const v = viewedFile("a.txt", diff(changed("a.txt", { change: "deleted" }), "@@ -1,2 +0,0 @@\n-a\n-b\n"), {
      content: null,
      refused: null,
    });
    expect(v.kind === "lines" && v.lines.map((l) => l.kind)).toEqual(["del", "del"]);
    expect(v.kind === "lines" && v.content).toBeNull();
  });

  it("says a file too large to open is, whichever answer says so", () => {
    expect(viewedFile("a.txt", diff(changed("a.txt", { too_large: true }), null), { content: "x", refused: null }).kind).toBe(
      "too_large",
    );
    expect(viewedFile("a.txt", diff(changed("a.txt"), null), { content: "x", refused: null }).kind).toBe("too_large");
    expect(viewedFile("a.txt", null, { content: null, refused: "too_large" }).kind).toBe("too_large");
  });

  it("shows an unchanged file as it is, and says when it is gone", () => {
    const v = viewedFile("a.txt", null, { content: "same\n", refused: null });
    expect(v.kind === "lines" && v.file).toBeNull();
    expect(v.kind === "lines" && v.lines.map((l) => l.kind)).toEqual(["ctx"]);
    expect(viewedFile("a.txt", null, { content: null, refused: null }).kind).toBe("gone");
  });

  it("reads both, taking no changes and a refused read as answers", async () => {
    const noChanges = Promise.reject(refused("not_found", "File has no changes"));
    expect((await loadViewed("a.txt", noChanges, Promise.resolve(read("same\n")))).kind).toBe("lines");
    const big = Promise.reject(refused("invalid_input", "File is too large to open in editor"));
    expect((await loadViewed("a.txt", Promise.reject(refused("not_found", "none")), big)).kind).toBe("too_large");
    const binary = Promise.reject(refused("invalid_input", "Binary file"));
    expect((await loadViewed("a.txt", Promise.reject(refused("not_found", "none")), binary)).kind).toBe("binary");
  });

  it("fails when the host can't be reached", async () => {
    const down = () => Promise.reject(refused("network", "offline"));
    await expect(loadViewed("a.txt", down(), Promise.resolve(read("x")))).rejects.toThrow("offline");
    await expect(loadViewed("a.txt", Promise.resolve(diff(changed("a.txt"))), down())).rejects.toThrow("offline");
  });
});
