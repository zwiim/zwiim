import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ExternalLink, ChevronRight, Eye, Loader2, Pencil, RefreshCw } from "lucide-react";
import { FolderIcon, fileIcon } from "@/components/FileIcon";
import {
  delimitedEditable,
  delimiterFor,
  imageMime,
  parseDelimited,
  previewErrorMessage,
  previewKind,
  serializeDelimited,
  type DelimitedTable,
} from "@zwiim/preview";
import { readSheet, type Sheet } from "@zwiim/preview/sheet";
import { renderWord } from "@zwiim/preview/word";
import {
  hasCapability,
  isMarkdown,
  readChunks,
  readFile,
  ZwiimClientError,
  type FileContent,
  type FileTreeNode,
} from "@zwiim/sdk";
import { Markdown } from "@/components/Markdown";
import { Button } from "@/components/ui/button";
import { isTauri, openFileBytes } from "@/lib/bridge";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { markdownStyle } from "@/lib/prefs";
import { usePref } from "@/lib/use-pref";
import { cn } from "@/lib/utils";
import { ListSkeleton } from "./DiffPanel";
import { HtmlView } from "./preview/HtmlView";
import { PdfView } from "./preview/PdfView";
import { PreviewFailure, PreviewLoading } from "./preview/PreviewState";
import { SheetEditor } from "./preview/SheetEditor";
import { SheetView } from "./preview/SheetView";
import { WordView } from "./preview/WordView";

/** A directory listing: loaded, or why it could not be. */
type Listing = { nodes: FileTreeNode[] } | { error: string; missing?: boolean };

/**
 * Read-only workspace browser; the tree comes from the host. Opening a file
 * shows it beside the conversation.
 */
