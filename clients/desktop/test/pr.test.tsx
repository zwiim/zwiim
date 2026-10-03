import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, ZwiimClientError, type PrState } from "@zwiim/sdk";
import { openLink } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { PrBanner } from "@/panels/PrBanner";
import { FakeTransport, tick } from "./fakes";

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
});
