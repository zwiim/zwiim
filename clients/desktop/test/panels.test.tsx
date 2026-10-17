import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, ZwiimClientError, type WorkspaceInfo } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { DiffPanel, numberPatch } from "@/panels/DiffPanel";
import { fileIconId, folderIconId } from "@/lib/file-icons";
import { fileIcon, FilesPanel, FileView } from "@/panels/FilesPanel";
import { GitPanel, relativeTime, statusLabel } from "@/panels/GitPanel";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

async function mount(t: FakeTransport, ui: React.ReactNode) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(<HostProvider value={controller}>{ui}</HostProvider>);
  await tick();
  return controller;
}

const log = (entries: object[] = []) => ({ entries, has_upstream: false });

const W1: WorkspaceInfo = {
  workspace_id: "w1",
  name: "repo",
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  is_git_repo: true,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  display_path: null,
};
const PLAIN: WorkspaceInfo = { ...W1, workspace_id: "p1", name: "notes", is_git_repo: false, branch: null };

describe("git panel", () => {
  it("names status codes and dates readably", () => {
    expect(statusLabel("M").label).toBe("Modified");
    expect(statusLabel("MM").label).toBe("Modified");
    expect(statusLabel("??").label).toBe("Untracked");
    expect(statusLabel("UU").label).toBe("Conflict");
    expect(statusLabel("D").label).toBe("Deleted");
    const now = Date.parse("2026-09-24T12:00:00Z");
    expect(relativeTime("2026-09-24T11:59:30Z", now)).toBe("just now");
    expect(relativeTime("2026-09-24T11:55:00Z", now)).toBe("5 min ago");
    expect(relativeTime("2026-09-23T12:00:00Z", now)).toBe("yesterday");
    expect(relativeTime("not a date", now)).toBe("not a date");
  });

  it("commits the checked files with a message, then refreshes", async () => {
    const t = new FakeTransport();
    let status = [
      { status: "M", file: "a.txt" },
      { status: "??", file: "b.txt" },
    ];
    t.handlers["git.status"] = () => status;
    t.handlers["git.log"] = () =>
      log([{ hash: "abc1234", full_hash: "abc".padEnd(40, "0"), author: "t", date: "2026-09-24T00:00:00Z", message: "init\n\nbody", files: [], pushed: false }]);
    t.handlers["git.commit"] = (p) => {
      status = [{ status: "??", file: "b.txt" }];
      return { committed_files: p.files.length };
    };
    await mount(t, <GitPanel workspace={W1} />);
    expect(screen.getByText("Modified")).toBeTruthy();
    expect(screen.getByText("Untracked")).toBeTruthy();
    expect(screen.getByText("init")).toBeTruthy();
    expect(screen.getByText("local")).toBeTruthy();

    const commit = () => screen.getByRole("button", { name: /^Commit/ });
    expect((commit() as HTMLButtonElement).disabled).toBe(true); // no message yet
    fireEvent.change(screen.getByLabelText("Commit message"), { target: { value: "  fix a  " } });
    // Leave b.txt out.
    fireEvent.click(screen.getAllByRole("checkbox")[2]!);
    expect(commit().textContent).toContain("Commit 1 file");
    fireEvent.click(commit());
    await tick();
    expect(t.calls.find((c) => c.op === "git.commit")!.params).toEqual({ workspace_id: "w1", message: "fix a", files: ["a.txt"] });
    expect(screen.getByTestId("git-notice").textContent).toBe("Committed 1 file.");
    expect(screen.queryByText("Modified")).toBeNull();
    expect((screen.getByLabelText("Commit message") as HTMLTextAreaElement).value).toBe("");
  });

  it("does not retry a commit whose outcome is unknown, and shows the repo as it is", async () => {
    const t = new FakeTransport();
    t.handlers["git.status"] = () => [{ status: "M", file: "a.txt" }];
    t.handlers["git.log"] = () => log();
    t.handlers["git.commit"] = () => {
      throw new ZwiimClientError("unknown_outcome", "lost");
    };
    await mount(t, <GitPanel workspace={W1} />);
    fireEvent.change(screen.getByLabelText("Commit message"), { target: { value: "x" } });
    const statusCalls = t.calls.filter((c) => c.op === "git.status").length;
    fireEvent.click(screen.getByRole("button", { name: /^Commit/ }));
    await tick();
    expect(t.calls.filter((c) => c.op === "git.commit")).toHaveLength(1);
    expect(screen.getByTestId("git-commit-error").textContent).toBeTruthy();
    expect(t.calls.filter((c) => c.op === "git.status").length).toBe(statusCalls + 1);
  });

  it("refetches when the host says the diff changed", async () => {
    const t = new FakeTransport();
    t.handlers["git.status"] = () => [];
    t.handlers["git.log"] = () => log();
    await mount(t, <GitPanel workspace={W1} />);
    expect(screen.getByText("Working tree clean")).toBeTruthy();
    const before = t.calls.filter((c) => c.op === "git.status").length;
    await t.feedEvent("workspace.diff_changed", { workspace_id: "w1" });
    await tick();
    expect(t.calls.filter((c) => c.op === "git.status").length).toBe(before + 1);
  });
});

