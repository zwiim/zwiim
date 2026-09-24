import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUp, Loader2, RefreshCw } from "lucide-react";
import type { GitLogEntry, GitStatusEntry } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";

const STATUS: Record<string, { label: string; className: string }> = {
  M: { label: "Modified", className: "text-amber-600" },
  A: { label: "Added", className: "text-emerald-600" },
  D: { label: "Deleted", className: "text-red-600" },
  R: { label: "Renamed", className: "text-sky-600" },
  C: { label: "Copied", className: "text-sky-600" },
  T: { label: "Type changed", className: "text-amber-600" },
  U: { label: "Conflict", className: "text-destructive" },
  "??": { label: "Untracked", className: "text-muted-foreground" },
};

/** A porcelain status code (`M`, `MM`, `??`, `UU` …) in words. */
export function statusLabel(code: string): { letter: string; label: string; className: string } {
  if (code === "??") return { letter: "U", ...STATUS["??"]! };
  if (code.includes("U") || code === "AA" || code === "DD") return { letter: "!", ...STATUS.U! };
  const letter = code.trim()[0] ?? "?";
  return { letter, ...(STATUS[letter] ?? { label: code, className: "text-muted-foreground" }) };
}

/** "3 min ago", "yesterday", or the date for anything older than a week. */
export function relativeTime(iso: string, now = Date.now()): string {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return iso;
  const s = Math.max(0, Math.round((now - then) / 1000));
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  if (d === 1) return "yesterday";
  if (d < 7) return `${d} days ago`;
  return new Date(then).toLocaleDateString();
}

/**
 * Git as the host reports it: working-tree changes with a commit form, and
 * recent history. Refetched when the host says the workspace's diff may have
 * changed.
 */
