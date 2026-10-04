import { useEffect, useState } from "react";
import {
  Check,
  ChevronDown,
  ChevronRight,
  GitBranchPlus,
  GitMerge,
  GitPullRequest,
  GitPullRequestClosed,
  GitPullRequestDraft,
  Loader2,
  Minus,
  X,
} from "lucide-react";
import { hasCapability, type CheckState, type MergeMethod, type PrCheckItem, type PrEntry, type PrState } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { openLink } from "@/lib/bridge";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";

/** What a merge does, as GitHub's own button says it. */
export const METHOD_LABELS: Record<MergeMethod, string> = {
  squash: "Squash and merge",
  merge: "Create a merge commit",
  rebase: "Rebase and merge",
};

/** Why the Host can't see pull requests, in words for the user. */
export function prErrorText(code: string, message: string): string {
  switch (code) {
    case "gh_unauthenticated":
      return "GitHub CLI on the Host isn't signed in. Run gh auth login there to follow pull requests.";
    case "gh_unavailable":
      return "GitHub CLI isn't installed on the Host. Install gh there to follow pull requests.";
    default:
      return message;
  }
}

/** A refused merge in words: what changed, or why GitHub won't merge it now. */
export function mergeErrorText(e: unknown): string {
  const code = e && typeof e === "object" && "code" in e ? String((e as { code: unknown }).code) : null;
  const details = e && typeof e === "object" && "details" in e ? (e as { details?: Record<string, unknown> }).details : undefined;
  switch (code) {
    case "pr_changed":
      return "The pull request changed since you looked. Check it again before merging.";
    case "not_mergeable":
      return typeof details?.reason === "string" ? details.reason : "GitHub won't merge it right now.";
    case "gh_unauthenticated":
    case "gh_unavailable":
      return prErrorText(code, "");
    case "github_unreachable":
      return "The Host couldn't reach GitHub. Try again.";
    default:
      return errorMessage(e);
  }
}

/** A refused Continue in words: what holds the worktree, or what GitHub says. */
export function continueErrorText(e: unknown, number: number): string {
  const code = e && typeof e === "object" && "code" in e ? String((e as { code: unknown }).code) : null;
  switch (code) {
    case "workspace_busy":
      return "An agent is still working here. Wait for it or stop it, then Continue.";
    case "workspace_dirty":
      return "Uncommitted changes here. Commit or discard them first.";
    case "head_not_merged":
      return `This branch has commits #${number} didn't merge. Continue would leave them behind.`;
    case "pr_not_merged":
      return `#${number} isn't merged on GitHub yet.`;
    default:
      return mergeErrorText(e);
  }
}

/** The one-word state a PR is in: Merged, Closed, Draft, or how its checks stand. */
export function prStatus(pr: PrState): { label: string; tone: "done" | "err" | "work" | "mut" } {
  if (pr.state === "merged") return { label: "Merged", tone: "done" };
  if (pr.state === "closed") return { label: "Closed", tone: "mut" };
  if (pr.state === "draft") return { label: "Draft", tone: "mut" };
  const { failed, pending, passed } = pr.checks;
  if (failed > 0) return { label: "Checks failed", tone: "err" };
  if (pending > 0) return { label: "Checks running", tone: "work" };
  if (passed > 0) return { label: "Checks passed", tone: "done" };
  return { label: "Open", tone: "mut" };
}

const TONE: Record<"done" | "err" | "work" | "mut", string> = {
  done: "text-done",
  err: "text-destructive",
  work: "text-brand",
  mut: "text-muted-foreground",
};

function CheckIcon({ state, className }: { state: CheckState; className?: string }) {
  switch (state) {
    case "passed":
      return <Check className={cn("text-done size-3.5", className)} aria-label="passed" />;
    case "failed":
      return <X className={cn("text-destructive size-3.5", className)} aria-label="failed" />;
    case "pending":
      return <Loader2 className={cn("text-brand size-3.5 animate-spin", className)} aria-label="running" />;
    case "skipped":
      return <Minus className={cn("text-muted-foreground size-3.5", className)} aria-label="skipped" />;
  }
}

function PrIcon({ pr }: { pr: PrState }) {
  const c = "size-4 shrink-0";
  if (pr.state === "merged") return <GitMerge className={cn(c, "text-done")} />;
  if (pr.state === "closed") return <GitPullRequestClosed className={cn(c, "text-muted-foreground")} />;
  if (pr.state === "draft") return <GitPullRequestDraft className={cn(c, "text-muted-foreground")} />;
  return <GitPullRequest className={cn(c, "text-done")} />;
}

/** `2 passed · 1 running · 1 failed`, or `No checks`. */
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

function CheckRow({ check }: { check: PrCheckItem }) {
  const name = check.workflow ? `${check.workflow} / ${check.name}` : check.name;
  return (
    <li>
      <button
        type="button"
        className="hover:bg-muted flex w-full items-center gap-2 rounded px-1.5 py-1 text-left font-mono text-[11.5px] disabled:cursor-default disabled:hover:bg-transparent"
        disabled={!check.link}
        title={check.link ?? undefined}
        onClick={() => check.link && void openLink(check.link).catch(() => {})}
      >
        <CheckIcon state={check.state} />
        <span className="min-w-0 truncate">{name}</span>
      </button>
    </li>
  );
}

/**
 * The workspace branch's pull request, as the Host follows it on GitHub:
 * number and title (opens it), status, checks (each opens its run), and Merge.
 * Once merged, Continue moves the worktree to a new branch from the PR's base.
 * Nothing here asks GitHub; the Host's feed keeps it current.
 */