describe("a plain folder (not a Git repository)", () => {
  it("says so in the Git and Diff panels, without asking Git", async () => {
    const t = new FakeTransport();
    await mount(
      t,
      <>
        <GitPanel workspace={PLAIN} />
        <DiffPanel workspace={PLAIN} selected={null} onOpen={() => {}} />
      </>,
    );
    const notes = screen.getAllByTestId("not-git-repository");
    expect(notes).toHaveLength(2);
    expect(notes[0]!.textContent).toContain("Not a Git repository");
    expect(notes[0]!.textContent).toContain("notes is a plain folder");
    expect(screen.queryByRole("alert")).toBeNull();
    expect(t.calls.filter((c) => c.op.startsWith("git."))).toEqual([]);
  });
});

describe("diff line numbers", () => {
  it("numbers old and new lines from the hunk header", () => {
    const lines = numberPatch(
      ["--- a/x", "+++ b/x", "@@ -3,3 +3,4 @@", " keep", "-old", "+new", "+more", " tail", "\\ No newline at end of file"].join("\n"),
    );
    expect(lines.map((l) => [l.kind, l.old, l.new])).toEqual([
      ["meta", null, null],
      ["meta", null, null],
      ["hunk", null, null],
      ["context", 3, 3],
      ["del", 4, null],
      ["add", null, 4],
      ["add", null, 5],
      ["context", 5, 6],
      ["meta", null, null],
    ]);
  });
});

