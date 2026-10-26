import { useEffect, useRef, useState, type ReactNode } from "react";
import { Loader2, Lock, Search } from "lucide-react";
import {
  ZwiimClientError,
  fullName,
  isActive,
  jobFor,
  progressLabel,
  repoRef,
  sameRepo,
  type CloneJob,
  type GithubRepo,
  type RepoRef,
} from "@zwiim/sdk";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Dialog } from "@/components/ui/dialog";
import { errorMessage, useController, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";
import { relativeTime } from "./GitPanel";
import { GithubSignIn, useGithubSignInOffered } from "./Github";
import { Code, unavailableLine, type Unavailable } from "./PublishDialog";

/**
 * A failed clone's line (spec §5); a Zwiim add or worktree failure keeps its own message.
 * `signInOffered`: the caller shows Sign in to GitHub under a `gh_unauthenticated` line.
 */
export function cloneErrorLine(job: CloneJob, host: string, signInOffered = false): ReactNode {
  const code = job.error?.code ?? "failed";
  switch (code) {
    case "gh_unavailable":
    case "gh_unauthenticated":
      return unavailableLine(code, host, signInOffered);
    case "network":
      return "Couldn't reach GitHub.";
    case "not_found":
      return (
        <>
          <Code>{fullName(job)}</Code> was not found, or this account can't see it.
        </>
      );
    case "auth_failed":
      return (
        <>
          GitHub refused the credentials. Check <Code>gh auth status</Code>.
        </>
      );
    case "timeout":
      return "The clone stopped making progress.";
    case "failed":
      return "The clone failed.";
    default:
      return job.error?.message ?? "The clone failed.";
  }
}

/** A thin bar; without a percent, a full faint pulse. */
export function ProgressBar({ percent, fill, label }: { percent: number | null; fill: string; label: string }) {
  return (
    <div
      className="bg-muted h-1 w-full overflow-hidden rounded-full"
      role="progressbar"
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent ?? undefined}
      aria-valuetext={label}
    >
      <div
        className={cn("h-full rounded-full transition-[width]", fill, percent === null && "animate-pulse opacity-40")}
        style={{ width: `${percent ?? 100}%` }}
      />
    </div>
  );
}

/** GitHub's mark: Primer Octicons `mark-github` (MIT). lucide-react has none. */
export function GithubMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" className={cn("size-4", className)}>
      <path d="M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z" />
    </svg>
  );
}

type ListFailure = { kind: "unavailable"; reason: Unavailable } | { kind: "failed"; message: ReactNode };

/**
 * Why the list could not load. `network` and `timeout` are GitHub's only when
 * the Host answered (a status); without one they are the connection's own.
 */
function listFailure(e: unknown): ListFailure {
  if (e instanceof ZwiimClientError) {
    if (e.code === "gh_unavailable" || e.code === "gh_unauthenticated")
      return { kind: "unavailable", reason: e.code as Unavailable };
    if (e.status !== undefined) {
      if (e.code === "github_unreachable" || e.code === "network" || e.code === "timeout") {
        return { kind: "failed", message: "Couldn't reach GitHub." };
      }
      if (e.code === "auth_failed") {
        return {
          kind: "failed",
          message: (
            <>
              GitHub refused the credentials. Check <Code>gh auth status</Code>.
            </>
          ),
        };
      }
    }
  }
  return { kind: "failed", message: errorMessage(e) };
}

type List = { kind: "loading" } | ListFailure | { kind: "ready"; repos: GithubRepo[]; next: number | null };

export interface GithubCloneDialogProps {
  /** Whether the Host is another computer ("the host" in copy) or this one ("this Mac"). */
  remote: boolean;
  /** This Host's clone jobs, cancelled and dismissed ones left out. */
  jobs: readonly CloneJob[];
  /** Closing never stops a clone; it goes on in the workspace list. */
  onClose(): void;
  /** Start cloning `owner/name`; rejects with the Host's refusal. */
  onClone(owner: string, name: string): Promise<void>;
}