export function PrBanner({ workspaceId }: { workspaceId: string }) {
  const entry: PrEntry | undefined = useHost((s) => s.prs[workspaceId]);
  const workspace = useHost((s) => s.workspaces?.find((w) => w.workspace_id === workspaceId));
  const continueOffered = useHost((s) => hasCapability(s.connection.compat, "workspace.continue"));
  const controller = useController();
  const online = useOnline();
  const [open, setOpen] = useState(false);
  const [merging, setMerging] = useState(false);
  const [continuing, setContinuing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The PR this client just merged, shown merged until the feed says so too. */
  const [justMerged, setJustMerged] = useState<number | null>(null);
  /** The PR this client just continued past, hidden until the feed clears it. */
  const [continued, setContinued] = useState<number | null>(null);
  const feedPr = entry?.pr ?? null;
  const pr =
    feedPr && justMerged === feedPr.number && feedPr.state !== "merged"
      ? { ...feedPr, state: "merged" as const }
      : feedPr;

  // A new head is a new pull request to judge; an old refusal no longer applies.
  useEffect(() => setError(null), [workspaceId, pr?.head_sha, pr?.state]);
  useEffect(() => {
    setJustMerged(null);
    setContinued(null);
  }, [workspaceId, feedPr?.number]);

  if (!entry) return null;
  if (feedPr && continued === feedPr.number) return null;
  if (!pr) {
    if (!entry.error) return null;
    return (
      <div className="text-muted-foreground flex shrink-0 items-start gap-2 border-b px-3 py-2 text-xs" data-testid="pr-banner">
        <GitPullRequest className="mt-px size-3.5 shrink-0" />
        <span>{prErrorText(entry.error.code, entry.error.message)}</span>
      </div>
    );
  }

  const status = prStatus(pr);
  const settled = pr.state === "merged" || pr.state === "closed";
  const method = pr.merge.method;
  const canMerge = online && !merging && pr.merge.allowed && !settled;
  const reason = !online ? "The Host is offline" : pr.merge.reason;
  const canContinue =
    pr.state === "merged" && workspace?.kind === "worktree" && !workspace.removed && continueOffered;

  async function merge() {
    setMerging(true);
    setError(null);
    try {
      // Pinned to the head shown here: a push since then is refused, not merged.
      const merged = await controller.mergePr(workspaceId, pr!, method ?? undefined);
      setJustMerged(merged.number);
    } catch (e) {
      setError(mergeErrorText(e));
    } finally {
      setMerging(false);
    }
  }

  async function continueHere() {
    setContinuing(true);
    setError(null);
    try {
      await controller.continueWorkspace(workspaceId, pr!.number);
      setContinued(pr!.number);
    } catch (e) {
      setError(continueErrorText(e, pr!.number));
    } finally {
      setContinuing(false);
    }
  }

  return (
    <section className="shrink-0 border-b px-3 py-2" aria-label="Pull request" data-testid="pr-banner">
      <div className="flex items-center gap-2">
        <PrIcon pr={pr} />
        <button
          type="button"
          className="min-w-0 flex-1 truncate text-left text-[13px] hover:underline"
          title={pr.url}
          onClick={() => void openLink(pr.url).catch(() => {})}
        >
          <span className="text-muted-foreground font-mono">#{pr.number}</span> {pr.title}
        </button>
        {settled ? (
          <>
            <span className={cn("shrink-0 text-xs font-medium", TONE[status.tone])} data-testid="pr-status">
              {status.label}
            </span>
            {canContinue && (
              <Button
                size="xs"
                className="bg-brand text-brand-foreground hover:bg-brand/85 shrink-0"
                disabled={!online || continuing}
                title={
                  online
                    ? `New branch from origin/${pr.base}, where #${pr.number} landed. Sessions and terminals stay here.`
                    : "The Host is offline"
                }
                onClick={() => void continueHere()}
              >
                {continuing ? <Loader2 className="animate-spin" /> : <GitBranchPlus />}
                Continue
              </Button>
            )}
          </>
        ) : (
          <Button
            size="xs"
            className="bg-done text-done-foreground hover:bg-done/85 shrink-0"
            disabled={!canMerge}
            title={canMerge && method ? METHOD_LABELS[method] : (reason ?? undefined)}
            onClick={() => void merge()}
          >
            {merging ? <Loader2 className="animate-spin" /> : <GitMerge />}
            Merge
          </Button>
        )}
      </div>
      {!settled && (
        <div className="mt-1 flex items-center gap-2 pl-6 text-xs">
          <span className={cn("font-medium", TONE[status.tone])} data-testid="pr-status">
            {status.label}
          </span>
          {pr.checks.items.length > 0 ? (
            <button
              type="button"
              className="text-muted-foreground hover:text-foreground flex items-center gap-0.5 font-mono text-[11.5px]"
              aria-expanded={open}
              onClick={() => setOpen((o) => !o)}
            >
              {checksLine(pr)}
              {open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
            </button>
          ) : (
            <span className="text-muted-foreground font-mono text-[11.5px]">{checksLine(pr)}</span>
          )}
        </div>
      )}
      {!settled && !pr.merge.allowed && reason && (
        <p className="text-muted-foreground mt-1 pl-6 text-xs" data-testid="pr-reason">
          {reason}
        </p>
      )}
      {error && (
        <p className="text-destructive mt-1 pl-6 text-xs" role="alert">
          {error}
        </p>
      )}
      {open && !settled && (
        <ul className="mt-1 max-h-48 overflow-y-auto pl-4" aria-label="Checks">
          {pr.checks.items.map((c, i) => (
            <CheckRow key={`${c.workflow ?? ""}/${c.name}/${i}`} check={c} />
          ))}
        </ul>
      )}
    </section>
  );
}