describe("files panel", () => {
  it("shows loading, line numbers and a file's error", async () => {
    const t = new FakeTransport();
    let release!: () => void;
    t.handlers["workspace.tree"] = () => [
      { name: "a.txt", path: "a.txt", kind: "file" },
      { name: "big.bin", path: "big.bin", kind: "file" },
    ];
    t.handlers["files.read"] = (p) => {
      if (p.path === "big.bin") throw new ZwiimClientError("invalid_input", "File is too large to open in editor", 400);
      return new Promise((resolve) => {
        release = () =>
          resolve({ path: "a.txt", content: "one\ntwo\n", etag: "e", mtime_ms: null, readonly: false, line_ending: "lf" });
      });
    };
    const onOpen = vi.fn();
    await mount(t, <FilesPanel workspaceId="w1" selected={null} onOpen={onOpen} />);
    fireEvent.click(screen.getByText("a.txt"));
    expect(onOpen).toHaveBeenCalledWith("a.txt");
    cleanup();

    await mount(t, <FileView workspaceId="w1" path="a.txt" />);
    expect(screen.getByText("Opening…")).toBeTruthy();
    release();
    await tick();
    const rows = screen.getByTestId("file-content").querySelectorAll("tr");
    expect([...rows].map((r) => r.textContent)).toEqual(["1one", "2two"]);
    cleanup();

    await mount(t, <FileView workspaceId="w1" path="big.bin" />);
    expect(screen.getByTestId("file-error").textContent).toBe("File is too large to open in editor");
    expect(screen.queryByTestId("file-content")).toBeNull();
  });

  it("shows an empty workspace and a folder that failed to list", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.tree"] = (p) => {
      if (!p.path) return [{ name: "src", path: "src", kind: "dir" }];
      throw new ZwiimClientError("permission_denied", "Permission denied", 403);
    };
    await mount(t, <FilesPanel workspaceId="w1" selected={null} onOpen={() => {}} />);
    fireEvent.click(screen.getByText("src"));
    await tick();
    expect(screen.getByRole("alert").textContent).toBe("Permission denied");
  });

  it("explains a workspace whose folder is gone instead of showing the host's error", async () => {
    const t = new FakeTransport();
    t.handlers["workspace.tree"] = () => {
      throw new ZwiimClientError("not_found", "Directory not found", 404);
    };
    await mount(t, <FilesPanel workspaceId="w1" selected={null} onOpen={() => {}} />);
    expect(screen.getByRole("status").textContent).toContain("This folder is no longer on the host");
    expect(screen.queryByText("Directory not found")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("re-lists the tree and rereads the open file when the workspace's files change", async () => {
    const t = new FakeTransport();
    let files = [{ name: "a.txt", path: "a.txt", kind: "file" }];
    let content = "one\n";
    t.handlers["workspace.tree"] = () => files;
    t.handlers["files.read"] = () => ({
      path: "a.txt",
      content,
      etag: "e",
      mtime_ms: null,
      readonly: false,
      line_ending: "lf",
    });
    await mount(
      t,
      <>
        <FilesPanel workspaceId="w1" selected={null} onOpen={() => {}} />
        <FileView workspaceId="w1" path="a.txt" />
      </>,
    );
    expect(screen.queryByText("b.txt")).toBeNull();
    files = [...files, { name: "b.txt", path: "b.txt", kind: "file" }];
    content = "one\ntwo\n";
    await t.feedEvent("workspace.diff_changed", { workspace_id: "w1" });
    await tick();
    expect(screen.getByText("b.txt")).toBeTruthy();
    const rows = screen.getByTestId("file-content").querySelectorAll("tr");
    expect([...rows].map((r) => r.textContent)).toEqual(["1one", "2two"]);
  });

  it("opens a folder holding only a folder straight through, on one row", async () => {
    const t = new FakeTransport();
    const dirs: Record<string, object[]> = {
      "": [{ name: "dist", path: "dist", kind: "dir" }],
      dist: [{ name: "0.1.0-rc.3", path: "dist/0.1.0-rc.3", kind: "dir" }],
      "dist/0.1.0-rc.3": [{ name: "app.dmg", path: "dist/0.1.0-rc.3/app.dmg", kind: "file" }],
    };
    t.handlers["workspace.tree"] = (p) => dirs[p.path ?? ""];
    await mount(t, <FilesPanel workspaceId="w1" selected={null} onOpen={() => {}} />);
    fireEvent.click(screen.getByText("dist"));
    await tick();
    await tick();
    expect(screen.getByText("dist / 0.1.0-rc.3")).toBeTruthy();
    expect(screen.getByText("app.dmg")).toBeTruthy();
    fireEvent.click(screen.getByText("dist / 0.1.0-rc.3"));
    expect(screen.queryByText("app.dmg")).toBeNull();
    expect(screen.getByText("dist")).toBeTruthy();
  });

  it("does not open through a linked folder", async () => {
    const t = new FakeTransport();
    // `a/up` links back to `a`: opening through it would never end.
    t.handlers["workspace.tree"] = (p) =>
      p.path ? [{ name: "up", path: `${p.path}/up`, kind: "dir", symlink: true }] : [{ name: "a", path: "a", kind: "dir" }];
    await mount(t, <FilesPanel workspaceId="w1" selected={null} onOpen={() => {}} />);
    fireEvent.click(screen.getByText("a"));
    await tick();
    await tick();
    expect(screen.getByText("up")).toBeTruthy();
    expect(t.calls.filter((c) => c.op === "workspace.tree").map((c) => c.params.path ?? "")).toEqual(["", "a"]);
  });

  it("marks files by type", () => {
    // Material Icon Theme's own tables: exact names, then the longest known extension.
    expect(fileIconId("main.tsx", false)).toBe("react_ts");
    expect(fileIconId("lib.RS", false)).toBe("rust");
    expect(fileIconId("index.html", false)).toBe("html");
    expect(fileIconId("src/Button.test.tsx", false)).toBe("test-jsx");
    expect(fileIconId("package.json", false)).toBe("nodejs");
    expect(fileIconId(".gitignore", false)).toBe("git");
    expect(fileIconId("Cargo.lock", false)).toBe("lock");
    expect(fileIconId("notes.zzzq", false)).toBe("file");
    expect(folderIconId("src", false, false)).toBe("folder-src");
    expect(folderIconId("src", true, false)).toBe("folder-src-open");
    expect(folderIconId("misc-stuff", false, false)).toBe("folder");
    // The icon is the pack's image, the same element for the same type.
    expect(fileIcon("a.rs").icon).toBe(fileIcon("b.rs").icon);
    expect(fileIcon("a.rs").icon).not.toBe(fileIcon("a.ts").icon);
  });
});