/** Lists the Host account's GitHub repositories; a click clones one into a workspace (spec §6). */
export function GithubCloneDialog({ remote, jobs, onClose, onClone }: GithubCloneDialogProps) {
  const controller = useController();
  const online = useOnline();
  const signInOffered = useGithubSignInOffered();
  const hostName = remote ? "the host" : "this Mac";
  const [list, setList] = useState<List>({ kind: "loading" });
  // `signIn`: the next page needs gh signed in.
  const [more, setMore] = useState<{ loading: boolean; error: ReactNode; signIn?: boolean }>({
    loading: false,
    error: null,
  });
  const [text, setText] = useState("");
  // Repositories whose clone request is on its way, lowercased `owner/name`.
  const [starting, setStarting] = useState<string[]>([]);
  const [refused, setRefused] = useState<{ key: string; message: string } | null>(null);
  // Each load's number: an answer to an older one is dropped.
  const asked = useRef(0);

  async function load(refresh: boolean) {
    const ask = ++asked.current;
    setList({ kind: "loading" });
    setMore({ loading: false, error: null });
    try {
      const page = await controller.githubRepos(undefined, refresh);
      if (ask === asked.current) setList({ kind: "ready", repos: page.repos, next: page.next_page });
    } catch (e) {
      if (ask === asked.current) setList(listFailure(e));
    }
  }

  async function loadMore() {
    if (list.kind !== "ready" || list.next === null || more.loading) return;
    const ask = asked.current;
    const page = list.next;
    setMore({ loading: true, error: null });
    try {
      const next = await controller.githubRepos(page);
      if (ask !== asked.current) return;
      setList((l) =>
        l.kind === "ready"
          ? {
              kind: "ready",
              repos: [...l.repos, ...next.repos.filter((r) => !l.repos.some((o) => sameRepo(o, r)))],
              next: next.next_page,
            }
          : l,
      );
      setMore({ loading: false, error: null });
    } catch (e) {
      if (ask !== asked.current) return;
      const f = listFailure(e);
      setMore(
        f.kind === "unavailable"
          ? {
              loading: false,
              error: unavailableLine(f.reason, hostName, signInOffered),
              signIn: f.reason === "gh_unauthenticated",
            }
          : { loading: false, error: f.message },
      );
    }
  }

  useEffect(() => {
    void load(false);
  }, []);

  async function clone(target: RepoRef) {
    const key = fullName(target).toLowerCase();
    if (starting.includes(key)) return;
    setRefused(null);
    setStarting((s) => [...s, key]);
    try {
      await onClone(target.owner, target.name);
    } catch (e) {
      setRefused({ key, message: errorMessage(e) });
    } finally {
      setStarting((s) => s.filter((k) => k !== key));
    }
  }

  async function cancel(job: CloneJob) {
    setRefused(null);
    try {
      await controller.cancelClone(job.clone_id);
    } catch (e) {
      setRefused({ key: fullName(job).toLowerCase(), message: errorMessage(e) });
    }
  }

  const ref = repoRef(text);
  const needle = (ref ? fullName(ref) : text.trim()).toLowerCase();
  const repos = list.kind === "ready" ? list.repos : [];
  const shown =
    needle === ""
      ? repos
      : repos.filter(
          (r) =>
            fullName(r).toLowerCase().includes(needle) ||
            (!ref && (r.description ?? "").toLowerCase().includes(needle)),
        );
  // A named repository no loaded page lists gets its own row; the Host decides if it exists.
  const pasted = ref && list.kind !== "unavailable" && !repos.some((r) => sameRepo(r, ref)) ? ref : null;

  const rowFor = (target: RepoRef, repo?: GithubRepo) => {
    const key = fullName(target).toLowerCase();
    return (
      <RepoRow
        key={key}
        target={target}
        repo={repo}
        job={jobFor(jobs, target)}
        starting={starting.includes(key)}
        refused={refused?.key === key ? refused.message : null}
        online={online}
        host={hostName}
        onClone={() => void clone(target)}
        onCancel={(job) => void cancel(job)}
      />
    );
  };

  return (
    <Dialog className="max-h-[80vh] max-w-lg" aria-label="Open GitHub project" onClose={onClose}>
      <div className="flex flex-col gap-3 px-5 pt-5">
        <h2 className="text-xl font-semibold tracking-tight">Open GitHub project</h2>
        {list.kind !== "unavailable" && (
          <label className="bg-muted/60 ring-border focus-within:ring-ring flex items-center gap-2 rounded-lg px-3 py-1.5 ring-1 transition-shadow">
            <Search className="text-muted-foreground size-3.5 shrink-0" />
            <input
              autoFocus
              aria-label="Search repositories"
              placeholder="Search, or paste owner/name"
              className="text-ui min-w-0 flex-1 bg-transparent outline-none"
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
          </label>
        )}
      </div>
      <div className="mt-3 flex min-h-0 flex-1 flex-col gap-px overflow-y-auto px-3">
        {list.kind === "unavailable" && (
          <div className="flex flex-col gap-2 px-3 py-2">
            <div className="flex items-center gap-2">
              <p className="flex-1 text-sm">{unavailableLine(list.reason, hostName, signInOffered)}</p>
              <Button size="xs" variant="ghost" disabled={!online} onClick={() => void load(true)}>
                Try again
              </Button>
            </div>
            {list.reason === "gh_unauthenticated" && signInOffered && (
              // Signed in, the list loads.
              <GithubSignIn host={hostName} onSignedIn={() => void load(true)} />
            )}
          </div>
        )}
        {list.kind === "loading" && (
          <p className="text-muted-foreground flex items-center gap-2 px-3 py-2 text-xs">
            <Loader2 className="size-3.5 animate-spin" />
            Loading repositories…
          </p>
        )}
        {list.kind === "failed" && (
          <div className="flex items-center gap-2 px-3 py-2">
            <p className="text-destructive flex-1 text-xs">{list.message}</p>
            <Button size="xs" variant="ghost" disabled={!online} onClick={() => void load(true)}>
              Try again
            </Button>
          </div>
        )}
        {list.kind === "ready" && shown.map((r) => rowFor(r, r))}
        {pasted && rowFor(pasted)}
        {list.kind === "ready" && shown.length === 0 && !pasted && (
          <p className="text-muted-foreground px-3 py-2 text-xs">
            {list.repos.length === 0 ? "No repositories on this account yet." : "No repositories match."}
          </p>
        )}
        {list.kind === "ready" && list.next !== null && (
          <div className="flex flex-col items-center gap-1 py-2">
            <Button size="sm" variant="ghost" disabled={more.loading || !online} onClick={() => void loadMore()}>
              {more.loading && <Loader2 className="animate-spin" />}
              Load more
            </Button>
            {more.error && <p className="text-destructive text-xs">{more.error}</p>}
            {more.signIn && signInOffered && (
              // Signed in, the page loads.
              <GithubSignIn host={hostName} onSignedIn={() => void loadMore()} />
            )}
          </div>
        )}
      </div>
      <div className="flex justify-end px-5 py-4">
        <Button variant="secondary" size="sm" onClick={onClose}>
          Close
        </Button>
      </div>
    </Dialog>
  );
}

