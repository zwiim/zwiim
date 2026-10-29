import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  ZwiimClientError,
  type ConflictCheck,
  type PrState,
  type SessionSummary,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { openLink } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberAutoMergeButton } from "@/lib/prefs";
import { PrBanner } from "@/panels/PrBanner";
import { FakeTransport, hostHello, tick } from "./fakes";

vi.mock("@/lib/bridge", async (actual) => ({
  ...(await actual<typeof import("@/lib/bridge")>()),
  openLink: vi.fn(async () => {}),
}));

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  window.localStorage.clear();
  vi.mocked(openLink).mockClear();
});

const pr = (extra: Partial<PrState> = {}): PrState => ({
  number: 12,
  url: "https://github.com/o/r/pull/12",
  title: "Add the thing",
  state: "open",
  base: "main",
  head: "zwiim/a",
  head_sha: "abc123",
  merged_at: null,
  checks: {
    passed: 1,
    failed: 0,
    pending: 1,
    skipped: 0,
    items: [
      { name: "lint", workflow: "CI", state: "passed", link: "https://github.com/o/r/actions/runs/1" },
      { name: "test", workflow: "CI", state: "pending", link: "https://github.com/o/r/actions/runs/2" },
    ],
  },
  merge: { allowed: true, status: "unstable", reason: null, method: "squash", methods: ["squash", "merge"] },
  ...extra,
});

async function mount(t: FakeTransport, sessionId?: string) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <PrBanner workspaceId="a" sessionId={sessionId} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

const merge = () => screen.getByRole("button", { name: /Merge/ }) as HTMLButtonElement;
const continueButton = () => screen.queryByRole("button", { name: /Continue/ }) as HTMLButtonElement | null;
const archiveButton = () => screen.queryByRole("button", { name: "Archive" }) as HTMLButtonElement | null;

const worktree = (extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  workspace_id: "a",
  name: "repo-a",
  alias: null,
  kind: "worktree",
  parent_workspace_id: "root",
  branch: "zwiim/a",
  base_branch: "main",
  base_commit: "0123456",
  removed: false,
  is_git_repo: true,
  display_path: null,
  ...extra,
});

/** A Host whose workspace "a" has a merged pull request. */
function mergedHost(workspace: WorkspaceInfo = worktree()) {
  const t = new FakeTransport();
  t.handlers["workspace.list"] = () => [workspace];
  const snapshot = t.snapshot.bind(t);
  t.snapshot = () => ({
    ...snapshot(),
    prs: { a: { pr: pr({ state: "merged", merged_at: "2026-10-04T00:00:00Z" }), error: null } },
  });
  return t;
}

