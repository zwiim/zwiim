import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, ZwiimClientError, type WorkspaceInfo } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { DiffPanel, numberPatch } from "@/panels/DiffPanel";
import { FilesPanel, FileView } from "@/panels/FilesPanel";
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
});
