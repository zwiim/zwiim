// A worktree's pull request as the Host follows it on GitHub (DESIGN.md,
// Session strip → PR strip): what the strip, the branch chip's badge and the
// PR sheet say about it, and a refused merge in words. The phone never asks
// GitHub; the Host's feed keeps `HostState.prs` current, and Merge goes to the
// Host pinned to the head shown here. Once merged, Continue moves the worktree
// to a new branch from the PR's base.

import {
  hasCapability,
  isClientError,
  type Compatibility,
  type MergeMethod,
  type PrEntry,
  type PrState,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { SIGN_IN_HERE } from "./github";

export type PrTone = "done" | "err" | "work" | "mut";

/** What a merge does, as GitHub's own button says it. */
export const METHOD_LABELS: Record<MergeMethod, string> = {
  squash: "Squash and merge",
  merge: "Create a merge commit",
  rebase: "Rebase and merge",
};

export interface PrLook {
  /** `Checks running`, `Merged`… (the strip puts `#12` before it). */
  status: string;
  tone: PrTone;
  /** `1 failed · 1 running · 2 passed`, or `No checks`. */
  checks: string;
  /** Merged or closed: nothing left to do here. */
  settled: boolean;
  /** GitHub would merge it now (and the phone can ask). */
  canMerge: boolean;
  /** Why Merge is off, in words; null when it is on or settled. */
  reason: string | null;
  /** What Merge does, `Squash and merge`; null when the repository allows none. */
  method: string | null;
}

export function checksLine(pr: PrState): string {
  const { passed, failed, pending, skipped } = pr.checks;
  const parts = [
    failed > 0 && `${failed} failed`,
    pending > 0 && `${pending} running`,
    passed > 0 && `${passed} passed`,
    skipped > 0 && `${skipped} skipped`,
  ].filter(Boolean);
  return parts.length > 0 ? parts.join(" · ") : "No checks";
}

export function prLook(pr: PrState, connected: boolean): PrLook {
  const settled = pr.state === "merged" || pr.state === "closed";
  const { failed, pending, passed } = pr.checks;
  const [status, tone]: [string, PrTone] =
    pr.state === "merged"
      ? ["Merged", "done"]
      : pr.state === "closed"
        ? ["Closed", "mut"]
        : pr.state === "draft"
          ? ["Draft", "mut"]
          : failed > 0
            ? ["Checks failed", "err"]
            : pending > 0
              ? ["Checks running", "work"]
              : passed > 0
                ? ["Checks passed", "done"]
                : ["Open", "mut"];
  const canMerge = !settled && connected && pr.merge.allowed;
  const reason = settled || canMerge ? null : !connected ? "Not connected to the Host" : pr.merge.reason;
  return {
    status,
    tone,
    checks: checksLine(pr),
    settled,
    canMerge,
    reason,
    method: pr.merge.method ? METHOD_LABELS[pr.merge.method] : null,
  };
}

/** What a screen reader says for the strip: the counts only while open. */
export function prLabel(number: number, look: PrLook): string {
  const label = `Pull request #${number}, ${look.status}`;
  return look.settled ? label : `${label}. ${look.checks}`;
}

/** The branch chip's badge: `#12` in the PR's tone; null without a PR. */
export function prBadge(entry: PrEntry | undefined): { text: string; tone: PrTone } | null {
  const pr = entry?.pr;
  if (!pr) return null;
  return { text: `#${pr.number}`, tone: prLook(pr, true).tone };
}

/**
 * Why the Host can't see pull requests, in words for the user. `signInOffered`:
 * the Host signs its GitHub CLI in itself (`github.auth`), from the Host tab.
 */
export function prErrorText(code: string, message: string, hostName: string, signInOffered = false): string {
  if (code === "gh_unauthenticated") {
    return signInOffered
      ? `GitHub CLI on ${hostName} isn't signed in. ${SIGN_IN_HERE} to follow pull requests.`
      : `GitHub CLI on ${hostName} isn't signed in. Run gh auth login there to follow pull requests.`;
  }
  if (code === "gh_unavailable") return `GitHub CLI isn't installed on ${hostName}. Install gh there to follow pull requests.`;
  return message;
}

/** A refused or failed merge, in words, with the next step. */
export function mergeError(e: unknown, hostName: string, signInOffered = false): string {
  if (isClientError(e, "pr_changed")) return "The pull request changed since you looked. Check it again before merging.";
  if (isClientError(e, "not_mergeable")) {
    const reason = (e as { details?: Record<string, unknown> }).details?.reason;
    return typeof reason === "string" ? reason : "GitHub won't merge it right now.";
  }
  if (isClientError(e, "gh_unauthenticated") || isClientError(e, "gh_unavailable")) {
    return prErrorText((e as { code: string }).code, "", hostName, signInOffered);
  }
  if (isClientError(e, "github_unreachable")) return `${hostName} couldn't reach GitHub. Try again.`;
  if (isClientError(e, "unknown_outcome")) return `Not sure that went through. Check the PR on ${hostName} before trying again.`;
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't merge the pull request. Try again.";
}

/** Continue is for a merged PR in a worktree still on disk, on a Host that serves it. */
export function offersContinue(pr: PrState, workspace: WorkspaceInfo | undefined, compat: Compatibility | null): boolean {
  return (
    pr.state === "merged" &&
    workspace?.kind === "worktree" &&
    !workspace.removed &&
    hasCapability(compat, "workspace.continue")
  );
}

/** What Continue does, above its button. */
export const continueLine = (pr: PrState) =>
  `Start a new branch from ${pr.base}, where #${pr.number} landed. Sessions stay; each agent is told on its next turn. The old branch stays.`;

/** A refused or failed Continue: what holds the worktree, or what GitHub says. */
export function continueError(e: unknown, number: number, hostName: string, signInOffered = false): string {
  if (isClientError(e, "workspace_busy")) return "An agent is still working here. Wait for it or stop it, then Continue.";
  if (isClientError(e, "workspace_dirty")) return "Uncommitted changes here. Commit or discard them first.";
  if (isClientError(e, "head_not_merged")) return `This branch has commits #${number} didn't merge. Continue would leave them behind.`;
  if (isClientError(e, "pr_not_merged")) return `#${number} isn't merged on GitHub yet.`;
  if (isClientError(e, "unknown_outcome")) return `Not sure that went through. Check the branch on ${hostName} before trying again.`;
  if (
    isClientError(e, "gh_unauthenticated") ||
    isClientError(e, "gh_unavailable") ||
    isClientError(e, "github_unreachable") ||
    isClientError(e, "network") ||
    isClientError(e, "timeout")
  ) {
    return mergeError(e, hostName, signInOffered);
  }
  return "Couldn't continue. Try again.";
}

/** The head commit as people read it: `abc1234`. */
export const shortSha = (sha: string) => sha.slice(0, 7);