describe("pull request banner", () => {
  it("follows the Host's PR from running checks to Merged, and never asks GitHub itself", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "setInterval"] });
    const t = new FakeTransport();
    const snapshot = t.snapshot.bind(t);
    t.snapshot = () => ({ ...snapshot(), prs: { a: { pr: pr(), error: null } } });
    t.handlers["git.pr_merge"] = () => ({ number: 12, method: "squash", merged_at: null });
    await mount(t);

    expect(screen.getByTestId("pr-banner").textContent).toContain("#12 Add the thing");
    expect(screen.getByTestId("pr-status").textContent).toBe("Checks running");
    expect(merge().disabled).toBe(false);
    expect(merge().title).toBe("Squash and merge");

    // Each check opens its run on GitHub; so does the PR.
    fireEvent.click(screen.getByRole("button", { name: /1 running · 1 passed/ }));
    fireEvent.click(screen.getByRole("button", { name: /CI \/ test/ }));
    expect(openLink).toHaveBeenLastCalledWith("https://github.com/o/r/actions/runs/2");
    fireEvent.click(screen.getByRole("button", { name: /Add the thing/ }));
    expect(openLink).toHaveBeenLastCalledWith("https://github.com/o/r/pull/12");

    const passed = pr({ checks: { ...pr().checks, passed: 2, pending: 0 }, merge: { ...pr().merge, status: "clean" } });
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: passed, error: null });
    expect(screen.getByTestId("pr-status").textContent).toBe("Checks passed");

    fireEvent.click(merge());
    await act(async () => {
      await Promise.resolve();
    });
    // Pinned to the head shown, with the method the tooltip named.
    expect(t.calls.find((c) => c.op === "git.pr_merge")!.params).toEqual({
      workspace_id: "a",
      number: 12,
      head_sha: "abc123",
      method: "squash",
    });
    // Merged as soon as the Host says so, before the feed catches up.
    expect(screen.getByTestId("pr-status").textContent).toBe("Merged");
    await t.feedEvent("workspace.pr_changed", {
      workspace_id: "a",
      pr: pr({ state: "merged", merged_at: "2026-10-03T00:00:00Z" }),
      error: null,
    });
    expect(screen.getByTestId("pr-status").textContent).toBe("Merged");
    expect(screen.queryByRole("button", { name: /^Merge/ })).toBeNull();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5 * 60_000);
    });
    expect(t.calls.some((c) => c.op === "git.pr")).toBe(false);
  });

  it("disables Merge with GitHub's reason, and shows failed checks", async () => {
    const t = new FakeTransport();
    await mount(t);
    expect(screen.queryByTestId("pr-banner")).toBeNull();
    const blocked = pr({
      checks: { ...pr().checks, failed: 1, pending: 0 },
      merge: { allowed: false, status: "blocked", reason: "Waiting for required checks or review", method: "squash", methods: ["squash"] },
    });
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: blocked, error: null });
    expect(screen.getByTestId("pr-status").textContent).toBe("Checks failed");
    expect(merge().disabled).toBe(true);
    expect(merge().title).toBe("Waiting for required checks or review");
    expect(screen.getByTestId("pr-reason").textContent).toBe("Waiting for required checks or review");

    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: pr({ state: "closed" }), error: null });
    expect(screen.getByTestId("pr-status").textContent).toBe("Closed");
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: null, error: null });
    expect(screen.queryByTestId("pr-banner")).toBeNull();
  });

  it("offers Auto-merge while GitHub waits on checks, then Cancel auto-merge", async () => {
    const t = new FakeTransport();
    const waiting = pr({
      merge: {
        allowed: false,
        status: "blocked",
        reason: "Waiting for required checks or review",
        method: "squash",
        methods: ["squash"],
        auto: null,
        auto_available: true,
      },
    });
    const snapshot = t.snapshot.bind(t);
    t.snapshot = () => ({ ...snapshot(), prs: { a: { pr: waiting, error: null } } });
    t.handlers["git.pr_auto_merge"] = (p) =>
      (p as { enabled: boolean }).enabled
        ? { number: 12, outcome: "on", method: "squash" }
        : { number: 12, outcome: "off", method: null };
    await mount(t);

    const auto = screen.getByRole("button", { name: "Auto-merge" }) as HTMLButtonElement;
    expect(screen.queryByRole("button", { name: /^Merge/ })).toBeNull();
    expect(auto.title).toBe(
      "Squash and merge once checks pass, by GitHub itself. Pushes before then are merged too, once they pass.",
    );
    fireEvent.click(auto);
    await act(async () => {
      await Promise.resolve();
    });
    // Pinned to the head shown, with the method GitHub's own button would use.
    expect(t.calls.find((c) => c.op === "git.pr_auto_merge")!.params).toEqual({
      workspace_id: "a",
      number: 12,
      enabled: true,
      head_sha: "abc123",
      method: "squash",
    });
    // On as soon as the Host says so, before the feed catches up.
    expect(screen.getByTestId("pr-reason").textContent).toBe("Merges when checks pass");
    const queued = { ...waiting, merge: { ...waiting.merge, auto: { method: "squash" as const }, auto_available: false } };
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: queued, error: null });
    expect(screen.getByTestId("pr-reason").textContent).toBe("Merges when checks pass");
    // Checks done, still waiting on review.
    await t.feedEvent("workspace.pr_changed", {
      workspace_id: "a",
      pr: { ...queued, checks: { ...queued.checks, passed: 2, pending: 0 } },
      error: null,
    });
    expect(screen.getByTestId("pr-reason").textContent).toBe("Merges when GitHub's requirements are met");

    fireEvent.click(screen.getByRole("button", { name: /Cancel auto-merge/ }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(t.calls.filter((c) => c.op === "git.pr_auto_merge").at(-1)!.params).toEqual({
      workspace_id: "a",
      number: 12,
      enabled: false,
    });
    expect(screen.getByRole("button", { name: "Auto-merge" })).toBeTruthy();
    expect(screen.getByTestId("pr-reason").textContent).toBe("Waiting for required checks or review");

    // GitHub merging it brings the Continue | Archive choice as any merge does.
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: pr({ state: "merged" }), error: null });
    expect(screen.getByTestId("pr-status").textContent).toBe("Merged");
    expect(screen.queryByRole("button", { name: /auto-merge/i })).toBeNull();
  });

  it("merges at once a PR that became mergeable, and says when the repo has auto-merge off", async () => {
    const t = new FakeTransport();
    const waiting = pr({
      merge: { allowed: false, status: "blocked", reason: "Waiting", method: "squash", methods: ["squash"], auto: null, auto_available: true },
    });
    const replies = [
      () => {
        throw new ZwiimClientError("auto_merge_disabled", "Auto-merge is off", 409);
      },
      () => ({ number: 12, outcome: "merged", method: "squash" }),
    ];
    t.handlers["git.pr_auto_merge"] = () => replies.shift()!();
    await mount(t);
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: waiting, error: null });
    fireEvent.click(screen.getByRole("button", { name: "Auto-merge" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole("alert").textContent).toBe("Auto-merge is off for this repository on GitHub.");
    fireEvent.click(screen.getByRole("button", { name: "Auto-merge" }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByTestId("pr-status").textContent).toBe("Merged");
  });

  it("keeps Merge for a Host without auto-merge", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "git.pr_auto_merge") });
    await mount(t);
    await t.feedEvent("workspace.pr_changed", {
      workspace_id: "a",
      pr: pr({ merge: { allowed: false, status: "blocked", reason: "Waiting", method: "squash", methods: ["squash"], auto_available: true } }),
      error: null,
    });
    expect(merge().disabled).toBe(true);
    expect(screen.queryByRole("button", { name: "Auto-merge" })).toBeNull();
  });

  it("leaves only Merge with Auto-merge turned off in Settings, yet still cancels one queued", async () => {
    rememberAutoMergeButton(false);
    const t = new FakeTransport();
    t.handlers["git.pr_auto_merge"] = () => ({ number: 12, outcome: "off", method: null });
    await mount(t);
    const merge = { allowed: false, status: "blocked" as const, reason: "Waiting", method: "squash" as const, methods: ["squash" as const] };
    await t.feedEvent("workspace.pr_changed", {
      workspace_id: "a",
      pr: pr({ merge: { ...merge, auto: null, auto_available: true } }),
      error: null,
    });
    expect(screen.queryByRole("button", { name: "Auto-merge" })).toBeNull();
    expect(screen.getByRole("button", { name: /^Merge/ })).toBeTruthy();

    // Turned on from another device: this one can still cancel it.
    await t.feedEvent("workspace.pr_changed", {
      workspace_id: "a",
      pr: pr({ merge: { ...merge, auto: { method: "squash" }, auto_available: false } }),
      error: null,
    });
    fireEvent.click(screen.getByRole("button", { name: /Cancel auto-merge/ }));
    await act(async () => {
      await Promise.resolve();
    });
    expect(t.calls.filter((c) => c.op === "git.pr_auto_merge").at(-1)!.params).toMatchObject({ enabled: false });
    expect(screen.queryByRole("button", { name: "Auto-merge" })).toBeNull();
  });

  it("says when the PR moved since it was shown, or GitHub refuses", async () => {
    const t = new FakeTransport();
    const refusals = [
      new ZwiimClientError("pr_changed", "The pull request changed", 409),
      new ZwiimClientError("not_mergeable", "Not mergeable", 409, { reason: "Merge conflicts" }),
    ];
    t.handlers["git.pr_merge"] = () => {
      throw refusals.shift();
    };
    await mount(t);
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: pr(), error: null });
    fireEvent.click(merge());
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole("alert").textContent).toBe(
      "The pull request changed since you looked. Check it again before merging.",
    );
    fireEvent.click(merge());
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole("alert").textContent).toBe("Merge conflicts");
    // A new head clears the refusal.
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: pr({ head_sha: "def456" }), error: null });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("explains a Host whose GitHub CLI is signed out", async () => {
    const t = new FakeTransport();
    await mount(t);
    await t.feedEvent("workspace.pr_changed", {
      workspace_id: "a",
      pr: null,
      error: { code: "gh_unauthenticated", message: "GitHub CLI on the Host is not logged in" },
    });
    expect(screen.getByTestId("pr-banner").textContent).toBe(
      "GitHub CLI on the Host isn't signed in.Sign in to GitHub",
    );
    t.handlers["github.sign_in"] = () => ({
      state: "signing_in",
      code: "ABCD-1234",
      url: "https://github.com/login/device",
    });
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    await tick();
    expect(screen.getByLabelText("Sign-in code").textContent).toBe("ABCD-1234");
    fireEvent.click(screen.getByRole("button", { name: /Open GitHub/ }));
    expect(openLink).toHaveBeenCalledWith("https://github.com/login/device");
  });

  it("on a Host without GitHub sign-in, still says to run gh auth login", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "github.auth") });
    await mount(t);
    await t.feedEvent("workspace.pr_changed", {
      workspace_id: "a",
      pr: null,
      error: { code: "gh_unauthenticated", message: "GitHub CLI on the Host is not logged in" },
    });
    expect(screen.getByTestId("pr-banner").textContent).toBe(
      "GitHub CLI on the Host isn't signed in. Run gh auth login there to follow pull requests.",
    );
    expect(screen.queryByRole("button", { name: "Sign in to GitHub" })).toBeNull();
  });

  it("a merge the signed-out GitHub CLI refused offers Sign in to GitHub", async () => {
    const t = new FakeTransport();
    t.handlers["git.pr_merge"] = () => {
      throw new ZwiimClientError("gh_unauthenticated", "GitHub CLI on the Host is not logged in", 422);
    };
    t.handlers["github.sign_in"] = () => ({ state: "signed_in", login: "Ashish1101" });
    await mount(t);
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: pr(), error: null });
    fireEvent.click(merge());
    await tick();
    expect(screen.getByRole("alert").textContent).toBe("GitHub CLI on the Host isn't signed in.");
    fireEvent.click(screen.getByRole("button", { name: "Sign in to GitHub" }));
    await tick();
    // Signed in, the refusal no longer stands: Merge again.
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.queryByRole("button", { name: "Sign in to GitHub" })).toBeNull();
  });

  it("continues a merged PR's worktree on a new branch from its base, and lets the banner go", async () => {
    const t = mergedHost();
    t.handlers["workspace.continue"] = () => worktree({ branch: "zwiim/b", base_branch: "origin/main" });
    await mount(t);

    expect(screen.getByTestId("pr-status").textContent).toBe("Merged");
    expect(continueButton()!.disabled).toBe(false);
    expect(continueButton()!.title).toBe(
      "New branch from origin/main, where #12 landed. Sessions and terminals stay here.",
    );

    fireEvent.click(continueButton()!);
    await tick();
    expect(t.calls.find((c) => c.op === "workspace.continue")!.params).toEqual({ workspace_id: "a", number: 12 });
    // Gone as soon as the Host answers, before the feed clears the PR.
    expect(screen.queryByTestId("pr-banner")).toBeNull();
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: null, error: null });
    expect(screen.queryByTestId("pr-banner")).toBeNull();
    // The new branch's own pull request shows again.
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: pr({ number: 13, head: "zwiim/b" }), error: null });
    expect(screen.getByTestId("pr-banner").textContent).toContain("#13");
  });

  it("offers Continue only on a merged PR in a live worktree whose Host serves it", async () => {
    const cases: [string, FakeTransport][] = [];
    const closed = mergedHost();
    closed.snapshot = () => ({ ...FakeTransport.prototype.snapshot.call(closed), prs: { a: { pr: pr({ state: "closed" }), error: null } } });
    cases.push(["closed", closed]);
    cases.push(["root", mergedHost(worktree({ kind: "root", parent_workspace_id: null, base_branch: null }))]);
    cases.push(["removed", mergedHost(worktree({ removed: true, branch: null }))]);
    const older = mergedHost();
    older.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "workspace.continue") });
    cases.push(["older Host", older]);

    for (const [name, t] of cases) {
      await mount(t);
      expect(screen.getByTestId("pr-banner"), name).toBeTruthy();
      expect(continueButton(), name).toBeNull();
      cleanup();
    }
  });

  it("disables Continue while the Host is offline", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "setInterval"] });
    const t = mergedHost();
    await mount(t);
    expect(continueButton()!.disabled).toBe(false);
    await act(async () => t.handlersForEvents!.onClose({ code: 1006, reason: "" }));
    await tick();
    expect(continueButton()!.disabled).toBe(true);
    expect(continueButton()!.title).toBe("The Host is offline");
  });

  it("says why the Host refused to continue", async () => {
    const t = mergedHost();
    const refusals: [ZwiimClientError, string][] = [
      [
        new ZwiimClientError("workspace_busy", "Workspace is busy", 409, { live_sessions: 1, terminals: 0, session_ids: ["s1"] }),
        "An agent is still working here. Wait for it or stop it, then Continue.",
      ],
      [
        new ZwiimClientError("workspace_dirty", "Workspace has uncommitted changes", 409, { changes: 2 }),
        "Uncommitted changes here. Commit or discard them first.",
      ],
      [
        new ZwiimClientError("head_not_merged", "Branch moved", 409, { head: "def", pr_head: "abc" }),
        "This branch has commits #12 didn't merge. Continue would leave them behind.",
      ],
      [
        new ZwiimClientError("pr_not_merged", "Not merged", 409, { number: 12, state: "open" }),
        "#12 isn't merged on GitHub yet.",
      ],
      [new ZwiimClientError("github_unreachable", "gh failed", 502), "The Host couldn't reach GitHub. Try again."],
    ];
    const queue = refusals.map(([e]) => e);
    t.handlers["workspace.continue"] = () => {
      throw queue.shift();
    };
    await mount(t);
    for (const [, text] of refusals) {
      fireEvent.click(continueButton()!);
      await tick();
      expect(screen.getByRole("alert").textContent).toBe(text);
    }
    // Refused, the banner stays so the user can try again.
    expect(screen.getByTestId("pr-status").textContent).toBe("Merged");
  });

  it("archives a merged PR's worktree in one click, and says what became of its branch", async () => {
    const t = mergedHost();
    t.handlers["workspace.remove_worktree"] = (p) => ({
      workspace_id: p.workspace_id,
      parent_workspace_id: "root",
      kept_sessions: 1,
      branch: "zwiim/a",
      branch_deleted: true,
    });
    await mount(t);
    const archive = archiveButton()!;
    expect(archive.disabled).toBe(false);
    expect(continueButton()).toBeTruthy();
    fireEvent.click(archive);
    await tick();
    expect(t.calls.filter((c) => c.op === "workspace.remove_worktree").map((c) => c.params)).toEqual([
      { workspace_id: "a" },
    ]);
    expect(screen.getByRole("status").textContent).toBe("Deleted branch zwiim/a.");
  });

  it("asks before an archive loses uncommitted changes, and refuses while work runs", async () => {
    const t = mergedHost();
    const answers: (ZwiimClientError | null)[] = [
      new ZwiimClientError("workspace_busy", "Workspace is busy", 409, { live_sessions: 1, terminals: 0, session_ids: ["s1"] }),
      new ZwiimClientError("workspace_dirty", "Workspace has uncommitted changes", 409, { changes: 2 }),
      new ZwiimClientError("workspace_dirty", "Workspace has uncommitted changes", 409, { changes: 2 }),
      null,
    ];
    t.handlers["workspace.remove_worktree"] = (p) => {
      const e = answers.shift();
      if (e) throw e;
      return { workspace_id: p.workspace_id, parent_workspace_id: "root", kept_sessions: 0 };
    };
    await mount(t);

    fireEvent.click(archiveButton()!);
    await tick();
    expect(screen.getByRole("alert").textContent).toBe("1 agent running — stop it to archive.");

    fireEvent.click(archiveButton()!);
    await tick();
    expect(screen.getByRole("alertdialog").textContent).toContain("2 uncommitted changes will be lost.");
    expect(archiveButton()!.disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();

    fireEvent.click(archiveButton()!);
    await tick();
    fireEvent.click(screen.getByRole("button", { name: "Discard and archive" }));
    await tick();
    expect(t.calls.filter((c) => c.op === "workspace.remove_worktree").map((c) => c.params.force)).toEqual([
      undefined,
      undefined,
      undefined,
      true,
    ]);
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("offers Archive only on a merged PR in a live worktree", async () => {
    const closed = mergedHost();
    closed.snapshot = () => ({ ...FakeTransport.prototype.snapshot.call(closed), prs: { a: { pr: pr({ state: "closed" }), error: null } } });
    const cases: [string, FakeTransport][] = [
      ["closed", closed],
      ["root", mergedHost(worktree({ kind: "root", parent_workspace_id: null, base_branch: null }))],
      ["archived", mergedHost(worktree({ removed: true, branch: null }))],
    ];
    for (const [name, t] of cases) {
      await mount(t);
      expect(archiveButton(), name).toBeNull();
      cleanup();
    }
    // A Host without Continue still archives.
    const older = mergedHost();
    older.handlers["host.hello"] = () =>
      hostHello({ capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "workspace.continue") });
    await mount(older);
    expect(continueButton()).toBeNull();
    expect(archiveButton()).toBeTruthy();
  });
});

