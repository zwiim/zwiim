import { describe, expect, it } from "vitest";
import { ZwiimClientError, type Compatibility, type PrState, type WorkspaceInfo } from "@zwiim/sdk";
import {
  checksLine,
  continueError,
  continueLine,
  mergeError,
  offersContinue,
  prBadge,
  prErrorText,
  prLabel,
  prLook,
  shortSha,
} from "../src/host/pr";

const pr = (extra: Partial<PrState> = {}): PrState => ({
  number: 12,
  url: "https://github.com/o/r/pull/12",
  title: "Add the thing",
  state: "open",
  base: "main",
  head: "zwiim/a",
  head_sha: "abc123def456",
  merged_at: null,
  checks: { passed: 1, failed: 0, pending: 1, skipped: 0, items: [] },
  merge: { allowed: true, status: "unstable", reason: null, method: "squash", methods: ["squash", "merge"] },
  ...extra,
});
const checks = (c: Partial<PrState["checks"]>) => ({ ...pr().checks, ...c });

describe("pull request look", () => {
  it("says how the checks stand while open, and Merge says what it does", () => {
    expect(prLook(pr(), true)).toEqual({
      status: "Checks running",
      tone: "work",
      checks: "1 running · 1 passed",
      settled: false,
      canMerge: true,
      reason: null,
      method: "Squash and merge",
    });
    expect(prLook(pr({ checks: checks({ pending: 0, passed: 3 }) }), true)).toMatchObject({ status: "Checks passed", tone: "done" });
    expect(prLook(pr({ checks: checks({ failed: 1 }) }), true)).toMatchObject({ status: "Checks failed", tone: "err" });
    expect(prLook(pr({ checks: checks({ pending: 0, passed: 0 }) }), true)).toMatchObject({ status: "Open", checks: "No checks" });
    expect(prLook(pr({ state: "draft" }), true)).toMatchObject({ status: "Draft", tone: "mut" });
  });

  it("is settled once merged or closed: nothing to merge, no reason", () => {
    expect(prLook(pr({ state: "merged" }), true)).toMatchObject({ status: "Merged", tone: "done", settled: true, canMerge: false, reason: null });
    expect(prLook(pr({ state: "closed" }), false)).toMatchObject({ status: "Closed", tone: "mut", settled: true, canMerge: false, reason: null });
  });

  it("keeps Merge off with GitHub's reason, or because the Host is out of reach", () => {
    const blocked = pr({ merge: { allowed: false, status: "blocked", reason: "Waiting for required checks or review", method: "squash", methods: ["squash"] } });
    expect(prLook(blocked, true)).toMatchObject({ canMerge: false, reason: "Waiting for required checks or review" });
    expect(prLook(pr(), false)).toMatchObject({ canMerge: false, reason: "Not connected to the Host" });
    expect(prLook(pr({ merge: { ...pr().merge, method: null, methods: [] } }), true).method).toBeNull();
  });

  it("reads the strip aloud without stale counts once merged", () => {
    const running = pr({ checks: checks({ pending: 2, passed: 0 }) });
    expect(prLabel(12, prLook(running, true))).toBe("Pull request #12, Checks running. 2 running");
    expect(prLabel(12, prLook({ ...running, state: "merged" }, true))).toBe("Pull request #12, Merged");
    expect(prLabel(12, prLook({ ...running, state: "closed" }, true))).toBe("Pull request #12, Closed");
  });

  it("counts checks failed first", () => {
    expect(checksLine(pr({ checks: checks({ passed: 2, failed: 1, pending: 1, skipped: 1 }) }))).toBe(
      "1 failed · 1 running · 2 passed · 1 skipped",
    );
  });

  it("badges the branch chip with the PR number in its tone", () => {
    expect(prBadge(undefined)).toBeNull();
    expect(prBadge({ pr: null, error: { code: "gh_unavailable", message: "gh missing" } })).toBeNull();
    expect(prBadge({ pr: pr(), error: null })).toEqual({ text: "#12", tone: "work" });
    expect(prBadge({ pr: pr({ state: "merged" }), error: null })).toEqual({ text: "#12", tone: "done" });
  });

  it("names the head as people read it", () => {
    expect(shortSha("abc123def456")).toBe("abc123d");
  });
});