interface RepoRowProps {
  target: RepoRef;
  /** The listed repository; absent for a pasted `owner/name`. */
  repo?: GithubRepo;
  job: CloneJob | null;
  starting: boolean;
  refused: string | null;
  online: boolean;
  host: string;
  onClone(): void;
  onCancel(job: CloneJob): void;
}

function RepoRow({ target, repo, job, starting, refused, online, host, onClone, onCancel }: RepoRowProps) {
  const name = fullName(target);
  const active = job !== null && isActive(job) ? job : null;
  const failed = job?.state === "failed" ? job : null;
  const signInOffered = useGithubSignInOffered();
  const added = (repo?.workspace_id ?? null) !== null || job?.state === "done";
  return (
    <div
      className="hover:bg-muted/60 has-[:focus-visible]:bg-muted/60 has-[:focus-visible]:ring-ring flex flex-col gap-1.5 rounded-lg px-3 py-2 transition-colors has-[:focus-visible]:ring-2"
      data-testid={`repo-${name}`}
    >
      <button
        className="flex min-w-0 flex-col items-start gap-0.5 text-left outline-none disabled:cursor-default"
        aria-label={`Clone ${name}`}
        disabled={active !== null || starting || !online}
        onClick={onClone}
      >
        <span className="flex max-w-full items-center gap-1.5">
          {!repo && <span className="text-muted-foreground text-ui">Clone</span>}
          <span className="text-ui truncate font-mono">{name}</span>
          {repo?.private && (
            <span role="img" aria-label="Private" title="Private" className="text-muted-foreground">
              <Lock className="size-3" aria-hidden="true" />
            </span>
          )}
          {repo?.fork && (
            <Badge variant="outline" className="text-2xs px-1.5 py-0">
              Fork
            </Badge>
          )}
          {repo?.archived && (
            <Badge variant="outline" className="text-2xs px-1.5 py-0">
              Archived
            </Badge>
          )}
          {added && (
            <Badge variant="secondary" className="text-2xs px-1.5 py-0">
              Added
            </Badge>
          )}
          {starting && <Loader2 className="text-muted-foreground size-3 animate-spin" />}
        </span>
        {repo && (repo.description || repo.pushed_at) && (
          <span className="text-muted-foreground flex w-full items-center gap-2 text-xs">
            <span className="min-w-0 flex-1 truncate">{repo.description}</span>
            {repo.pushed_at && <span className="shrink-0">{relativeTime(repo.pushed_at)}</span>}
          </span>
        )}
      </button>
      {active && (
        <div className="flex items-center gap-2">
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="text-muted-foreground text-xs">{progressLabel(active)}</span>
            <ProgressBar percent={active.percent} fill="bg-brand" label={progressLabel(active)} />
          </div>
          <Button
            size="xs"
            variant="ghost"
            disabled={active.state === "adding" || !online}
            onClick={() => onCancel(active)}
          >
            Cancel
          </Button>
        </div>
      )}
      {failed && (
        <div className="flex items-center gap-2">
          <p className="text-destructive min-w-0 flex-1 text-xs">{cloneErrorLine(failed, host, signInOffered)}</p>
          <Button size="xs" variant="ghost" disabled={starting || !online} onClick={onClone}>
            Retry
          </Button>
        </div>
      )}
      {failed?.error?.code === "gh_unauthenticated" && signInOffered && <GithubSignIn host={host} />}
      {refused && <p className="text-destructive text-xs">{refused}</p>}
    </div>
  );
}