const shas = { base_sha: "b".repeat(40), head_sha: "h".repeat(40) };
const conflicting = (files = ["src/app.ts", "README.md"]): ConflictCheck => ({
  base: "origin/main",
  ...shas,
  state: "conflicts",
  files,
});
const dirty = () =>
  pr({ merge: { allowed: false, status: "dirty", reason: "Merge conflicts", method: "squash", methods: ["squash"] } });
const summary = (id: string, extra: Partial<SessionSummary> = {}): SessionSummary => ({
  session_id: id,
  workspace_id: "a",
  title: null,
  agent: "codex",
  status: "pending_review",
  live: false,
  created_at: "2026-10-29T00:00:00Z",
  updated_at: "2026-10-29T00:00:00Z",
  ...extra,
});

/** A Host whose worktree "a" conflicts with main. */
function conflictHost({
  check = conflicting(),
  prState = dirty() as PrState | null,
  sessions = [summary("s1")],
  workspace = worktree(),
  capabilities = [...CLIENT_CAPABILITIES],
}: {
  check?: ConflictCheck;
  prState?: PrState | null;
  sessions?: SessionSummary[];
  workspace?: WorkspaceInfo;
  capabilities?: string[];
} = {}) {
  const t = new FakeTransport();
  t.handlers["host.hello"] = () => hostHello({ capabilities });
  t.handlers["workspace.list"] = () => [workspace];
  t.handlers["session.list"] = () => sessions;
  t.handlers["workspace.resolve_conflicts"] = () => ({ base: "origin/main", files: check.state === "clean" ? [] : check.files });
  t.handlers["session.cancel"] = () => ({});
  const snapshot = t.snapshot.bind(t);
  t.snapshot = () => ({
    ...snapshot(),
    prs: prState ? { a: { pr: prState, error: null } } : {},
    conflicts: { a: check },
  });
  return t;
}