export function FilesPanel({
  workspaceId,
  selected,
  onOpen,
}: {
  workspaceId: string;
  /** The file showing, if any. */
  selected: string | null;
  onOpen(path: string): void;
}) {
  const controller = useController();
  const [listings, setListings] = useState<Record<string, Listing>>({});
  const [open, setOpen] = useState<Set<string>>(new Set());

  const load = useCallback(
    async (path: string) => {
      try {
        const nodes = await controller.client.request("workspace.tree", {
          workspace_id: workspaceId,
          ...(path ? { path } : {}),
          max_depth: 1,
        });
        setListings((c) => ({ ...c, [path]: { nodes } }));
        return nodes;
      } catch (e) {
        // The workspace's own folder is gone: not an error to read, a state to explain.
        const missing = !path && e instanceof ZwiimClientError && e.code === "not_found";
        setListings((c) => ({ ...c, [path]: { error: errorMessage(e), missing } }));
        return null;
      }
    },
    [controller, workspaceId],
  );

  const reset = useCallback(() => {
    setListings({});
    setOpen(new Set());
    void load("");
  }, [load]);

  useEffect(() => reset(), [reset]);

  // The workspace's files changed: re-list what is showing, in place.
  const revision = useHost((s) => s.diffRevision[workspaceId] ?? 0);
  const shown = useRef({ open, listings });
  shown.current = { open, listings };
  const seen = useRef(revision);
  useEffect(() => {
    if (seen.current === revision) return;
    seen.current = revision;
    const { open, listings } = shown.current;
    for (const path of ["", ...open]) if (listings[path]) void load(path);
  }, [revision, load]);

  function expand(path: string) {
    setOpen((o) => new Set(o).add(path));
    const listing = shown.current.listings[path];
    if (listing && !("error" in listing)) return;
    setListings(({ [path]: _, ...rest }) => rest);
    // A folder holding only a folder opens straight through to it, but not
    // through a link: one to a folder above would open forever.
    void load(path).then((nodes) => {
      const only = nodes?.length === 1 ? nodes[0]! : null;
      if (only?.kind === "dir" && !only.symlink) expand(only.path);
    });
  }

  function close(path: string) {
    setOpen((o) => {
      const next = new Set(o);
      next.delete(path);
      return next;
    });
  }

  /** An open folder whose only entry is a folder (not a link) shares its row: `a / b`. */
  function chainOf(node: FileTreeNode): FileTreeNode[] {
    const chain = [node];
    for (let at = node; open.has(at.path);) {
      const listing = listings[at.path];
      const only = listing && !("error" in listing) && listing.nodes.length === 1 ? listing.nodes[0]! : null;
      if (only?.kind !== "dir" || only.symlink) break;
      at = only;
      chain.push(at);
    }
    return chain;
  }

  const renderListing = (path: string, depth: number): React.ReactNode => {
    const listing = listings[path];
    // Level with the file icons beside it.
    const pad = { paddingLeft: depth * 14 + 24 };
    if (!listing) {
      return <ListSkeleton rows={path ? 2 : 6} className="pr-2" style={{ paddingLeft: depth * 14 + 4 }} />;
    }
    if ("error" in listing && listing.missing) {
      return (
        <div className="text-muted-foreground flex flex-col gap-1 px-3 py-2 text-xs" role="status">
          <p className="text-foreground font-medium">This folder is no longer on the host</p>
          <p>
            It was moved, renamed or deleted. Put it back, or add it again from its new place with + in the sidebar.
          </p>
        </div>
      );
    }
    if ("error" in listing) {
      return (
        <div className="flex items-center gap-2 py-0.5 pr-2 text-xs" style={pad}>
          <p className="text-destructive min-w-0 flex-1" role="alert">
            {listing.error}
          </p>
          <Button size="xs" variant="outline" onClick={() => (path ? expand(path) : reset())}>
            Retry
          </Button>
        </div>
      );
    }
    if (listing.nodes.length === 0) {
      return (
        <p className="text-muted-foreground py-0.5 text-xs" style={pad}>
          {path ? "Empty folder" : "This workspace is empty"}
        </p>
      );
    }
    return listing.nodes.map((n) => {
      const row =
        "hover:bg-muted flex h-7 w-full items-center gap-1.5 rounded-md px-1 text-left text-ui transition-colors";
      const indent = { paddingLeft: depth * 14 + 4 };
      if (n.kind !== "dir") {
        const { icon: Icon, color } = fileIcon(n.name);
        return (
          <button
            key={n.path}
            className={cn(row, selected === n.path && "bg-accent font-medium")}
            style={indent}
            onClick={() => onOpen(n.path)}
          >
            <Icon className={cn("ml-5 size-4 shrink-0", color)} />
            <span className="truncate">{n.name}</span>
          </button>
        );
      }
      const chain = chainOf(n);
      const last = chain[chain.length - 1]!;
      const expanded = open.has(last.path);
      return (
        <div key={n.path}>
          <button className={row} style={indent} onClick={() => (expanded ? close(n.path) : expand(last.path))}>
            {expanded ? (
              <ChevronDown className="text-muted-foreground size-3.5 shrink-0" />
            ) : (
              <ChevronRight className="text-muted-foreground size-3.5 shrink-0" />
            )}
            <FolderIcon name={last.name} open={expanded} />
            <span className="truncate">{chain.map((c) => c.name).join(" / ")}</span>
          </button>
          {expanded && renderListing(last.path, depth + 1)}
        </div>
      );
    });
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center justify-between px-3 py-1">
        <span className="text-muted-foreground text-xs">Workspace files</span>
        <Button size="icon-sm" variant="ghost" title="Refresh" onClick={reset}>
          <RefreshCw />
        </Button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto px-1 pb-1" data-testid="file-tree">
        {renderListing("", 0)}
      </div>
    </div>
  );
}

export { fileIcon } from "@/components/FileIcon";

type Shown =
  | { text: FileContent }
  | { image: string }
  | { sheets: Sheet[] }
  | { word: string }
  | { pdf: true }
  | { html: true }
  | { none: true };

