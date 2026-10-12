import { ZwiimClientError, type CloneJob, type GithubRepo } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import { addedWorkspace, cloneCards, cloneProblem, listProblem, repoMeta, repoRows, settleClones } from "../src/host/clone";

const job = (over: Partial<CloneJob> = {}): CloneJob => ({
  clone_id: "c1",
  owner: "acme",
  name: "web",
  state: "cloning",
  phase: "receiving",
  percent: 10,
  workspace_id: null,
  worktree_id: null,
  error: null,
  ...over,
});

const repo = (over: Partial<GithubRepo> = {}): GithubRepo => ({
  owner: "acme",
  name: "web",
  private: false,
  fork: false,
  archived: false,
  description: null,
  pushed_at: null,
  size_kb: 0,
  default_branch: "main",
  workspace_id: null,
  ...over,
});

const failed = (code: string, message = "/Users/me/zwiim/repos/acme/web: boom") =>
  job({ state: "failed", phase: null, percent: null, error: { code, message } });

describe("listProblem", () => {
  it("says what to do about gh on the Host", () => {
    expect(listProblem(new ZwiimClientError("gh_unavailable", "x", 422), "Studio Mac")).toBe(
      "GitHub CLI isn't installed on Studio Mac. Install gh there to clone projects.",
    );
    expect(listProblem(new ZwiimClientError("gh_unauthenticated", "x", 422), "Studio Mac")).toBe(
      "GitHub CLI on Studio Mac isn't signed in. Run gh auth login there.",
    );
    expect(listProblem(new ZwiimClientError("auth_failed", "x", 401), "Studio Mac")).toBe(
      "GitHub refused the credentials. Run gh auth status on Studio Mac.",
    );
  });

  it("blames GitHub for the Host's network failures and the phone for its own", () => {
    const unreachable = "Studio Mac couldn't reach GitHub.";
    expect(listProblem(new ZwiimClientError("github_unreachable", "x", 502), "Studio Mac")).toBe(unreachable);
    expect(listProblem(new ZwiimClientError("timeout", "x"), "Studio Mac")).toBe(unreachable);
    // From the Host (it has a status): gh couldn't reach GitHub.
    expect(listProblem(new ZwiimClientError("network", "x", 502), "Studio Mac")).toBe(unreachable);
    // From the phone (no status): there is no connection to the Host.
    expect(listProblem(new ZwiimClientError("network", "Not connected to the host"), "Studio Mac")).toBe(
      "Not connected to Studio Mac. Try again when it's back.",
    );
  });

  it("shows any other error's message", () => {
    expect(listProblem(new ZwiimClientError("internal", "Something broke", 500), "Studio Mac")).toBe("Something broke");
    expect(listProblem(new Error("plain"), "Studio Mac")).toBe("plain");
  });
});

describe("cloneProblem", () => {
  it("offers Retry only where trying again can help, and never shows the Host's message", () => {
    expect(cloneProblem(failed("network"), "Studio Mac")).toEqual({ text: "Couldn't reach GitHub.", retry: true });
    expect(cloneProblem(failed("timeout"), "Studio Mac")).toEqual({ text: "The clone stopped making progress.", retry: true });
    expect(cloneProblem(failed("not_found"), "Studio Mac")).toEqual({
      text: "acme/web was not found, or this account can't see it.",
      retry: false,
    });
    expect(cloneProblem(failed("gh_unauthenticated"), "Studio Mac")).toEqual({
      text: "GitHub CLI on Studio Mac isn't signed in. Run gh auth login there.",
      retry: false,
    });
    expect(cloneProblem(failed("gh_unavailable"), "Studio Mac").retry).toBe(false);
    expect(cloneProblem(failed("auth_failed"), "Studio Mac")).toEqual({
      text: "GitHub refused the credentials. Run gh auth status on Studio Mac.",
      retry: false,
    });
    expect(cloneProblem(failed("failed"), "Studio Mac")).toEqual({ text: "The clone failed.", retry: true });
    expect(cloneProblem(failed("disk_full"), "Studio Mac")).toEqual({ text: "The clone failed.", retry: true });
    expect(cloneProblem(job({ state: "failed", error: null }), "Studio Mac")).toEqual({ text: "The clone failed.", retry: true });
  });
});

