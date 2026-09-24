import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import type { DiffFile, WorkspaceDiff, WorkspaceInfo } from "@zwiim/sdk";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { cn } from "@/lib/utils";

const CHANGE_LETTER: Record<DiffFile["change"], string> = { added: "A", modified: "M", deleted: "D" };

/**
 * Read-only review of a workspace's changes, as the host computes them: a
 * worktree against its base, the root's uncommitted changes against HEAD.
 * Refetched when the host says this workspace's diff may have changed.
 */
export function DiffPanel({ workspace }: { workspace: WorkspaceInfo }) {
  const controller = useController();
  const workspaceId = workspace.workspace_id;
  const revision = useHost((s) => s.diffRevision[workspaceId] ?? 0);
  const [diff, setDiff] = useState<WorkspaceDiff | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [patch, setPatch] = useState<{ path: string; text: string | null } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const selectedRef = useRef<string | null>(null);
  /** Bumped per load; answers to older loads are dropped. */
  const loads = useRef(0);

  const select = (path: string | null) => {
    selectedRef.current = path;
    setSelected(path);
  };

  /** Reload the file list and, while one is open, that file's patch. */
  const refresh = useCallback(async () => {
    const load = ++loads.current;
    setError(null);
    setLoading(true);
    try {
      const next = await controller.loadDiff(workspaceId);
      if (load !== loads.current) return;
      setDiff(next);
      const file = selectedRef.current;
      if (file === null) return;
      if (!next.files.some((f) => f.path === file)) {
        select(null);
        setPatch(null);
        return;
      }
      const one = await controller.loadDiff(workspaceId, file);
      if (load === loads.current && selectedRef.current === file) {
        setPatch({ path: file, text: one.patch ?? null });
      }
    } catch (e) {
      if (load === loads.current) setError(errorMessage(e));
    } finally {
      if (load === loads.current) setLoading(false);
    }
  }, [controller, workspaceId]);

  useEffect(() => {
    setDiff(null);
    select(null);
    setPatch(null);
  }, [workspaceId]);

  useEffect(() => {
    void refresh();
  }, [refresh, revision]);

  async function open(file: DiffFile) {
    select(file.path);
    setPatch(null);
    setError(null);
    try {
      const one = await controller.loadDiff(workspaceId, file.path);
      if (selectedRef.current === file.path) setPatch({ path: file.path, text: one.patch ?? null });
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  const against =
    workspace.parent_workspace_id === null
      ? "Uncommitted changes against HEAD"
      : `Against ${workspace.base_branch ?? "base"}@${(diff?.base_commit ?? workspace.base_commit ?? "").slice(0, 7)}`;
  const current = diff?.files.find((f) => f.path === selected) ?? null;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 border-b px-4 py-1.5">
        <span className="text-muted-foreground text-xs font-medium uppercase">Diff</span>
        <span className="text-muted-foreground truncate text-xs">{against}</span>
        <Button className="ml-auto" size="icon-sm" variant="ghost" title="Refresh" onClick={refresh} disabled={loading}>
          <RefreshCw className={cn(loading && "animate-spin")} />
        </Button>
      </div>
      {error && (
        <p className="text-destructive px-4 py-2 text-xs" role="alert" data-testid="diff-error">
          {error}
        </p>
      )}
      <div className="grid min-h-0 flex-1 grid-cols-[minmax(12rem,1fr)_2fr] divide-x">
        <section className="min-h-0 overflow-auto p-2" data-testid="diff-files">
          {diff === null && !error && <Loading />}
          {diff?.files.length === 0 && <p className="text-muted-foreground p-2 text-xs">No changes</p>}
          {diff?.files.map((f) => (
            <button
              key={f.path}
              className={cn(
                "hover:bg-muted flex w-full items-center gap-2 rounded-md px-2 py-1 text-left font-mono text-xs",
                selected === f.path && "bg-muted",
              )}
              onClick={() => open(f)}
            >
              <span className="text-muted-foreground w-3 shrink-0">{CHANGE_LETTER[f.change]}</span>
              <span className="truncate">{f.path}</span>
              <span className="ml-auto flex shrink-0 items-center gap-1">
                {f.untracked && <Badge variant="outline">untracked</Badge>}
                {f.binary && <Badge>binary</Badge>}
                {f.too_large && <Badge>too large</Badge>}
                {f.additions !== null && <span className="text-emerald-600">+{f.additions}</span>}
                {f.deletions !== null && <span className="text-red-600">−{f.deletions}</span>}
              </span>
            </button>
          ))}
          {diff?.truncated && (
            <p className="text-muted-foreground p-2 text-xs">More files changed than can be listed.</p>
          )}
        </section>
        <section className="min-h-0 overflow-auto" data-testid="diff-patch">
          {selected === null && <p className="text-muted-foreground p-4 text-xs">Select a file to see its changes.</p>}
          {selected !== null && patch?.path !== selected && !error && <Loading />}
          {selected !== null && patch?.path === selected && patch.text === null && (
            <p className="text-muted-foreground p-4 text-xs">
              {current?.binary ? "Binary file" : "File too large to show"} — content not shown.
            </p>
          )}
          {selected !== null && patch?.path === selected && patch.text !== null && <Patch text={patch.text} />}
        </section>
      </div>
    </div>
  );
}

function Loading() {
  return (
    <p className="text-muted-foreground flex items-center gap-1.5 p-2 text-xs">
      <Loader2 className="size-3 animate-spin" /> Loading…
    </p>
  );
}

export interface PatchLine {
  text: string;
  kind: "add" | "del" | "context" | "hunk" | "meta";
  /** Line number in the old file; null where the line is not in it. */
  old: number | null;
  /** Line number in the new file; null where the line is not in it. */
  new: number | null;
}

/** A unified diff, line by line, with old/new line numbers from its hunks. */
export function numberPatch(text: string): PatchLine[] {
  let oldNo = 0;
  let newNo = 0;
  let inHunk = false;
  return text.split("\n").map((line): PatchLine => {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
    if (hunk) {
      oldNo = Number(hunk[1]);
      newNo = Number(hunk[2]);
      inHunk = true;
      return { text: line, kind: "hunk", old: null, new: null };
    }
    if (line.startsWith("diff --git ")) inHunk = false;
    if (!inHunk || line.startsWith("\\")) return { text: line, kind: "meta", old: null, new: null };
    if (line.startsWith("+")) return { text: line, kind: "add", old: null, new: newNo++ };
    if (line.startsWith("-")) return { text: line, kind: "del", old: oldNo++, new: null };
    return { text: line, kind: "context", old: oldNo++, new: newNo++ };
  });
}

function Patch({ text }: { text: string }) {
  // A trailing newline is not a line of the file.
  const lines = numberPatch(text.endsWith("\n") ? text.slice(0, -1) : text);
  return (
    <table className="w-full border-collapse font-mono text-xs leading-5">
      <tbody>
        {lines.map((l, i) => (
          <tr
            key={i}
            className={cn(
              l.kind === "add" && "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
              l.kind === "del" && "bg-red-500/10 text-red-700 dark:text-red-300",
              (l.kind === "hunk" || l.kind === "meta") && "text-muted-foreground",
            )}
          >
            <td className="text-muted-foreground w-10 pr-2 text-right align-top select-none">{l.old ?? ""}</td>
            <td className="text-muted-foreground w-10 border-r pr-2 text-right align-top select-none">{l.new ?? ""}</td>
            <td className="px-2 whitespace-pre">{l.text || " "}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
