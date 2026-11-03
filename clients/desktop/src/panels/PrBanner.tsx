import { useEffect, useMemo, useRef, useState } from "react";
import {
  Archive,
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
import {
  busyDetails,
  conflictView,
  dirtyDetails,
  hasCapability,
  resolveLabel,
  resolveRefusalText,
  type CheckState,
  type ConflictView,
  type MergeMethod,
  type PrCheckItem,
  type PrAutoMerge,
  type PrEntry,
  type PrState,
  type ResolveStrategy,

} from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { openLink } from "@/lib/bridge";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { autoMergeButton } from "@/lib/prefs";
import { usePref } from "@/lib/use-pref";
import { cn } from "@/lib/utils";
import { GithubSignIn, useGithubSignInOffered } from "./Github";
import { busyMessage, removalNote } from "./Sidebar";

/** What a merge does, as GitHub's own button says it. */
export const METHOD_LABELS: Record<MergeMethod, string> = {
  squash: "Squash and merge",
  merge: "Create a merge commit",
  rebase: "Rebase and merge",
};

/**
 * Why the Host can't see pull requests, in words for the user.
 * `signInOffered`: the caller shows Sign in to GitHub under a `gh_unauthenticated` line.
 */
export function prErrorText(code: string, message: string, signInOffered = false): string {
  switch (code) {
    case "gh_unauthenticated":
      return signInOffered
        ? "GitHub CLI on the Host isn't signed in."
        : "GitHub CLI on the Host isn't signed in. Run gh auth login there to follow pull requests.";
    case "gh_unavailable":
      return "GitHub CLI isn't installed on the Host. Install gh there to follow pull requests.";
    default:
      return message;
  }
}

/** A refused merge in words: what changed, or why GitHub won't merge it now. */
export function mergeErrorText(e: unknown, signInOffered = false): string {
  const code = e && typeof e === "object" && "code" in e ? String((e as { code: unknown }).code) : null;
  const details = e && typeof e === "object" && "details" in e ? (e as { details?: Record<string, unknown> }).details : undefined;
  switch (code) {
    case "pr_changed":
      return "The pull request changed since you looked. Check it again before merging.";
    case "not_mergeable":
      return typeof details?.reason === "string" ? details.reason : "GitHub won't merge it right now.";
    case "auto_merge_disabled":
      return "Auto-merge is off for this repository on GitHub.";
    case "gh_unauthenticated":
    case "gh_unavailable":
      return prErrorText(code, "", signInOffered);
    case "github_unreachable":
      return "The Host couldn't reach GitHub. Try again.";
    default:
      return errorMessage(e);
  }
}

function isGhUnauthenticated(e: unknown): boolean {
  return !!e && typeof e === "object" && "code" in e && (e as { code: unknown }).code === "gh_unauthenticated";
}

/** A refused Continue in words: what holds the worktree, or what GitHub says. */
export function continueErrorText(e: unknown, number: number, signInOffered = false): string {
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
      return mergeErrorText(e, signInOffered);
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
  work: "text-muted-foreground",
  mut: "text-muted-foreground",
};

function CheckIcon({ state, className }: { state: CheckState; className?: string }) {
  switch (state) {
    case "passed":
      return <Check className={cn("text-done size-3.5", className)} aria-label="passed" />;
    case "failed":
      return <X className={cn("text-destructive size-3.5", className)} aria-label="failed" />;
    case "pending":
      return <Loader2 className={cn("text-muted-foreground size-3.5 animate-spin", className)} aria-label="running" />;
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
        className="hover:bg-muted flex w-full items-center gap-2 rounded-sm px-1.5 py-1 text-left font-mono text-2xs transition-colors disabled:cursor-default disabled:hover:bg-transparent"
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

/** The conflict line; it opens the list of files git couldn't merge. */
function ConflictLine({ view }: { view: ConflictView }) {
  const [open, setOpen] = useState(false);
  if (!view.line) return null;
  return (
    <>
      <button
        type="button"
        className="hover:text-foreground flex items-center gap-0.5 text-left text-xs font-medium transition-colors"
        aria-expanded={open}
        disabled={view.files.length === 0}
        onClick={() => setOpen((o) => !o)}
      >
        {view.line}
        {view.files.length > 0 && (open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />)}
      </button>
      {open && (
        <ul className="mt-1 max-h-48 overflow-y-auto font-mono text-2xs" aria-label="Conflicting files" data-testid="conflict-files">
          {view.files.map((f) => (
            <li key={f} className="text-muted-foreground truncate py-0.5" title={f}>
              {f}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

/**
 * The conflict button where Merge was: one tap hands the conflicts to the
 * chat's agent, merging the base in; the menu beside it rebases instead.
 * While that turn runs, it reads Resolving with a Cancel for the turn.
 */
function ResolveControls({
  workspaceId,
  view,
  hasPr,
  onError,
}: {
  workspaceId: string;
  view: ConflictView;
  hasPr: boolean;
  onError(text: string | null): void;
}) {
  const controller = useController();
  const online = useOnline();
  const [sending, setSending] = useState(false);
  const [menu, setMenu] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!menu) return;
    const close = (e: MouseEvent) => {
      if (!menuRef.current?.contains(e.target as Node)) setMenu(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [menu]);
  const action = view.resolve!;

  async function resolve(strategy?: ResolveStrategy) {
    if (action.state !== "ready") return;
    setMenu(false);
    setSending(true);
    onError(null);
    try {
      await controller.resolveConflicts(workspaceId, action.session_id, strategy);
    } catch (e) {
      onError(resolveRefusalText(e) ?? errorMessage(e));
    } finally {
      setSending(false);
    }
  }

  async function cancel() {
    if (action.state !== "resolving") return;
    try {
      await controller.client.request("session.cancel", { session_id: action.session_id });
    } catch (e) {
      onError(errorMessage(e));
    }
  }

  const ready = action.state === "ready" && online && !sending;
  const busy = action.state === "resolving" || sending;
  const title = !online ? "The Host is offline" : action.state === "blocked" ? action.reason : undefined;
  return (
    <div className="flex shrink-0 items-center gap-1">
      {action.state === "resolving" && (
        <Button size="xs" variant="ghost" disabled={!online} onClick={() => void cancel()}>
          Cancel
        </Button>
      )}
      <div className="relative flex" ref={menuRef}>
        <Button
          size="xs"
          variant="brand"
          className={cn(action.state !== "resolving" && "rounded-r-none")}
          disabled={!ready}
          title={title}
          onClick={() => void resolve()}
        >
          {busy ? <Loader2 className="animate-spin" /> : <GitMerge />}
          {resolveLabel(action, hasPr)}
        </Button>
        {action.state !== "resolving" && (
          <Button
            size="xs"
            variant="brand"
            className="border-brand-foreground/20 rounded-l-none border-l px-1"
            disabled={!ready}
            aria-label="More ways to resolve"
            aria-haspopup="menu"
            aria-expanded={menu}
            onClick={() => setMenu((m) => !m)}
          >
            <ChevronDown />
          </Button>
        )}
        {menu && (
          <div
            role="menu"
            className="bg-popover text-popover-foreground absolute top-full right-0 z-20 mt-1 min-w-40 rounded-md border p-1 shadow-md"
            onKeyDown={(e) => e.key === "Escape" && setMenu(false)}
          >
            <button
              type="button"
              role="menuitem"
              className="hover:bg-muted w-full rounded-sm px-2 py-1 text-left text-xs"
              title="Rebase this branch onto its base, resolving each commit"
              onClick={() => void resolve("rebase")}
            >
              Rebase instead
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/** Conflicts on a branch with no pull request (or a root's): the line, and Resolve where it can run. */
/** `prError`: why the Host can't follow this branch's pull request, if it can't. */
function ConflictBanner({
  workspaceId,
  view,
  prError,
}: {
  workspaceId: string;
  view: ConflictView;
  prError?: { code: string; message: string } | null;
}) {
  const signInOffered = useGithubSignInOffered();
  const [error, setError] = useState<string | null>(null);
  return (
    <section className="shrink-0 border-b px-3 py-2" aria-label="Merge conflicts" data-testid="pr-banner">
      <div className="flex items-start gap-2">
        <GitMerge className="text-muted-foreground mt-0.5 size-4 shrink-0" />
        <div className="min-w-0 flex-1" data-testid="pr-conflicts">
          <ConflictLine view={view} />
          <BlockedReason view={view} />
        </div>
      </div>
      {/* Its own row: the side panel is too narrow for the line, Cancel and Resolving side by side. */}
      {view.resolve && (
        <div className="mt-1.5 pl-6">
          <ResolveControls workspaceId={workspaceId} view={view} hasPr={false} onError={setError} />
        </div>
      )}
      {error && (
        <p className="text-destructive mt-1 pl-6 text-xs" role="alert">
          {error}
        </p>
      )}
      {prError && (
        <div className="text-muted-foreground mt-1.5 flex flex-col gap-2 pl-6 text-xs">
          <span data-testid="pr-error">{prErrorText(prError.code, prError.message, signInOffered)}</span>
          {prError.code === "gh_unauthenticated" && signInOffered && <GithubSignIn />}
        </div>
      )}
    </section>
  );
}

function BlockedReason({ view }: { view: ConflictView }) {
  if (view.resolve?.state !== "blocked") return null;
  return (
    <p className="text-muted-foreground mt-1 text-xs" data-testid="conflict-blocked">
      {view.resolve.reason}
    </p>
  );
}

/**
 * The workspace branch's pull request, as the Host follows it on GitHub:
 * number and title (opens it), status, checks (each opens its run), and Merge,
 * or Auto-merge while GitHub waits on checks or review (GitHub then merges it,
 * with this device asleep or not).
 * Once merged, Continue moves the worktree to a new branch from the PR's base,
 * or Archive puts it away.
 * When the branch conflicts with its base, Merge becomes Merge conflicts, a
 * turn for the chat's agent. Nothing here asks GitHub or git; the Host's feed
 * keeps it current.
 * `sessionId`: the chat on screen, which resolves the conflicts when it is here.

 */
export function PrBanner({ workspaceId, sessionId }: { workspaceId: string; sessionId?: string | null }) {
  const entry: PrEntry | undefined = useHost((s) => s.prs[workspaceId]);
  const state = useHost((s) => s);
  const conflicts = useMemo(() => conflictView(state, workspaceId, sessionId), [state, workspaceId, sessionId]);
  const workspace = useHost((s) => s.workspaces?.find((w) => w.workspace_id === workspaceId));
  const continueOffered = useHost((s) => hasCapability(s.connection.compat, "workspace.continue"));
  const autoOffered = useHost((s) => hasCapability(s.connection.compat, "git.pr_auto_merge"));
  const autoButton = usePref(autoMergeButton);
  const signInOffered = useGithubSignInOffered();
  const controller = useController();
  const online = useOnline();
  const [open, setOpen] = useState(false);
  const [merging, setMerging] = useState(false);
  /** Auto-merge being turned on or off. */
  const [arming, setArming] = useState(false);
  const [continuing, setContinuing] = useState(false);
  const [archiving, setArchiving] = useState(false);
  /** Uncommitted changes the last archive would lose, awaiting a yes. */
  const [discarding, setDiscarding] = useState<number | null>(null);
  /** What became of the archived worktree's branch and archive script. */
  const [archived, setArchived] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The last merge was refused for want of a signed-in GitHub CLI. */
  const [signInNeeded, setSignInNeeded] = useState(false);
  /** The PR this client just merged, shown merged until the feed says so too. */
  const [justMerged, setJustMerged] = useState<number | null>(null);
  /** The PR this client just continued past, hidden until the feed clears it. */
  const [continued, setContinued] = useState<number | null>(null);
  /** Auto-merge as this client just set it, shown until the feed says otherwise. */
  const [autoSet, setAutoSet] = useState<{ number: number; auto: PrAutoMerge | null } | null>(null);
  const feedPr = entry?.pr ?? null;
  const pr =
    feedPr && justMerged === feedPr.number && feedPr.state !== "merged"
      ? { ...feedPr, state: "merged" as const }
      : feedPr && autoSet?.number === feedPr.number
        ? {
            ...feedPr,
            // Just turned off, it can go back on: it was waiting on GitHub's requirements.
            merge: {
              ...feedPr.merge,
              auto: autoSet.auto,
              auto_available: !autoSet.auto && (!!feedPr.merge.auto_available || !!feedPr.merge.auto),
            },
          }
        : feedPr;

  // A new head is a new pull request to judge; an old refusal no longer applies.
  useEffect(() => {
    setError(null);
    setSignInNeeded(false);
  }, [workspaceId, pr?.head_sha, pr?.state]);
  useEffect(() => {
    setJustMerged(null);
    setContinued(null);
    setDiscarding(null);
    setArchived(null);
  }, [workspaceId, feedPr?.number]);
  // The feed's word on auto-merge replaces what this client set.
  useEffect(() => {
    setAutoSet(null);
  }, [workspaceId, feedPr?.number, feedPr?.merge.auto?.method, !!feedPr?.merge.auto]);

  const gone = !pr || (feedPr !== null && continued === feedPr.number);
  if (gone && conflicts) return <ConflictBanner workspaceId={workspaceId} view={conflicts} prError={entry?.error} />;
  if (!entry) return null;
  if (feedPr && continued === feedPr.number) return null;
  if (!pr) {
    if (!entry.error) return null;
    return (
      <div className="text-muted-foreground flex shrink-0 items-start gap-2 border-b px-3 py-2 text-xs" data-testid="pr-banner">
        <GitPullRequest className="mt-px size-3.5 shrink-0" />
        <div className="flex min-w-0 flex-col gap-2">
          <span>{prErrorText(entry.error.code, entry.error.message, signInOffered)}</span>
          {/* Signed in, the Host's feed finds the pull request on its next look. */}
          {entry.error.code === "gh_unauthenticated" && signInOffered && <GithubSignIn />}
        </div>
      </div>
    );
  }

  const status = prStatus(pr);
  const settled = pr.state === "merged" || pr.state === "closed";
  // A merged or closed PR has nothing left to merge; its conflicts don't matter.
  const conflict = settled ? null : conflicts;
  const method = pr.merge.method;
  const canMerge = online && !merging && pr.merge.allowed && !settled;
  const reason = !online ? "The Host is offline" : pr.merge.reason;
  const liveWorktree = pr.state === "merged" && workspace?.kind === "worktree" && !workspace.removed;
  const canContinue = liveWorktree && continueOffered;
  const busy = merging || continuing || archiving;
  /** GitHub merges it by itself once its requirements are met. Shown even with the button turned off, so it can be cancelled. */
  const autoOn = !settled && autoOffered && !!pr.merge.auto;
  const canAuto = !settled && autoOffered && autoButton && !pr.merge.allowed && !!pr.merge.auto_available;
  const autoMethod = pr.merge.auto?.method ?? method;
  const autoLine =
    pr.checks.pending > 0 ? "Merges when checks pass" : "Merges when GitHub's requirements are met";

  async function merge() {
    setMerging(true);
    setError(null);
    setSignInNeeded(false);
    try {
      // Pinned to the head shown here: a push since then is refused, not merged.
      const merged = await controller.mergePr(workspaceId, pr!, method ?? undefined);
      setJustMerged(merged.number);
    } catch (e) {
      setError(mergeErrorText(e, signInOffered));
      setSignInNeeded(signInOffered && isGhUnauthenticated(e));
    } finally {
      setMerging(false);
    }
  }

  async function setAuto(enabled: boolean) {
    setArming(true);
    setError(null);
    setSignInNeeded(false);
    try {
      // On is pinned to the head shown here; GitHub keeps it across this user's later pushes.
      const set = await controller.setPrAutoMerge(workspaceId, pr!, enabled, enabled ? (method ?? undefined) : undefined);
      if (set.outcome === "merged") setJustMerged(set.number);
      else setAutoSet({ number: set.number, auto: set.outcome === "on" ? { method: set.method } : null });
    } catch (e) {
      setError(mergeErrorText(e, signInOffered));
      setSignInNeeded(signInOffered && isGhUnauthenticated(e));
    } finally {
      setArming(false);
    }
  }

  async function continueHere() {
    setContinuing(true);
    setError(null);
    setSignInNeeded(false);
    try {
      await controller.continueWorkspace(workspaceId, pr!.number);
      setContinued(pr!.number);
    } catch (e) {
      setError(continueErrorText(e, pr!.number, signInOffered));
      setSignInNeeded(signInOffered && isGhUnauthenticated(e));
    } finally {
      setContinuing(false);
    }
  }

  async function archive(force: boolean) {
    setArchiving(true);
    setError(null);
    setSignInNeeded(false);
    setDiscarding(null);
    try {
      setArchived(removalNote(await controller.removeWorktree(workspaceId, { force })));
    } catch (e) {
      const working = busyDetails(e);
      const dirty = dirtyDetails(e);
      if (working) setError(busyMessage(working));
      else if (dirty) setDiscarding(dirty.changes);
      else setError(errorMessage(e));
    } finally {
      setArchiving(false);
    }
  }

  return (
    <section className="shrink-0 border-b px-3 py-2" aria-label="Pull request" data-testid="pr-banner">
      <div className="flex items-center gap-2">
        <PrIcon pr={pr} />
        <button
          type="button"
          className="min-w-0 flex-1 truncate text-left text-ui hover:underline"
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
                variant="brand"
                className="shrink-0"
                disabled={!online || busy}
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
            {liveWorktree && (
              <Button
                size="xs"
                variant="secondary"
                className="shrink-0"
                disabled={!online || busy || discarding !== null}
                title={online ? "Archive this worktree. Its sessions stay readable." : "The Host is offline"}
                onClick={() => void archive(false)}
              >
                {archiving ? <Loader2 className="animate-spin" /> : <Archive />}
                Archive
              </Button>
            )}
          </>
        ) : conflict?.resolve ? (
          <ResolveControls workspaceId={workspaceId} view={conflict} hasPr onError={setError} />
        ) : autoOn ? (
          <Button
            size="xs"
            variant="secondary"
            className="shrink-0"
            disabled={!online || arming}
            title={online ? "Leave it for a manual merge" : "The Host is offline"}
            onClick={() => void setAuto(false)}
          >
            {arming ? <Loader2 className="animate-spin" /> : <X />}
            Cancel auto-merge
          </Button>
        ) : canAuto ? (
          <Button
            size="xs"
            variant="done"
            className="shrink-0"
            disabled={!online || arming}
            title={
              online
                ? `${method ? `${METHOD_LABELS[method]} once` : "GitHub merges it once"} ${pr.checks.pending > 0 ? "checks pass" : "GitHub's requirements are met"}, by GitHub itself. Pushes before then are merged too, once they pass.`
                : "The Host is offline"
            }
            onClick={() => void setAuto(true)}
          >
            {arming ? <Loader2 className="animate-spin" /> : <GitMerge />}
            Auto-merge
          </Button>
        ) : (
          <Button
            size="xs"
            variant="done"
            className="shrink-0"
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
        <div className="mt-1 pl-6 text-xs">
          {/* The status alone keeps the strip short; the counts and runs open under it. */}
          {pr.checks.items.length > 0 ? (
            <button
              type="button"
              className={cn("flex items-center gap-0.5 font-medium transition-opacity hover:opacity-80", TONE[status.tone])}
              aria-expanded={open}
              title={checksLine(pr)}
              onClick={() => setOpen((o) => !o)}
            >
              <span data-testid="pr-status">{status.label}</span>
              {open ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
            </button>
          ) : (
            <span className={cn("font-medium", TONE[status.tone])} data-testid="pr-status">
              {status.label}
            </span>
          )}
        </div>
      )}
      {conflict && (
        <div className="mt-1 pl-6" data-testid="pr-conflicts">
          <ConflictLine view={conflict} />
          <BlockedReason view={conflict} />
        </div>
      )}
      {conflict?.line ? null : autoOn ? (
        <p className="text-muted-foreground mt-1 pl-6 text-xs" data-testid="pr-reason" title={autoMethod ? METHOD_LABELS[autoMethod] : undefined}>
          {autoLine}
        </p>
      ) : (
        !settled &&
        !pr.merge.allowed &&
        reason && (
          <p className="text-muted-foreground mt-1 pl-6 text-xs" data-testid="pr-reason">
            {reason}
          </p>
        )
      )}
      {discarding !== null && (
        <div className="mt-1 flex items-center gap-1.5 pl-6 text-xs" role="alertdialog">
          <p className="min-w-0 flex-1">
            {discarding} uncommitted {discarding === 1 ? "change" : "changes"} will be lost.
          </p>
          {/* Focus lands on the safe choice (DESIGN.md: never on the destructive one). */}
          <Button size="xs" variant="ghost" autoFocus onClick={() => setDiscarding(null)}>
            Cancel
          </Button>
          <Button size="xs" variant="destructive" disabled={!online || archiving} onClick={() => void archive(true)}>
            Discard and archive
          </Button>
        </div>
      )}
      {archived && (
        <p className="text-muted-foreground mt-1 pl-6 text-xs" role="status">
          {archived}
        </p>
      )}
      {error && (
        <p className="text-destructive mt-1 pl-6 text-xs" role="alert">
          {error}
        </p>
      )}
      {error && signInNeeded && (
        <div className="mt-1 pl-6">
          <GithubSignIn
            onSignedIn={() => {
              setError(null);
              setSignInNeeded(false);
            }}
          />
        </div>
      )}
      {open && !settled && (
        <div className="mt-1 pl-4">
          <p className="text-muted-foreground px-1.5 font-mono text-2xs" data-testid="pr-checks-line">
            {checksLine(pr)}
          </p>
          <ul className="max-h-48 overflow-y-auto" aria-label="Checks">
            {pr.checks.items.map((c, i) => (
              <CheckRow key={`${c.workflow ?? ""}/${c.name}/${i}`} check={c} />
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
