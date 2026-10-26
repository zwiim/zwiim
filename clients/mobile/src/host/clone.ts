// GitHub clones on the phone: what each failure says, which rows the
// repository list shows, and which clone cards and selections are due.

import {
  fullName,
  isActive,
  isClientError,
  jobFor,
  repoRef,
  sameRepo,
  type CloneJob,
  type GithubRepo,
  type RepoRef,
} from "@zwiim/sdk";
import { ago } from "../account/hosts";
import { errorInfo } from "./connection";
import { SIGN_IN_HERE } from "./github";

/** `signInOffered`: the Host signs its GitHub CLI in itself (`github.auth`), from the Host tab. */
const GH_LINES: Record<string, (host: string, signInOffered: boolean) => string> = {
  gh_unavailable: (host) => `GitHub CLI isn't installed on ${host}. Install gh there to clone projects.`,
  gh_unauthenticated: (host, signInOffered) =>
    signInOffered
      ? `GitHub CLI on ${host} isn't signed in. ${SIGN_IN_HERE}.`
      : `GitHub CLI on ${host} isn't signed in. Run gh auth login there.`,
};

const authLine = (host: string) => `GitHub refused the credentials. Run gh auth status on ${host}.`;

/** Why the repository list couldn't load. */
export function listProblem(e: unknown, host: string, signInOffered = false): string {
  const { code, message } = errorInfo(e);
  const gh = GH_LINES[code];
  if (gh) return gh(host, signInOffered);
  switch (code) {
    case "github_unreachable":
    case "timeout":
      return `${host} couldn't reach GitHub.`;
    case "network":
      // A Host reply carries a status; the phone's own "no connection" doesn't.
      return isClientError(e) && e.status !== undefined
        ? `${host} couldn't reach GitHub.`
        : `Not connected to ${host}. Try again when it's back.`;
    case "auth_failed":
      return authLine(host);
    default:
      return message;
  }
}

/** A failed clone's line, and whether trying again can help. Never the Host's message: it may hold paths. */
export function cloneProblem(job: CloneJob, host: string, signInOffered = false): { text: string; retry: boolean } {
  const code = job.error?.code ?? "failed";
  const gh = GH_LINES[code];
  if (gh) return { text: gh(host, signInOffered), retry: false };
  switch (code) {
    case "network":
      return { text: "Couldn't reach GitHub.", retry: true };
    case "timeout":
      return { text: "The clone stopped making progress.", retry: true };
    case "not_found":
      return { text: `${fullName(job)} was not found, or this account can't see it.`, retry: false };
    case "auth_failed":
      return { text: authLine(host), retry: false };
    default:
      return { text: "The clone failed.", retry: true };
  }
}

export type RepoRow =
  | { kind: "repo"; repo: GithubRepo; job: CloneJob | null }
  | { kind: "paste"; ref: RepoRef; job: CloneJob | null };

/**
 * The loaded repositories matching `query` (name, or description unless the
 * query is a repository), led by a paste row for a repository not loaded.
 */
export function repoRows(loaded: readonly GithubRepo[], query: string, jobs: readonly CloneJob[]): RepoRow[] {
  const ref = repoRef(query);
  const needle = ref ? fullName(ref).toLowerCase() : query.trim().toLowerCase();
  const match = (r: GithubRepo) =>
    fullName(r).toLowerCase().includes(needle) || (!ref && (r.description ?? "").toLowerCase().includes(needle));
  const rows: RepoRow[] = loaded.filter(match).map((repo) => ({ kind: "repo", repo, job: jobFor(jobs, repo) }));
  if (ref && !loaded.some((r) => sameRepo(r, ref))) rows.unshift({ kind: "paste", ref, job: jobFor(jobs, ref) });
  return rows;
}

/** The root workspace a row already has: listed by the Host, or just cloned (pages are cached). */
export function addedWorkspace(row: RepoRow): string | null {
  return (row.kind === "repo" ? row.repo.workspace_id : null) ?? (row.job?.state === "done" ? row.job.workspace_id : null);
}

function size(kb: number): string {
  if (kb < 1024) return `${kb} KB`;
  if (kb < 1024 * 1024) return `${(kb / 1024).toFixed(1)} MB`;
  return `${(kb / (1024 * 1024)).toFixed(1)} GB`;
}

/** "pushed 2h ago · 4.1 MB"; either part left out when unknown. `now` in unix seconds. */
export function repoMeta(repo: GithubRepo, now: number): string {
  const parts: string[] = [];
  const pushed = repo.pushed_at === null ? NaN : Date.parse(repo.pushed_at);
  if (!Number.isNaN(pushed)) parts.push(`pushed ${ago(Math.floor(pushed / 1000), now)}`);
  if (repo.size_kb > 0) parts.push(size(repo.size_kb));
  return parts.join(" · ");
}

/** Cards above the workspace list: clones in flight, and failures not dismissed on this phone. */
export function cloneCards(clones: Readonly<Record<string, CloneJob>>, dismissed: ReadonlySet<string>): CloneJob[] {
  return Object.values(clones).filter((j) => isActive(j) || (j.state === "failed" && !dismissed.has(j.clone_id)));
}

/**
 * Of the clones being waited on, the ones now done; `left` keeps the rest
 * still running or not yet seen. Failed and cancelled ones are let go.
 */
export function settleClones(clones: Readonly<Record<string, CloneJob>>, ids: readonly string[]): { done: CloneJob[]; left: string[] } {
  const done: CloneJob[] = [];
  const left: string[] = [];
  for (const id of ids) {
    const job = clones[id];
    if (!job || isActive(job)) left.push(id);
    else if (job.state === "done") done.push(job);
  }
  return { done, left };
}
