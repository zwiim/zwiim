import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Columns2, Copy, Eye, FileText, Loader2, RefreshCw, Rows2 } from "lucide-react";
import {
  fullFile,
  isMarkdown,
  ZwiimClientError,
  type DiffFile,
  type FileLine,
  type WorkspaceDiff,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { Markdown } from "@/components/Markdown";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { diffLayout, rememberDiffLayout, type DiffLayout } from "@/lib/prefs";
import { cn } from "@/lib/utils";
import { FileChanges } from "./FileChanges";

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

  const isRepo = workspace.is_git_repo;
  useEffect(() => {
    // A plain folder has no diff: Git is never asked.
    if (isRepo) void refresh();
  }, [refresh, revision, isRepo]);

  if (!isRepo) return <NotGitRepository name={workspace.alias ?? workspace.name} />;

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

/** Git panels for a plain folder: nothing is wrong, there is just no Git. */
export function NotGitRepository({ name }: { name: string }) {
  return (
    <div className="flex flex-col gap-1 p-4 text-xs" data-testid="not-git-repository">
      <p className="font-medium">Not a Git repository</p>
      <p className="text-muted-foreground">
        {name} is a plain folder, so there are no changes or history to show. Files, terminals and sessions work as
        usual.
      </p>
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

/** What one file's tab shows, from its diff and its current content. */
type Viewed =
  | { kind: "lines"; lines: FileLine[]; file: DiffFile | null; baseCommit: string | null; content: string | null }
  | { kind: "binary"; file: DiffFile; baseCommit: string | null }
  | { kind: "too_large"; file: DiffFile | null; baseCommit: string | null }
  | { kind: "gone" };

const TOO_LARGE = "File is too large to open";

/** `files.read` refuses a file too big for the editor; that is not an error here. */
function isTooLarge(e: unknown): boolean {
  return e instanceof ZwiimClientError && e.code === "invalid_input" && /too large/i.test(e.message);
}

/**
 * One file in its own tab: the whole file as it is now, its uncommitted
 * changes marked and the unchanged stretches folded. Refetched when the host
 * says the workspace's diff may have changed; a file whose changes were
 * committed or reverted still shows, unmarked.
 */
export function DiffView({ workspace, path }: { workspace: WorkspaceInfo; path: string }) {
  const controller = useController();
  const workspaceId = workspace.workspace_id;
  const revision = useHost((s) => s.diffRevision[workspaceId] ?? 0);
  const [viewed, setViewed] = useState<Viewed | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [opened, setOpened] = useState<Record<number, number>>({});
  const [layout, setLayout] = useState<DiffLayout>(diffLayout);
  const [preview, setPreview] = useState(false);
  const loads = useRef(0);

  const refresh = useCallback(async () => {
    const load = ++loads.current;
    setError(null);
    setLoading(true);
    const diff = controller.loadDiff(workspaceId, path).catch((e: unknown) => {
      if (e instanceof ZwiimClientError && e.code === "not_found") return null;
      throw e;
    });
    const read = controller.client
      .request("files.read", { workspace_id: workspaceId, path })
      .then((f) => ({ content: f.content as string | null, tooLarge: false }))
      .catch((e: unknown) => ({ content: null, tooLarge: isTooLarge(e) }));
    try {
      const [one, now] = await Promise.all([diff, read]);
      if (load !== loads.current) return;
      const file = one?.files.find((f) => f.path === path) ?? null;
      const baseCommit = one?.base_commit ?? null;
      const patch = one?.patch ?? null;
      let next: Viewed;
      if (file?.binary) next = { kind: "binary", file, baseCommit };
      else if (file?.too_large || (file && patch === null) || now.tooLarge) next = { kind: "too_large", file, baseCommit };
      else if (file?.change === "deleted") next = { kind: "lines", lines: fullFile(patch, null), file, baseCommit, content: null };
      else if (file) next = { kind: "lines", lines: fullFile(patch, now.content), file, baseCommit, content: now.content };
      else if (now.content !== null)
        next = { kind: "lines", lines: fullFile(null, now.content), file: null, baseCommit, content: now.content };
      else next = { kind: "gone" };
      setViewed(next);
      setOpened({});
    } catch (e) {
      if (load === loads.current) setError(errorMessage(e));
    } finally {
      if (load === loads.current) setLoading(false);
    }
  }, [controller, workspaceId, path]);

  useEffect(() => {
    void refresh();
  }, [refresh, revision]);

  const chooseLayout = (next: DiffLayout) => {
    setLayout(next);
    rememberDiffLayout(next);
  };

  const file = viewed && viewed.kind !== "gone" ? viewed.file : null;
  const content = viewed?.kind === "lines" ? viewed.content : null;
  const markdown = isMarkdown(path) && file?.change !== "deleted";
  const showPreview = preview && markdown && content !== null;
  const slash = path.lastIndexOf("/");
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="diff-view">
      <div className="flex h-10 shrink-0 items-center gap-2 border-b px-4 text-xs">
        <FileText className="text-muted-foreground size-3.5 shrink-0" />
        <span className="min-w-0 truncate font-mono" title={path}>
          {slash >= 0 && <span className="text-muted-foreground">{path.slice(0, slash + 1)}</span>}
          <span className="text-foreground">{path.slice(slash + 1)}</span>
        </span>
        {file && <span className="text-muted-foreground shrink-0 font-mono">{CHANGE_LETTER[file.change]}</span>}
        {file && (
          <span className="flex shrink-0 items-center gap-1 font-mono">
            {file.additions !== null && <span className="text-emerald-600">+{file.additions}</span>}
            {file.deletions !== null && <span className="text-red-600">−{file.deletions}</span>}
          </span>
        )}
        {viewed && viewed.kind !== "gone" && (
          <span className="text-muted-foreground hidden truncate lg:inline">
            {againstLabel(workspace, viewed.baseCommit)}
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-1">
          <LayoutToggle layout={layout} onChange={chooseLayout} disabled={showPreview} />
          {markdown && (
            <Button
              size="xs"
              variant={showPreview ? "secondary" : "ghost"}
              aria-pressed={showPreview}
              disabled={content === null}
              onClick={() => setPreview((p) => !p)}
            >
              <Eye /> Preview
            </Button>
          )}
          <CopyButton text={file?.change === "deleted" ? null : content} />
          <Button size="icon-sm" variant="ghost" title="Refresh" onClick={refresh} disabled={loading}>
            <RefreshCw className={cn(loading && "animate-spin")} />
          </Button>
        </div>
      </div>
      <section className="min-h-0 flex-1 overflow-auto" data-testid="diff-patch">
        {error && (
          <p className="text-destructive p-4 text-xs" role="alert" data-testid="diff-error">
            {error}
          </p>
        )}
        {viewed === null && !error && <Loading />}
        {viewed?.kind === "gone" && <Note>This file is gone.</Note>}
        {viewed?.kind === "binary" && <Note>Binary file — not shown</Note>}
        {viewed?.kind === "too_large" && <Note>{TOO_LARGE}</Note>}
        {viewed?.kind === "lines" && viewed.file === null && (
          <p className="text-muted-foreground border-b px-4 py-2 text-xs">
            No uncommitted changes — showing the file as it is now.
          </p>
        )}
        {showPreview && <Markdown text={content} className="max-w-3xl px-6 py-4" />}
        {viewed?.kind === "lines" && !showPreview && (
          <FileChanges
            lines={viewed.lines}
            layout={layout}
            opened={opened}
            onReveal={(at, more) => setOpened((o) => ({ ...o, [at]: (o[at] ?? 0) + more }))}
          />
        )}
      </section>
    </div>
  );
}

function Note({ children }: { children: React.ReactNode }) {
  return <p className="text-muted-foreground p-4 text-xs">{children}</p>;
}

/** Unified or Split, as one segmented control. */
function LayoutToggle({
  layout,
  onChange,
  disabled,
}: {
  layout: DiffLayout;
  onChange(next: DiffLayout): void;
  disabled: boolean;
}) {
  const options = [
    { value: "unified", label: "Unified", Icon: Rows2 },
    { value: "split", label: "Split", Icon: Columns2 },
  ] as const;
  return (
    <div className="bg-muted flex items-center rounded-md p-0.5" role="group" aria-label="Layout">
      {options.map(({ value, label, Icon }) => (
        <button
          key={value}
          className={cn(
            "text-muted-foreground flex h-6 items-center gap-1 rounded-sm px-2 text-xs transition-colors disabled:opacity-50",
            layout === value ? "bg-background text-foreground shadow-xs" : "hover:text-foreground",
          )}
          aria-pressed={layout === value}
          disabled={disabled}
          onClick={() => onChange(value)}
        >
          <Icon className="size-3.5" /> {label}
        </button>
      ))}
    </div>
  );
}

/** Copies the file as it is now; says so for a moment. */
function CopyButton({ text }: { text: string | null }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <Button
      size="xs"
      variant="ghost"
      title="Copy file"
      disabled={text === null}
      onClick={() => {
        if (text === null) return;
        void navigator.clipboard?.writeText(text);
        setCopied(true);
      }}
    >
      {copied ? <Check /> : <Copy />} {copied ? "Copied" : "Copy"}
    </Button>
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