describe("repoRows", () => {
  const loaded = [repo(), repo({ name: "api", description: "Billing service" }), repo({ owner: "other", name: "site" })];

  it("filters loaded repositories by name and description", () => {
    expect(repoRows(loaded, "", []).map((r) => r.kind === "repo" && r.repo.name)).toEqual(["web", "api", "site"]);
    expect(repoRows(loaded, "billing", []).map((r) => r.kind === "repo" && r.repo.name)).toEqual(["api"]);
    expect(repoRows(loaded, "nothing", [])).toEqual([]);
  });

  it("leads with a paste row for a repository that is not loaded", () => {
    const rows = repoRows(loaded, "https://github.com/someone/else", [job({ owner: "someone", name: "else" })]);
    expect(rows[0]).toEqual({ kind: "paste", ref: { owner: "someone", name: "else" }, job: job({ owner: "someone", name: "else" }) });
    expect(rows).toHaveLength(1);
  });

  it("adds no paste row for a loaded repository, whatever its letter case", () => {
    const rows = repoRows(loaded, "ACME/Web", []);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ kind: "repo", repo: { owner: "acme", name: "web" } });
  });

  it("adds no paste row for text that is not a repository", () => {
    expect(repoRows(loaded, "acme/web/extra", []).some((r) => r.kind === "paste")).toBe(false);
  });

  it("gives each row its clone job", () => {
    const active = job({ clone_id: "a" });
    const rows = repoRows(loaded, "", [active]);
    expect(rows.map((r) => r.job)).toEqual([active, null, null]);
  });
});

describe("addedWorkspace", () => {
  it("is the repository's workspace, or a done clone's when the page is stale", () => {
    expect(addedWorkspace({ kind: "repo", repo: repo({ workspace_id: "root" }), job: null })).toBe("root");
    // The Host caches pages: a repository cloned a moment ago still reads workspace_id null.
    expect(addedWorkspace({ kind: "repo", repo: repo(), job: job({ state: "done", workspace_id: "root", worktree_id: "tree" }) })).toBe("root");
    expect(addedWorkspace({ kind: "paste", ref: { owner: "a", name: "b" }, job: job({ state: "done", workspace_id: "root" }) })).toBe("root");
    expect(addedWorkspace({ kind: "repo", repo: repo(), job: job() })).toBeNull();
    expect(addedWorkspace({ kind: "repo", repo: repo(), job: null })).toBeNull();
  });
});

describe("repoMeta", () => {
  const now = Math.floor(Date.parse("2026-10-12T12:00:00Z") / 1000);

  it("says when it was pushed and how big it is", () => {
    expect(repoMeta(repo({ pushed_at: "2026-10-12T10:00:00Z", size_kb: 4198 }), now)).toBe("pushed 2h ago · 4.1 MB");
    expect(repoMeta(repo({ pushed_at: "2026-10-12T11:59:30Z", size_kb: 512 }), now)).toBe("pushed just now · 512 KB");
    expect(repoMeta(repo({ size_kb: 3 * 1024 * 1024 }), now)).toBe("3.0 GB");
  });

  it("leaves out what it doesn't know", () => {
    expect(repoMeta(repo({ pushed_at: "2026-10-10T12:00:00Z" }), now)).toBe("pushed 2d ago");
    expect(repoMeta(repo({ pushed_at: "not a date", size_kb: 0 }), now)).toBe("");
    expect(repoMeta(repo(), now)).toBe("");
  });
});

describe("cloneCards", () => {
  it("shows active clones and failures not dismissed on this phone", () => {
    const clones = {
      a: job({ clone_id: "a" }),
      f: job({ clone_id: "f", state: "failed" }),
      g: job({ clone_id: "g", state: "failed" }),
      d: job({ clone_id: "d", state: "done" }),
      c: job({ clone_id: "c", state: "cancelled" }),
    };
    expect(cloneCards(clones, new Set(["g"])).map((j) => j.clone_id)).toEqual(["a", "f"]);
  });
});

describe("settleClones", () => {
  it("hands back done clones, keeps the unfinished, forgets the failed and cancelled", () => {
    const clones = {
      a: job({ clone_id: "a" }),
      d: job({ clone_id: "d", state: "done", workspace_id: "root", worktree_id: "tree" }),
      f: job({ clone_id: "f", state: "failed" }),
      c: job({ clone_id: "c", state: "cancelled" }),
    };
    expect(settleClones(clones, ["a", "d", "f", "c", "missing"])).toEqual({ done: [clones.d], left: ["a", "missing"] });
    expect(settleClones({}, [])).toEqual({ done: [], left: [] });
  });
});