/** One file's contents, read-only, in its own tab; what has no preview opens in its own app. */
export function FileView({ workspaceId, path }: { workspaceId: string; path: string }) {
  const controller = useController();
  const [shown, setShown] = useState<Shown | null>(null);
  const [failure, setFailure] = useState<unknown>(null);
  const [loading, setLoading] = useState(false);
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState<string | null>(null);
  const [reloads, setReloads] = useState(0);
  const loads = useRef(0);
  const imageUrl = useRef<string | null>(null);
  const name = path.slice(path.lastIndexOf("/") + 1);
  const kind = previewKind(name);
  // A page renders only in the app, where it gets its own origin.
  const page = kind === "html" && isTauri();
  const [mode, setMode] = useState<"rendered" | "source">("rendered");
  // A CSV/TSV being edited: the table, and the text and etag it was read at.
  const [editing, setEditing] = useState<{ table: DelimitedTable; source: string; etag: string } | null>(null);
  const [saving, setSaving] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);
  const [stale, setStale] = useState(false);
  const delimiter = delimiterFor(name);
  const canWrite = useHost((s) => hasCapability(s.connection.compat, "files.write"));
  const dirty = useMemo(() => !!editing && serializeDelimited(editing.table) !== editing.source, [editing]);
  const [preview, setPreview] = useState(false);
  const book = usePref(markdownStyle) === "book";

  const refresh = useCallback(async () => {
    const load = ++loads.current;
    setFailure(null);
    if (kind === "external") return setShown({ none: true });
    if (kind === "pdf" || (page && mode === "rendered")) {
      setShown(kind === "pdf" ? { pdf: true } : { html: true });
      setReloads((n) => n + 1);
      return;
    }
    setLoading(true);
    try {
      let next: Shown;
      if (kind === "image" || kind === "sheet" || kind === "word") {
        const bytes = await readFile(controller.client, workspaceId, path);
        if (kind === "sheet") next = { sheets: readSheet(bytes, name).sheets };
        else if (kind === "word") next = { word: await renderWord(bytes) };
        else {
          // A read overtaken by a newer one must not revoke the URL that one shows.
          if (load !== loads.current) return;
          if (imageUrl.current) URL.revokeObjectURL(imageUrl.current);
          imageUrl.current = URL.createObjectURL(new Blob([bytes as BlobPart], { type: imageMime(path) }));
          next = { image: imageUrl.current };
        }
      } else {
        next = { text: await controller.client.request("files.read", { workspace_id: workspaceId, path }) };
      }
      if (load === loads.current) setShown(next);
    } catch (e) {
      if (load === loads.current) {
        setShown(null);
        setFailure(e);
      }
    } finally {
      if (load === loads.current) setLoading(false);
    }
  }, [controller, workspaceId, path, kind, name, page, mode]);

  // Once per file; the header's Reload reads it again. Text alone also reloads
  // when the workspace's files change.
  const revision = useHost((s) => (kind === "text" ? (s.diffRevision[workspaceId] ?? 0) : 0));
  useEffect(() => {
    void refresh();
  }, [refresh, revision]);
  useEffect(
    () => () => {
      if (imageUrl.current) URL.revokeObjectURL(imageUrl.current);
    },
    [],
  );
  const onViewError = useCallback((e: unknown) => setFailure(e), []);

  async function openElsewhere() {
    setOpening(true);
    setOpenError(null);
    try {
      const chunks = readChunks(controller.client, workspaceId, path);
      const first = await chunks.next();
      if (first.done) return;
      async function* bytes() {
        yield first.value!.bytes;
        for await (const c of chunks) yield c.bytes;
      }
      await openFileBytes(first.value.name, first.value.mime_type, bytes());
    } catch (e) {
      setOpenError(previewErrorMessage(e, name) ?? errorMessage(e));
    } finally {
      setOpening(false);
    }
  }

  async function startEdit() {
    if (!delimiter) return;
    setEditError(null);
    try {
      const file = await controller.client.request("files.read", { workspace_id: workspaceId, path });
      const table = parseDelimited(file.content, delimiter);
      if (!delimitedEditable(table))
        return setEditError("Only files up to 1,000 rows and 100 columns can be edited here.");
      setEditing({ table, source: file.content, etag: file.etag });
    } catch (e) {
      setEditError(errorMessage(e));
    }
  }

  function stopEdit() {
    setEditing(null);
    setEditError(null);
    setStale(false);
  }

  async function save() {
    if (!editing) return;
    setSaving(true);
    setEditError(null);
    try {
      const content = serializeDelimited(editing.table);
      await controller.client.request("files.write", { workspace_id: workspaceId, path, content, etag: editing.etag });
      stopEdit();
      void refresh();
    } catch (e) {
      if (e instanceof ZwiimClientError && e.code === "file_changed") setStale(true);
      else setEditError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  const sheet = shown && "sheets" in shown ? shown.sheets[0] : undefined;
  const editable = !!delimiter && !!sheet && !sheet.truncated && canWrite;
  const markdown = isMarkdown(path) && !!shown && "text" in shown;

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="file-view">
      <div className="text-muted-foreground flex h-10 shrink-0 items-center gap-2 border-b px-4 text-xs">
        <span className="text-foreground min-w-0 flex-1 truncate font-mono">{path}</span>
        {page && (
          <div className="bg-muted flex shrink-0 gap-0.5 rounded-lg p-0.5" role="radiogroup" aria-label="Show">
            {(["rendered", "source"] as const).map((m) => (
              <button
                key={m}
                className={cn(
                  "h-6 rounded-md px-2 transition-colors",
                  mode === m
                    ? "bg-background text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground",
                )}
                role="radio"
                aria-checked={mode === m}
                onClick={() => setMode(m)}
              >
                {m === "rendered" ? "Rendered" : "Source"}
              </button>
            ))}
          </div>
        )}
        {markdown && (
          <Button
            size="xs"
            variant={preview ? "secondary" : "ghost"}
            aria-pressed={preview}
            onClick={() => setPreview((p) => !p)}
          >
            <Eye /> Preview
          </Button>
        )}
        {shown && "text" in shown && <span className="shrink-0">read-only</span>}
        <Button
          size="icon-sm"
          variant="ghost"
          title="Open in default app"
          disabled={opening}
          onClick={() => void openElsewhere()}
        >
          <ExternalLink />
        </Button>
        {editing ? (
          <>
            <Button size="xs" variant="ghost" onClick={stopEdit} disabled={saving}>
              Cancel
            </Button>
            <Button size="xs" variant="brand" onClick={() => void save()} disabled={!dirty || saving}>
              {saving && <Loader2 className="animate-spin" />} Save
            </Button>
          </>
        ) : (
          <>
            {editable && (
              <Button size="xs" variant="ghost" onClick={() => void startEdit()} disabled={loading}>
                <Pencil /> Edit
              </Button>
            )}
            <Button size="icon-sm" variant="ghost" title="Reload" onClick={refresh} disabled={loading}>
              <RefreshCw className={cn(loading && "animate-spin")} />
            </Button>
          </>
        )}
      </div>
      {stale && (
        <div className="flex items-center gap-3 border-b px-4 py-2 text-xs" role="alert">
          <p className="text-destructive flex-1">
            This file changed on disk since you started editing. Reload to see the latest version; your edits will be
            lost.
          </p>
          <Button
            size="xs"
            variant="outline"
            onClick={() => {
              stopEdit();
              void refresh();
            }}
          >
            Reload
          </Button>
        </div>
      )}
      {editError && (
        <p className="text-destructive px-4 pt-3 text-xs" role="alert">
          {editError}
        </p>
      )}
      {openError && (
        <p className="text-destructive px-4 pt-3 text-xs" role="alert" data-testid="open-error">
          {openError}
        </p>
      )}
      {failure ? (
        <PreviewFailure error={failure} name={name} opening={opening} onOpen={() => void openElsewhere()} />
      ) : shown && "none" in shown ? (
        <div
          className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center"
          data-testid="file-no-preview"
        >
          <p className="text-muted-foreground text-xs">
            No preview for <span className="text-foreground font-mono">{name}</span> here yet.
          </p>
          <Button size="sm" variant="outline" disabled={opening} onClick={() => void openElsewhere()}>
            {opening ? <Loader2 className="animate-spin" /> : <ExternalLink />} Open in default app
          </Button>
        </div>
      ) : shown && "pdf" in shown ? (
        <PdfView key={reloads} workspaceId={workspaceId} path={path} onError={onViewError} />
      ) : shown && "html" in shown ? (
        <HtmlView workspaceId={workspaceId} path={path} reloadKey={reloads} onError={onViewError} />
      ) : editing ? (
        <SheetEditor table={editing.table} onChange={(table) => setEditing((e) => e && { ...e, table })} />
      ) : shown && "sheets" in shown ? (
        <SheetView sheets={shown.sheets} />
      ) : shown && "word" in shown ? (
        <WordView html={shown.word} />
      ) : shown && "image" in shown ? (
        <div className="min-h-0 flex-1 overflow-auto p-4">
          <img className="max-w-full rounded-md" src={shown.image} alt={name} data-testid="file-image" />
        </div>
      ) : shown && preview && markdown ? (
        <div className="min-h-0 flex-1 overflow-auto" data-testid="file-markdown">
          <Markdown
            text={shown.text.content}
            className={book ? "markdown-book px-8 py-10" : "max-w-3xl px-6 py-4"}
          />
        </div>
      ) : shown ? (
        <FileText text={shown.text.content} />
      ) : (
        <PreviewLoading />
      )}
    </div>
  );
}

function FileText({ text }: { text: string }) {
  const lines = (text.endsWith("\n") ? text.slice(0, -1) : text).split(/\r?\n/);
  return (
    <div className="min-h-0 flex-1 overflow-auto" data-testid="file-content">
      <table className="border-collapse font-mono text-xs leading-5">
        <tbody>
          {lines.map((line, i) => (
            <tr key={i}>
              <td className="text-muted-foreground border-r px-2 text-right align-top select-none">{i + 1}</td>
              <td className="px-3 whitespace-pre">{line || " "}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
