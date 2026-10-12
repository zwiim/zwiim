import { describe, expect, it } from "vitest";
import { fullName, isActive, jobFor, progressLabel, repoRef, sameRepo } from "../src/state/clones";
import type { CloneJob } from "../src/types/protocol";

const job = (over: Partial<CloneJob> = {}): CloneJob => ({
  clone_id: "c1",
  owner: "acme",
  name: "web",
  state: "cloning",
  phase: "receiving",
  percent: 42,
  workspace_id: null,
  worktree_id: null,
  error: null,
  ...over,
});

describe("repoRef", () => {
  it("reads owner/name, GitHub URLs and SSH remotes", () => {
    expect(repoRef("acme/web")).toEqual({ owner: "acme", name: "web" });
    expect(repoRef("  acme/web/  ")).toEqual({ owner: "acme", name: "web" });
    expect(repoRef("https://github.com/acme/web")).toEqual({ owner: "acme", name: "web" });
    expect(repoRef("https://github.com/ACME/Web.git")).toEqual({ owner: "ACME", name: "Web" });
    expect(repoRef("github.com/acme/web/tree/main/src")).toEqual({ owner: "acme", name: "web" });
    expect(repoRef("https://www.github.com/acme/web?tab=readme")).toEqual({ owner: "acme", name: "web" });
    expect(repoRef("git@github.com:acme/web.git")).toEqual({ owner: "acme", name: "web" });
    expect(repoRef("acme/web.js")).toEqual({ owner: "acme", name: "web.js" });
  });

  it("refuses anything that is not one repository", () => {
    for (const bad of ["", "web", "acme/web/extra", "-acme/web", "acme/..", "acme/we b", "https://gitlab.com/acme/web", "a".repeat(40) + "/web"]) {
      expect(repoRef(bad)).toBeNull();
    }
  });
});

describe("clone jobs", () => {
  it("names and compares repositories ignoring case", () => {
    expect(fullName({ owner: "acme", name: "web" })).toBe("acme/web");
    expect(sameRepo({ owner: "ACME", name: "Web" }, { owner: "acme", name: "web" })).toBe(true);
    expect(sameRepo({ owner: "acme", name: "web" }, { owner: "acme", name: "api" })).toBe(false);
  });

  it("counts queued, cloning and adding as active", () => {
    expect(["queued", "cloning", "adding"].map((state) => isActive(job({ state: state as CloneJob["state"] })))).toEqual([true, true, true]);
    expect(["done", "failed", "cancelled"].map((state) => isActive(job({ state: state as CloneJob["state"] })))).toEqual([false, false, false]);
  });

  it("picks a repository's active job, else its failure, else its done job", () => {
    const done = job({ clone_id: "d", state: "done" });
    const failed = job({ clone_id: "f", state: "failed" });
    const active = job({ clone_id: "a", owner: "ACME" });
    expect(jobFor([done, failed, active], { owner: "acme", name: "web" })).toBe(active);
    expect(jobFor([done, failed], { owner: "acme", name: "web" })).toBe(failed);
    expect(jobFor([done], { owner: "acme", name: "web" })).toBe(done);
    expect(jobFor([job({ state: "cancelled" })], { owner: "acme", name: "web" })).toBeNull();
    expect(jobFor([active], { owner: "acme", name: "api" })).toBeNull();
  });

  it("labels each stage of a clone", () => {
    expect(progressLabel(job({ state: "queued" }))).toBe("Waiting to clone…");
    expect(progressLabel(job({ state: "adding" }))).toBe("Opening…");
    expect(progressLabel(job())).toBe("Receiving objects 42%");
    expect(progressLabel(job({ phase: "resolving", percent: null }))).toBe("Resolving deltas");
    expect(progressLabel(job({ phase: "checkout", percent: 7 }))).toBe("Checking out files 7%");
    expect(progressLabel(job({ phase: null }))).toBe("Cloning…");
  });
});