describe("pull request errors in words", () => {
  it("says what to do about the Host's GitHub CLI", () => {
    expect(prErrorText("gh_unauthenticated", "x", "Studio Mac")).toBe(
      "GitHub CLI on Studio Mac isn't signed in. Run gh auth login there to follow pull requests.",
    );
    expect(prErrorText("gh_unavailable", "x", "Studio Mac")).toBe(
      "GitHub CLI isn't installed on Studio Mac. Install gh there to follow pull requests.",
    );
    expect(prErrorText("github_unreachable", "GitHub didn't answer", "Studio Mac")).toBe("GitHub didn't answer");
  });

  it("sends a signed-out GitHub CLI to the Host tab when the Host signs in itself", () => {
    const signIn =
      "GitHub CLI on Studio Mac isn't signed in. Sign in under GitHub in the Host tab to follow pull requests.";
    expect(prErrorText("gh_unauthenticated", "x", "Studio Mac", true)).toBe(signIn);
    const err = new ZwiimClientError("gh_unauthenticated", "m", 422);
    expect(mergeError(err, "Studio Mac", true)).toBe(signIn);
    expect(continueError(err, 12, "Studio Mac", true)).toBe(signIn);
    // An older Host still asks for gh auth login.
    expect(mergeError(err, "Studio Mac")).toContain("Run gh auth login there");
  });

  it("explains a refused merge and what comes next", () => {
    const err = (code: string, details?: Record<string, unknown>) => new ZwiimClientError(code as never, "m", 409, details);
    expect(mergeError(err("pr_changed"), "Studio Mac")).toBe(
      "The pull request changed since you looked. Check it again before merging.",
    );
    expect(mergeError(err("not_mergeable", { reason: "Merge conflicts" }), "Studio Mac")).toBe("Merge conflicts");
    expect(mergeError(err("not_mergeable"), "Studio Mac")).toBe("GitHub won't merge it right now.");
    expect(mergeError(err("gh_unauthenticated"), "Studio Mac")).toContain("isn't signed in");
    expect(mergeError(err("github_unreachable"), "Studio Mac")).toBe("Studio Mac couldn't reach GitHub. Try again.");
    expect(mergeError(err("unknown_outcome"), "Studio Mac")).toBe(
      "Not sure that went through. Check the PR on Studio Mac before trying again.",
    );
    expect(mergeError(err("network"), "Studio Mac")).toBe("Not connected to Studio Mac. Try again when it's back.");
    expect(mergeError(new Error("boom"), "Studio Mac")).toBe("Couldn't merge the pull request. Try again.");
  });
});

describe("continuing after a merge", () => {
  const worktree = (extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
    workspace_id: "w",
    name: "w",
    alias: null,
    kind: "worktree",
    parent_workspace_id: "r",
    is_git_repo: true,
    branch: "zwiim/w",
    base_branch: "main",
    base_commit: null,
    removed: false,
    display_path: null,
    ...extra,
  });
  const serving = (capabilities: string[]): Compatibility => ({ status: "compatible", hostRevision: 1, capabilities });
  const host = serving(["git.pr", "workspace.continue"]);
  const merged = pr({ state: "merged" });

  it("offers Continue only on a merged PR in a live worktree whose Host serves it", () => {
    expect(offersContinue(merged, worktree(), host)).toBe(true);
    expect(offersContinue(pr(), worktree(), host)).toBe(false);
    expect(offersContinue(pr({ state: "closed" }), worktree(), host)).toBe(false);
    expect(offersContinue(merged, worktree({ kind: "root", parent_workspace_id: null }), host)).toBe(false);
    expect(offersContinue(merged, worktree({ removed: true }), host)).toBe(false);
    expect(offersContinue(merged, undefined, host)).toBe(false);
    expect(offersContinue(merged, worktree(), serving(["git.pr"]))).toBe(false);
    expect(offersContinue(merged, worktree(), null)).toBe(false);
  });

  it("says where the new branch starts, and what stays", () => {
    expect(continueLine(merged)).toBe(
      "Start a new branch from main, where #12 landed. Sessions stay; each agent is told on its next turn. The old branch stays.",
    );
  });

  it("says what holds the worktree, or what GitHub says", () => {
    const err = (code: string) => new ZwiimClientError(code as never, "m", 409);
    expect(continueError(err("workspace_busy"), 12, "Studio Mac")).toBe(
      "An agent is still working here. Wait for it or stop it, then Continue.",
    );
    expect(continueError(err("workspace_dirty"), 12, "Studio Mac")).toBe(
      "Uncommitted changes here. Commit or discard them first.",
    );
    expect(continueError(err("head_not_merged"), 12, "Studio Mac")).toBe(
      "This branch has commits #12 didn't merge. Continue would leave them behind.",
    );
    expect(continueError(err("pr_not_merged"), 12, "Studio Mac")).toBe("#12 isn't merged on GitHub yet.");
    expect(continueError(err("github_unreachable"), 12, "Studio Mac")).toBe("Studio Mac couldn't reach GitHub. Try again.");
    expect(continueError(err("gh_unauthenticated"), 12, "Studio Mac")).toContain("isn't signed in");
    expect(continueError(err("unknown_outcome"), 12, "Studio Mac")).toBe(
      "Not sure that went through. Check the branch on Studio Mac before trying again.",
    );
    expect(continueError(err("network"), 12, "Studio Mac")).toBe("Not connected to Studio Mac. Try again when it's back.");
    expect(continueError(new Error("boom"), 12, "Studio Mac")).toBe("Couldn't continue. Try again.");
  });
});