export function GitPanel({ workspaceId }: { workspaceId: string }) {
  const controller = useController();
  const online = useOnline();
  const revision = useHost((s) => s.diffRevision[workspaceId] ?? 0);
  const [status, setStatus] = useState<GitStatusEntry[] | null>(null);
  const [log, setLog] = useState<GitLogEntry[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** Files the user left out of the next commit; the rest are included. */
  const [excluded, setExcluded] = useState<Set<string>>(new Set());
  const [message, setMessage] = useState("");
  const [committing, setCommitting] = useState(false);
  const [commitError, setCommitError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const loads = useRef(0);

  const refresh = useCallback(async () => {
    const load = ++loads.current;
    setLoading(true);
    setError(null);
    try {
      const [s, l] = await Promise.all([
        controller.client.request("git.status", { workspace_id: workspaceId }),
        controller.client.request("git.log", { workspace_id: workspaceId, limit: 50 }),
      ]);
      if (load !== loads.current) return;
      setStatus(s);
      setLog(l.entries);
    } catch (e) {
      if (load === loads.current) setError(errorMessage(e));
    } finally {
      if (load === loads.current) setLoading(false);
    }
  }, [controller, workspaceId]);

  useEffect(() => {
    setStatus(null);
    setLog(null);
    setExcluded(new Set());
    setMessage("");
    setNotice(null);
    setCommitError(null);
  }, [workspaceId]);

  useEffect(() => {
    void refresh();
  }, [refresh, revision]);

  const files = status?.map((e) => e.file).filter((f) => !excluded.has(f)) ?? [];
  const canCommit = online && !committing && message.trim() !== "" && files.length > 0;

  async function commit() {
    setCommitting(true);
    setCommitError(null);
    setNotice(null);
    try {
      // Never retried: an unknown outcome is resolved by looking at the repo.
      const r = await controller.client.request("git.commit", {
        workspace_id: workspaceId,
        message: message.trim(),
        files,
      });
      setMessage("");
      setExcluded(new Set());
      setNotice(`Committed ${r.committed_files} file${r.committed_files === 1 ? "" : "s"}.`);
    } catch (e) {
      setCommitError(errorMessage(e));
    } finally {
      setCommitting(false);
      void refresh();
    }
  }

  const toggle = (file: string) =>
    setExcluded((prev) => {
      const next = new Set(prev);
      if (next.has(file)) next.delete(file);
      else next.add(file);
      return next;
    });
  const allIncluded = status !== null && status.every((e) => !excluded.has(e.file));

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 border-b px-4 py-1.5">
        <span className="text-muted-foreground text-xs font-medium uppercase">Git</span>
        <Button
          className="ml-auto"
          size="icon-sm"
          variant="ghost"
          title="Refresh"
          onClick={refresh}
          disabled={loading}
        >
          <RefreshCw className={cn(loading && "animate-spin")} />
        </Button>
      </div>
      {error && (
        <p className="text-destructive px-4 py-2 text-xs" role="alert" data-testid="git-error">
          {error}
        </p>
      )}
      <div className="grid min-h-0 flex-1 grid-cols-2 divide-x">
        <section className="flex min-h-0 flex-col" data-testid="git-status">
          <div className="min-h-0 flex-1 overflow-auto p-3">
            <div className="mb-2 flex items-center gap-2">
              <h3 className="text-xs font-medium">Changes</h3>
              {status && status.length > 0 && (
                <label className="text-muted-foreground ml-auto flex items-center gap-1 text-xs">
                  <input
                    type="checkbox"
                    checked={allIncluded}
                    onChange={() => setExcluded(allIncluded ? new Set(status.map((e) => e.file)) : new Set())}
                  />
                  All
                </label>
              )}
            </div>
            {status === null && !error && <Loading />}
            {status?.length === 0 && <p className="text-muted-foreground text-xs">Working tree clean</p>}
            {status?.map((e) => {
              const s = statusLabel(e.status);
              return (
                <label key={e.file} className="hover:bg-muted/60 flex items-center gap-2 rounded px-1 py-0.5 text-xs">
                  <input type="checkbox" checked={!excluded.has(e.file)} onChange={() => toggle(e.file)} />
                  <span className={cn("w-3 shrink-0 font-mono font-semibold", s.className)} title={s.label}>
                    {s.letter}
                  </span>
                  <span className="truncate font-mono">{e.file}</span>
                  <span className="text-muted-foreground ml-auto shrink-0">{s.label}</span>
                </label>
              );
            })}
          </div>
          {status && status.length > 0 && (
            <div className="flex flex-col gap-2 border-t p-3" data-testid="git-commit">
              <textarea
                className="bg-background min-h-16 rounded-md border px-2 py-1.5 text-xs"
                placeholder="Commit message"
                aria-label="Commit message"
                value={message}
                onChange={(e) => setMessage(e.target.value)}
                disabled={committing}
              />
              <div className="flex items-center gap-2">
                <Button size="xs" onClick={commit} disabled={!canCommit}>
                  {committing && <Loader2 className="animate-spin" />}
                  Commit {files.length} file{files.length === 1 ? "" : "s"}
                </Button>
                {!online && <span className="text-muted-foreground text-xs">Reconnect to commit</span>}
              </div>
            </div>
          )}
          {commitError && (
            <p className="text-destructive border-t px-3 py-2 text-xs" role="alert" data-testid="git-commit-error">
              {commitError}
            </p>
          )}
          {notice && (
            <p className="border-t px-3 py-2 text-xs text-emerald-600" role="status" data-testid="git-notice">
              {notice}
            </p>
          )}
        </section>
        <section className="min-h-0 overflow-auto p-3" data-testid="git-log">
          <h3 className="mb-2 text-xs font-medium">History</h3>
          {log === null && !error && <Loading />}
          {log?.length === 0 && <p className="text-muted-foreground text-xs">No commits yet</p>}
          {log?.map((c) => {
            const [subject, ...body] = c.message.trim().split("\n");
            return (
              <div key={c.full_hash} className="border-b py-1.5 last:border-0" title={body.join("\n").trim() || undefined}>
                <div className="truncate text-xs">{subject}</div>
                <div className="text-muted-foreground flex items-center gap-2 text-[11px]">
                  <span className="font-mono">{c.hash}</span>
                  <span className="truncate">{c.author}</span>
                  {c.pushed === false && (
                    <span className="flex items-center gap-0.5" title="Not pushed yet">
                      <ArrowUp className="size-3" />
                      local
                    </span>
                  )}
                  <span className="ml-auto shrink-0" title={new Date(c.date).toLocaleString()}>
                    {relativeTime(c.date)}
                  </span>
                </div>
              </div>
            );
          })}
        </section>
      </div>
    </div>
  );
}

function Loading() {
  return (
    <p className="text-muted-foreground flex items-center gap-1.5 text-xs">
      <Loader2 className="size-3 animate-spin" /> Loading…
    </p>
  );
}
