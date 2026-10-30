import { buildWorkspaceTree, ZwiimClientError, type TerminalMeta, type WorkspaceInfo } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  contextLabel,
  contextText,
  hasWorktreeLevel,
  openTile,
  workspaceCounts,
  worktreeBase,
  terminalsIn,
  terminalSummary,
  worktreeChoices,
  addedWithoutWorktree,
  worktreeError,
  workspaceRoots,
} from "../src/host/workspace";

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
const wt = (id: string, root: string, over: Partial<WorkspaceInfo> = {}) =>
  ws(id, { kind: "worktree", parent_workspace_id: root, branch: `zwiim/${id}`, base_branch: "main", ...over });

const LIST = [
  ws("zwiim", { display_path: "~/Projects/zwiim" }),
  ws("api", { alias: "my-api", display_path: "~/Work/api", branch: null }),
  wt("fix", "zwiim", { branch: "fix-auth" }),
  wt("old", "zwiim", { removed: true, branch: null, alias: "zwiim/old" }),
  wt("mobile", "zwiim"),
  ws("notes", { is_git_repo: false, branch: null, display_path: "~/notes" }),
];
const tree = () => buildWorkspaceTree({ workspaces: LIST, sessionsByWorkspace: {}, sessions: {} });

describe("where something lives", () => {
  it("names a root by its branch, or root without one", () => {
    expect(contextLabel(LIST, "zwiim")).toEqual({ workspace: "zwiim", worktree: "main" });
    expect(contextLabel(LIST, "api")).toEqual({ workspace: "my-api", worktree: "root" });
  });

  it("names a worktree by its root and its branch", () => {
    expect(contextLabel(LIST, "fix")).toEqual({ workspace: "zwiim", worktree: "fix-auth" });
    expect(contextText(contextLabel(LIST, "fix"))).toBe("zwiim · fix-auth");
    // A removed worktree has no branch: its name stands in.
    expect(contextLabel(LIST, "old")).toEqual({ workspace: "zwiim", worktree: "zwiim/old" });
  });

  it("names a plain folder by itself: it has no worktree", () => {
    expect(contextLabel(LIST, "notes")).toEqual({ workspace: "notes", worktree: null });
    expect(contextText(contextLabel(LIST, "notes"))).toBe("notes");
  });

  it("is unknown for a workspace the host doesn't list", () => {
    expect(contextLabel(LIST, "missing")).toBeNull();
    expect(contextLabel(null, "zwiim")).toBeNull();
    expect(contextText(null)).toBeNull();
  });
});

describe("the workspace picker", () => {
  it("lists roots only at the first level", () => {
    expect(workspaceRoots(tree(), "").map((n) => n.workspace.workspace_id)).toEqual(["zwiim", "api", "notes"]);
  });

  it("opens a workspace's worktrees, or straight into a new one when it has none", () => {
    const node = (id: string) => tree().find((n) => n.workspace.workspace_id === id)!;
    expect(hasWorktreeLevel(node("zwiim"))).toBe(true);
    // No worktree yet, repository or plain folder: tapping it makes one, as Desktop's click does.
    expect(hasWorktreeLevel(node("api"))).toBe(false);
    expect(hasWorktreeLevel(node("notes"))).toBe(false);
  });

  it("searches names, aliases and home paths", () => {
    const ids = (q: string) => workspaceRoots(tree(), q).map((n) => n.workspace.workspace_id);
    expect(ids("ZW")).toEqual(["zwiim"]);
    expect(ids("my-")).toEqual(["api"]);
    expect(ids("~/work")).toEqual(["api"]);
    expect(ids("  projects ")).toEqual(["zwiim"]);
    expect(ids("nothing")).toEqual([]);
  });

  it("lists a root's live worktrees, never the root: the Host runs nothing there", () => {
    expect(worktreeChoices(tree(), "zwiim").map((n) => n.workspace.workspace_id)).toEqual(["fix", "mobile"]);
    expect(worktreeChoices(tree(), "api")).toEqual([]);
    expect(worktreeChoices(tree(), "missing")).toEqual([]);
  });
});

describe("the workspace tree", () => {
  it("opens on the workspace holding the choice, else the first", () => {
    expect(openTile(tree(), "fix")).toBe("zwiim");
    expect(openTile(tree(), "notes")).toBe("notes");
    expect(openTile(tree(), "api")).toBe("api");
    expect(openTile(tree(), "old")).toBe("zwiim");
    expect(openTile(tree(), null)).toBe("zwiim");
    expect(openTile([], null)).toBeNull();
  });

  it("says what a workspace holds and where a worktree came from", () => {
    const [zwiim, api, notes] = workspaceRoots(tree(), "");
    expect(workspaceCounts(zwiim!)).toEqual(["2 worktrees"]);
    expect(workspaceCounts(api!)).toEqual([]);
    expect(workspaceCounts(notes!)).toEqual(["no Git"]);
    expect(worktreeBase(wt("x", "zwiim", { base_commit: "1a2b3c4d5e" }))).toBe("main@1a2b3c4");
    expect(worktreeBase(ws("zwiim"))).toBeNull();
  });
});

describe("new worktree errors", () => {
  const root = ws("zwiim");
  it("names a folder that isn't a repository", () => {
    expect(worktreeError(new ZwiimClientError("not_a_git_repository", "x"), root, "Studio Mac")).toBe(
      "zwiim isn't a Git repository, so it can't have worktrees.",
    );
  });
  it("says a repository with no commits has nothing to branch from", () => {
    expect(worktreeError(new ZwiimClientError("invalid_input", "The root workspace has no commits yet"), root, "Studio Mac")).toBe(
      "zwiim has no commits yet. Commit once on Studio Mac, then try again.",
    );
  });
  it("says when the host is away", () => {
    expect(worktreeError(new ZwiimClientError("network", "x"), root, "Studio Mac")).toBe(
      "Not connected to Studio Mac. Try again when it's back.",
    );
  });
  it("falls back to trying again", () => {
    expect(worktreeError(new Error("x"), root, "Studio Mac")).toBe("Couldn't create a worktree. Try again.");
  });
  it("says a folder was added even when its first worktree wasn't, and where to retry", () => {
    expect(addedWithoutWorktree(new Error("x"), root, "Studio Mac")).toBe(
      "Added zwiim, but couldn't create a worktree in it. Try New worktree in Files.",
    );
    expect(addedWithoutWorktree(new ZwiimClientError("network", "x"), root, "Studio Mac")).toBe(
      "Added zwiim. Not connected to Studio Mac. Try again when it's back.",
    );
  });
});

describe("terminals by worktree", () => {
  const term = (id: string, workspace_id: string) => ({ terminal_id: id, workspace_id, title: id, alive: true }) as TerminalMeta;
  const terminals = { a: term("a", "zwiim"), b: term("b", "fix"), c: term("c", "zwiim") };
  it("lists only the selected worktree's, and the others again on switching back", () => {
    expect(terminalsIn(terminals, "fix").map((t) => t.terminal_id)).toEqual(["b"]);
    expect(terminalsIn(terminals, "zwiim").map((t) => t.terminal_id)).toEqual(["a", "c"]);
    expect(terminalsIn(terminals, "mobile")).toEqual([]);
  });

  it("sums them up for the dock", () => {
    const dev = { ...term("d", "fix"), title: "dev" };
    const tests = { ...term("t", "fix"), title: "tests", alive: false };
    expect(terminalSummary([dev, tests], "fix-auth")).toBe("dev running · tests exited");
    expect(terminalSummary([], "fix-auth")).toBe("none in fix-auth");
  });
});
