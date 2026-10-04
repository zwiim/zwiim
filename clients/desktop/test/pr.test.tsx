import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_CAPABILITIES,
  HostClient,
  HostController,
  ZwiimClientError,
  type PrState,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { openLink } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { PrBanner } from "@/panels/PrBanner";
import { FakeTransport, hostHello, tick } from "./fakes";

vi.mock("@/lib/bridge", async (actual) => ({
  ...(await actual<typeof import("@/lib/bridge")>()),
  openLink: vi.fn(async () => {}),
}));

afterEach(() => {
  cleanup();
  vi.useRealTimers();
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

async function mount(t: FakeTransport) {
  const client = new HostClient({ transport: t });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <PrBanner workspaceId="a" />
    </HostProvider>,
  );
  await tick();
  return controller;
}

const merge = () => screen.getByRole("button", { name: /Merge/ }) as HTMLButtonElement;
const continueButton = () => screen.queryByRole("button", { name: /Continue/ }) as HTMLButtonElement | null;

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
      "GitHub CLI on the Host isn't signed in. Run gh auth login there to follow pull requests.",
    );
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
});
