// Pure helpers for GitHub clones, shared by every client: reading a pasted
// repository, matching clone jobs to repositories, and labelling progress.

import type { CloneJob } from "../types/protocol";

export interface RepoRef {
  owner: string;
  name: string;
}

/**
 * The repository a search text names: `owner/name`, a `github.com` URL (any
 * page of it), or an SSH remote. Null for anything else. The Host validates
 * again before cloning.
 */
export function repoRef(text: string): RepoRef | null {
  const t = text.trim().replace(/\/+$/, "");
  const m =
    /^(?:https?:\/\/)?(?:www\.)?github\.com\/([^/?#\s]+)\/([^/?#\s]+)/i.exec(t) ??
    /^git@github\.com:([^/\s]+)\/([^/\s]+)$/i.exec(t) ??
    /^([^/\s:]+)\/([^/\s]+)$/.exec(t);
  if (!m) return null;
  const owner = m[1]!;
  const name = m[2]!.replace(/\.git$/i, "");
  if (!/^[A-Za-z0-9][A-Za-z0-9-]{0,38}$/.test(owner)) return null;
  if (!/^[A-Za-z0-9._-]{1,100}$/.test(name) || name === "." || name === "..") return null;
  return { owner, name };
}

export const fullName = (r: RepoRef) => `${r.owner}/${r.name}`;

/** GitHub names are case-insensitive. */
export const sameRepo = (a: RepoRef, b: RepoRef) =>
  a.owner.toLowerCase() === b.owner.toLowerCase() && a.name.toLowerCase() === b.name.toLowerCase();

export const isActive = (j: CloneJob) => j.state === "queued" || j.state === "cloning" || j.state === "adding";

/** The job a repository's row shows: one in flight, else its failure, else a finished one. */
export function jobFor(jobs: readonly CloneJob[], repo: RepoRef): CloneJob | null {
  const mine = jobs.filter((j) => sameRepo(j, repo));
  return mine.find(isActive) ?? mine.find((j) => j.state === "failed") ?? mine.find((j) => j.state === "done") ?? null;
}

/** "Receiving objects 42%" */
export function progressLabel(job: CloneJob): string {
  if (job.state === "queued") return "Waiting to clone…";
  if (job.state === "adding") return "Opening…";
  const pct = job.percent === null ? "" : ` ${job.percent}%`;
  switch (job.phase) {
    case "receiving":
      return `Receiving objects${pct}`;
    case "resolving":
      return `Resolving deltas${pct}`;
    case "checkout":
      return `Checking out files${pct}`;
    default:
      return "Cloning…";
  }
}
