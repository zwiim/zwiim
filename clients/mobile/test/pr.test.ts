import { describe, expect, it } from "vitest";
import { ZwiimClientError, type PrState } from "@zwiim/sdk";
import { checksLine, mergeError, prBadge, prErrorText, prLook, shortSha } from "../src/host/pr";

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
    expect(prLook(pr({ state: "merged" }), true)).toMatchObject({ status: "Merged #12", tone: "done", settled: true, canMerge: false, reason: null });
    expect(prLook(pr({ state: "closed" }), false)).toMatchObject({ status: "Closed #12", tone: "mut", settled: true, canMerge: false, reason: null });
  });

  it("keeps Merge off with GitHub's reason, or because the Host is out of reach", () => {
    const blocked = pr({ merge: { allowed: false, status: "blocked", reason: "Waiting for required checks or review", method: "squash", methods: ["squash"] } });
    expect(prLook(blocked, true)).toMatchObject({ canMerge: false, reason: "Waiting for required checks or review" });
    expect(prLook(pr(), false)).toMatchObject({ canMerge: false, reason: "Not connected to the Host" });
    expect(prLook(pr({ merge: { ...pr().merge, method: null, methods: [] } }), true).method).toBeNull();
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