const resolveButton = () => screen.getByRole("button", { name: /^(Merge|Resolve|Resolving) conflicts/ }) as HTMLButtonElement;

describe("merge conflicts", () => {
  it("turns Merge into Merge conflicts, which hands them to the chat shown, then follows the turn", async () => {
    const t = conflictHost({ sessions: [summary("s2"), summary("s1")] });
    await mount(t, "s1");

    expect(screen.getByTestId("pr-conflicts").textContent).toContain("2 files conflict with main");
    // The one line replaces GitHub's generic reason.
    expect(screen.queryByTestId("pr-reason")).toBeNull();
    expect(screen.queryByRole("button", { name: /^Merge$/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /2 files conflict with main/ }));
    expect(screen.getByTestId("conflict-files").textContent).toBe("src/app.tsREADME.md");

    expect(resolveButton().textContent).toBe("Merge conflicts");
    fireEvent.click(resolveButton());
    await tick();
    expect(t.calls.find((c) => c.op === "workspace.resolve_conflicts")!.params).toEqual({
      workspace_id: "a",
      session_id: "s1",
    });

    await t.feedEvent("workspace.resolving_changed", { workspace_id: "a", session_id: "s1" });
    expect(resolveButton().textContent).toBe("Resolving conflicts…");
    expect(resolveButton().disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await tick();
    expect(t.calls.find((c) => c.op === "session.cancel")!.params).toEqual({ session_id: "s1" });

    // Resolved: the button follows the PR again, and the user merges once more.
    await t.feedEvent("workspace.conflicts_changed", { workspace_id: "a", conflicts: null });
    await t.feedEvent("workspace.resolving_changed", { workspace_id: "a", session_id: null });
    await t.feedEvent("workspace.pr_changed", { workspace_id: "a", pr: pr(), error: null });
    expect(screen.queryByTestId("pr-conflicts")).toBeNull();
    expect(screen.getByRole("button", { name: /^Merge$/ })).toBeTruthy();
  });

  it("resolves by rebasing from the secondary menu", async () => {
    const t = conflictHost();
    await mount(t);
    fireEvent.click(screen.getByRole("button", { name: "More ways to resolve" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Rebase instead" }));
    await tick();
    expect(t.calls.find((c) => c.op === "workspace.resolve_conflicts")!.params).toEqual({
      workspace_id: "a",
      session_id: "s1",
      strategy: "rebase",
    });
  });

  it("says a stopped merge is in progress", async () => {
    const t = conflictHost({
      check: { base: "origin/main", ...shas, state: "in_progress", operation: "merge", files: ["src/app.ts"] },
    });
    await mount(t);
    expect(screen.getByTestId("pr-conflicts").textContent).toContain("Merge in progress: 1 file unresolved");
    expect(resolveButton().textContent).toBe("Merge conflicts");
  });

  it("without a PR, offers Resolve conflicts", async () => {
    const t = conflictHost({ prState: null });
    await mount(t);
    expect(screen.getByTestId("pr-conflicts").textContent).toContain("2 files conflict with main");
    expect(resolveButton().textContent).toBe("Resolve conflicts");
    fireEvent.click(resolveButton());
    await tick();
    expect(t.calls.some((c) => c.op === "workspace.resolve_conflicts")).toBe(true);
  });

  it("without a PR, still says why the pull request can't be followed", async () => {
    const t = conflictHost({ prState: null });
    await mount(t);
    await t.feedEvent("workspace.pr_changed", {
      workspace_id: "a",
      pr: null,
      error: { code: "gh_unauthenticated", message: "GitHub CLI on the Host is not logged in" },
    });
    expect(screen.getByTestId("pr-conflicts").textContent).toContain("2 files conflict with main");
    expect(screen.getByTestId("pr-error").textContent).toBe("GitHub CLI on the Host isn't signed in.");
    expect(screen.getByRole("button", { name: "Sign in to GitHub" })).toBeTruthy();
  });

  it("shows a root's conflicts with no button: no session runs there", async () => {
    const t = conflictHost({ prState: null, workspace: worktree({ kind: "root", parent_workspace_id: null }) });
    await mount(t);
    expect(screen.getByTestId("pr-conflicts").textContent).toContain("2 files conflict with main");
    expect(screen.queryByRole("button", { name: /Resolve conflicts|Merge conflicts/ })).toBeNull();
  });

  it("waits while an agent works here, and asks for a chat when there is none", async () => {
    const busy = conflictHost({ sessions: [summary("s1", { live: true, status: "in_progress" })] });
    await mount(busy);
    expect(resolveButton().disabled).toBe(true);
    expect(screen.getByTestId("conflict-blocked").textContent).toBe(
      "An agent is working here. Wait for it or stop it, then resolve conflicts.",
    );
    cleanup();

    await mount(conflictHost({ sessions: [] }));
    expect(resolveButton().disabled).toBe(true);
    expect(screen.getByTestId("conflict-blocked").textContent).toBe("Start a chat here first, then resolve conflicts.");
  });

  it("says why the Host refused to resolve", async () => {
    const t = conflictHost();
    t.handlers["workspace.resolve_conflicts"] = () => {
      throw new ZwiimClientError("no_conflicts", "No conflicts", 409);
    };
    await mount(t);
    fireEvent.click(resolveButton());
    await tick();
    expect(screen.getByRole("alert").textContent).toBe("Nothing to resolve: the branch merges cleanly now.");
  });

  it("on an older Host, leaves the banner as it was", async () => {
    const t = conflictHost({
      capabilities: CLIENT_CAPABILITIES.filter((c) => c !== "workspace.conflicts" && c !== "workspace.resolve_conflicts"),
    });
    await mount(t);
    expect(screen.queryByTestId("pr-conflicts")).toBeNull();
    expect(screen.getByRole("button", { name: /^Merge$/ }).hasAttribute("disabled")).toBe(true);
    expect(screen.getByTestId("pr-reason").textContent).toBe("Merge conflicts");
  });
});
