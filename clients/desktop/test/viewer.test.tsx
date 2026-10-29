import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, ZwiimClientError, type DiffFile, type WorkspaceInfo } from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { rememberMarkdownStyle } from "@/lib/prefs";
import { DiffView } from "@/panels/DiffPanel";
import { FakeTransport, tick } from "./fakes";

const root: WorkspaceInfo = {
  workspace_id: "root",
  name: "repo",
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
};

const thirty = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n") + "\n";

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

/** A host whose one file has `diff` (null: no changes) and reads as `content` (null: gone). */
function host(diff: { file: DiffFile; patch?: string } | null, content: string | null | Error) {
  const t = new FakeTransport();
  t.handlers["git.diff"] = () => {
    if (!diff) throw new ZwiimClientError("not_found", "File has no changes", 404);
    return { base_commit: null, truncated: false, files: [diff.file], ...(diff.patch ? { patch: diff.patch } : {}) };
  };
  t.handlers["files.read"] = (p) => {
    if (content instanceof Error) throw content;
    if (content === null) throw new ZwiimClientError("not_found", "No such file", 404);
    return { path: p.path, content, etag: "e", mtime_ms: 0, readonly: false, line_ending: "lf" };
  };
  return t;
}

async function mount(t: FakeTransport, path: string) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <DiffView workspace={root} path={path} />
    </HostProvider>,
  );
  await tick();
}

const body = () => screen.getByTestId("diff-patch");

beforeEach(() => window.localStorage.clear());
afterEach(cleanup);

describe("file viewer", () => {
  it("shows the whole file with unchanged stretches folded, and opens a fold", async () => {
    await mount(host({ file: changed("a.txt"), patch: "@@ -15 +15 @@\n-old 15\n+line 15\n" }, thirty), "a.txt");
    const folds = screen.getAllByTestId("fold");
    expect(folds.map((f) => f.textContent)).toEqual(["11 unmodified lines", "12 unmodified lines"]);
    expect(body().textContent).toContain("-old 15");
    expect(body().textContent).toContain("+line 15");
    expect(body().textContent).not.toContain("line 1 ");
    expect(body().textContent).toContain("line 12");
    expect(body().textContent).not.toContain("line 11");

    fireEvent.click(folds[0]!);
    expect(screen.getAllByTestId("fold")).toHaveLength(1);
    expect(body().textContent).toContain("line 11");
  });

  it("switches to Split and remembers it", async () => {
    await mount(host({ file: changed("a.txt"), patch: "@@ -15 +15 @@\n-old 15\n+line 15\n" }, thirty), "a.txt");
    expect(screen.getByRole("button", { name: /unified/i }).getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: /split/i }));
    expect(screen.getByRole("button", { name: /split/i }).getAttribute("aria-pressed")).toBe("true");
    // Old and new share one row.
    const row = [...body().querySelectorAll("tr")].find((r) => r.textContent?.includes("old 15"));
    expect(row?.textContent).toContain("line 15");
    expect(window.localStorage.getItem("zwiim.desktop.diff_layout")).toBe('"split"');

    cleanup();
    await mount(host({ file: changed("a.txt"), patch: "@@ -15 +15 @@\n-old 15\n+line 15\n" }, thirty), "a.txt");
    expect(screen.getByRole("button", { name: /split/i }).getAttribute("aria-pressed")).toBe("true");
  });

  it("copies the whole current file", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await mount(host({ file: changed("a.txt"), patch: "@@ -15 +15 @@\n-old 15\n+line 15\n" }, thirty), "a.txt");
    fireEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(thirty);
    expect(screen.getByRole("button", { name: "Copied" })).toBeTruthy();
  });

  it("previews Markdown, and only Markdown", async () => {
    await mount(host({ file: changed("README.md"), patch: "@@ -1 +1 @@\n-# Old\n+# Title\n" }, "# Title\n"), "README.md");
    fireEvent.click(screen.getByRole("button", { name: /preview/i }));
    expect(screen.getByRole("heading", { name: "Title" })).toBeTruthy();
    expect(screen.queryByTestId("file-changes")).toBeNull();

    cleanup();
    await mount(host({ file: changed("a.txt"), patch: "@@ -15 +15 @@\n-old 15\n+line 15\n" }, thirty), "a.txt");
    expect(screen.queryByRole("button", { name: /preview/i })).toBeNull();
  });

  it("sets a Markdown preview as a book page once that style is picked", async () => {
    await mount(host({ file: changed("README.md"), patch: "@@ -1 +1 @@\n-# Old\n+# Title\n" }, "# Title\n"), "README.md");
    fireEvent.click(screen.getByRole("button", { name: /preview/i }));
    expect(screen.getByTestId("markdown").className).not.toContain("markdown-book");
    act(() => rememberMarkdownStyle("book"));
    expect(screen.getByTestId("markdown").className).toContain("markdown-book");
  });

  it("says a file too large to open is, with nothing to copy", async () => {
    await mount(host({ file: changed("big.log", { too_large: true, additions: null, deletions: null }) }, "x"), "big.log");
    expect(body().textContent).toBe("File is too large to open");
    expect((screen.getByRole("button", { name: "Copy" }) as HTMLButtonElement).disabled).toBe(true);

    cleanup();
    const refused = new ZwiimClientError("invalid_input", "File is too large to open in editor", 400);
    await mount(host(null, refused), "big.log");
    expect(body().textContent).toBe("File is too large to open");
  });

  it("shows a deleted file's old text, all removed, with nothing to copy or preview", async () => {
    await mount(
      host({ file: changed("gone.md", { change: "deleted", additions: 0, deletions: 2 }), patch: "@@ -1,2 +0,0 @@\n-a\n-b\n" }, null),
      "gone.md",
    );
    const rows = [...body().querySelectorAll("tr[data-kind]")];
    expect(rows.map((r) => r.getAttribute("data-kind"))).toEqual(["del", "del"]);
    expect((screen.getByRole("button", { name: "Copy" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole("button", { name: /preview/i })).toBeNull();
  });

  it("shows a file with no changes as it is now, and says when it is gone", async () => {
    await mount(host(null, "same\n"), "a.txt");
    expect(body().textContent).toContain("No uncommitted changes — showing the file as it is now.");
    expect(body().textContent).toContain("same");
    expect(screen.queryByTestId("fold")).toBeNull();

    cleanup();
    await mount(host(null, null), "a.txt");
    expect(body().textContent).toBe("This file is gone.");
  });
});
