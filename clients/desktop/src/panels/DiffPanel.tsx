import { useCallback, useEffect, useRef, useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import { ZwiimClientError, type DiffFile, type WorkspaceDiff, type WorkspaceInfo } from "@zwiim/sdk";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { cn } from "@/lib/utils";

const CHANGE_LETTER: Record<DiffFile["change"], string> = { added: "A", modified: "M", deleted: "D" };

/** What a workspace's changes are measured against. */
function againstLabel(workspace: WorkspaceInfo, baseCommit: string | null | undefined): string {
  return workspace.parent_workspace_id === null
    ? "Uncommitted changes against HEAD"
    : `Against ${workspace.base_branch ?? "base"}@${(baseCommit ?? workspace.base_commit ?? "").slice(0, 7)}`;
}

/**
 * A workspace's changed files, as the host computes them: a worktree against
 * its base, the root's uncommitted changes against HEAD. Refetched when the
 * host says this workspace's diff may have changed. Opening a file shows its
 * patch beside the conversation.
 */
export function DiffPanel({
  workspace,
  selected,
  onOpen,
}: {
  workspace: WorkspaceInfo;
  /** The file whose patch is showing, if any. */
  selected: string | null;
  onOpen(path: string): void;
}) {
  const controller = useController();
  const workspaceId = workspace.workspace_id;
  const revision = useHost((s) => s.diffRevision[workspaceId] ?? 0);
  const [diff, setDiff] = useState<WorkspaceDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  /** Bumped per load; answers to older loads are dropped. */
  const loads = useRef(0);

  const refresh = useCallback(async () => {
    const load = ++loads.current;
    setError(null);
    setLoading(true);
    try {
      const next = await controller.loadDiff(workspaceId);
      if (load === loads.current) setDiff(next);
    } catch (e) {
      if (load === loads.current) setError(errorMessage(e));
    } finally {
      if (load === loads.current) setLoading(false);
    }
  }, [controller, workspaceId]);

  useEffect(() => setDiff(null), [workspaceId]);

  useEffect(() => {
    void refresh();
  }, [refresh, revision]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 px-3 py-1">
        <span className="text-muted-foreground truncate text-xs">{againstLabel(workspace, diff?.base_commit)}</span>
        <Button className="ml-auto" size="icon-sm" variant="ghost" title="Refresh" onClick={refresh} disabled={loading}>
          <RefreshCw className={cn(loading && "animate-spin")} />
        </Button>
      </div>
      {error && (
        <p className="text-destructive px-3 py-2 text-xs" role="alert" data-testid="diff-error">
          {error}
        </p>
      )}
      <section className="min-h-0 flex-1 overflow-auto px-1 pb-1" data-testid="diff-files">
        {diff === null && !error && <Loading />}
        {diff?.files.length === 0 && <p className="text-muted-foreground p-2 text-xs">No changes</p>}
        {diff?.files.map((f) => (
          <button
            key={f.path}
            className={cn(
              "hover:bg-muted flex w-full items-center gap-2 rounded-md px-2 py-1 text-left font-mono text-xs",
              selected === f.path && "bg-muted",
            )}
            title={f.path}
            onClick={() => onOpen(f.path)}
          >
            <span className="text-muted-foreground w-3 shrink-0">{CHANGE_LETTER[f.change]}</span>
            <span className="truncate">{f.path}</span>
            <FileBadges file={f} />
          </button>
        ))}
        {diff?.truncated && <p className="text-muted-foreground p-2 text-xs">More files changed than can be listed.</p>}
      </section>
    </div>
  );
}

function FileBadges({ file }: { file: DiffFile }) {
  return (
    <span className="ml-auto flex shrink-0 items-center gap-1">
      {file.untracked && <Badge variant="outline">untracked</Badge>}
      {file.binary && <Badge>binary</Badge>}
      {file.too_large && <Badge>too large</Badge>}
      {file.additions !== null && <span className="text-emerald-600">+{file.additions}</span>}
      {file.deletions !== null && <span className="text-red-600">−{file.deletions}</span>}
    </span>
  );
}

/** What one file's diff tab shows: its patch, or why there is none. */
type FilePatch = { file: DiffFile; patch: string | null; baseCommit: string | null } | { gone: true };

/**
 * One changed file's patch, shown in its own tab. Refetched when the host
 * says the workspace's diff may have changed; a file whose changes were
 * committed or reverted says so rather than closing under the reader.
 */
export function DiffView({ workspace, path }: { workspace: WorkspaceInfo; path: string }) {
  const controller = useController();
  const workspaceId = workspace.workspace_id;
  const revision = useHost((s) => s.diffRevision[workspaceId] ?? 0);
  const [shown, setShown] = useState<FilePatch | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const loads = useRef(0);

  const refresh = useCallback(async () => {
    const load = ++loads.current;
    setError(null);
    setLoading(true);
    try {
      const one = await controller.loadDiff(workspaceId, path);
      const file = one.files.find((f) => f.path === path);
      if (load !== loads.current) return;
      setShown(file ? { file, patch: one.patch ?? null, baseCommit: one.base_commit ?? null } : { gone: true });
    } catch (e) {
      if (load !== loads.current) return;
      if (e instanceof ZwiimClientError && e.code === "not_found") setShown({ gone: true });
      else setError(errorMessage(e));
    } finally {
      if (load === loads.current) setLoading(false);
    }
  }, [controller, workspaceId, path]);

  useEffect(() => {
    void refresh();
  }, [refresh, revision]);

  const file = shown && "file" in shown ? shown : null;
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="diff-view">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b px-4 text-xs">
        {file && (
          <span className="text-muted-foreground w-3 shrink-0 font-mono">{CHANGE_LETTER[file.file.change]}</span>
        )}
        <span className="truncate font-mono">{path}</span>
        {file && <FileBadges file={file.file} />}
        <span className={cn("text-muted-foreground truncate", !file && "ml-auto")}>
          {againstLabel(workspace, file?.baseCommit)}
        </span>
        <Button size="icon-sm" variant="ghost" title="Refresh" onClick={refresh} disabled={loading}>
          <RefreshCw className={cn(loading && "animate-spin")} />
        </Button>
      </div>
      <section className="min-h-0 flex-1 overflow-auto" data-testid="diff-patch">
        {error && (
          <p className="text-destructive p-4 text-xs" role="alert" data-testid="diff-error">
            {error}
          </p>
        )}
        {shown === null && !error && <Loading />}
        {shown && "gone" in shown && (
          <p className="text-muted-foreground p-4 text-xs">This file has no changes anymore.</p>
        )}
        {file && file.patch === null && (
          <p className="text-muted-foreground p-4 text-xs">
            {file.file.binary ? "Binary file" : "File too large to show"} — content not shown.
          </p>
        )}
        {file && file.patch !== null && <Patch text={file.patch} />}
      </section>
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
