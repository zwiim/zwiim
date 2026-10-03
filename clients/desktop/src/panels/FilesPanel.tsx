import { useCallback, useEffect, useRef, useState } from "react";
import {
  Braces,
  ChevronDown,
  ChevronRight,
  File,
  FileArchive,
  FileCodeCorner,
  FileCog,
  FileImage,
  FileKey,
  FileText as FileTextIcon,
  Folder,
  FolderOpen,
  GitBranch,
  Loader2,
  Lock,
  RefreshCw,
  SquareTerminal,
  type LucideIcon,
} from "lucide-react";
import { ZwiimClientError, type FileContent, type FileTreeNode } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost } from "@/lib/host-context";
import { cn } from "@/lib/utils";

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
    for (let at = node; open.has(at.path); ) {
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
      return (
        <p className="text-muted-foreground flex items-center gap-1 py-0.5 text-xs" style={pad}>
          <Loader2 className="size-3 animate-spin" /> Loading…
        </p>
      );
    }
    if ("error" in listing && listing.missing) {
      return (
        <div className="text-muted-foreground flex flex-col gap-1 px-3 py-2 text-xs" role="status">
          <p className="text-foreground font-medium">This folder is no longer on the host</p>
          <p>It was moved, renamed or deleted. Put it back, or add it again from its new place with + in the sidebar.</p>
        </div>
      );
    }
    if ("error" in listing) {
      return (
        <p className="text-destructive py-0.5 text-xs" style={pad} role="alert">
          {listing.error}
        </p>
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
      const row = "hover:bg-muted flex w-full items-center gap-1.5 rounded-md px-1 py-[3px] text-left text-[13px]";
      const indent = { paddingLeft: depth * 14 + 4 };
      if (n.kind !== "dir") {
        const { icon: Icon, color } = fileIcon(n.name);
        return (
          <button
            key={n.path}
            className={cn(row, selected === n.path && "bg-muted font-medium")}
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
          <button
            className={row}
            style={indent}
            onClick={() => (expanded ? close(n.path) : expand(last.path))}
          >
            {expanded ? (
              <ChevronDown className="text-muted-foreground size-3.5 shrink-0" />
            ) : (
              <ChevronRight className="text-muted-foreground size-3.5 shrink-0" />
            )}
            {expanded ? (
              <FolderOpen className="text-muted-foreground size-4 shrink-0" />
            ) : (
              <Folder className="text-muted-foreground size-4 shrink-0" />
            )}
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

const BY_EXTENSION: Record<string, { icon: LucideIcon; color: string }> = {};
const kind = (icon: LucideIcon, color: string, extensions: string) => {
  for (const ext of extensions.split(" ")) BY_EXTENSION[ext] = { icon, color };
};
kind(Braces, "text-yellow-600 dark:text-yellow-300", "json jsonc json5");
kind(FileTextIcon, "text-sky-600 dark:text-sky-400", "md mdx markdown txt rst");
kind(FileCodeCorner, "text-blue-600 dark:text-blue-400", "ts tsx mts cts");
kind(FileCodeCorner, "text-yellow-600 dark:text-yellow-400", "js jsx mjs cjs");
kind(FileCodeCorner, "text-orange-600 dark:text-orange-400", "rs swift html");
kind(FileCodeCorner, "text-teal-600 dark:text-teal-400", "py go rb");
kind(FileCodeCorner, "text-violet-600 dark:text-violet-400", "kt kts java scala");
kind(FileCodeCorner, "text-pink-600 dark:text-pink-400", "css scss sass less");
kind(SquareTerminal, "text-emerald-600 dark:text-emerald-400", "sh bash zsh fish ps1");
kind(FileCog, "text-rose-600 dark:text-rose-400", "yml yaml toml ini conf cfg xml plist");
kind(FileImage, "text-emerald-600 dark:text-emerald-400", "png jpg jpeg gif svg webp ico icns avif");
kind(FileArchive, "text-amber-600 dark:text-amber-400", "zip tar gz tgz xz dmg apk aab");
kind(FileKey, "text-red-600 dark:text-red-400", "sig pem key pub crt minisig");

/** A file's type at a glance: icon and color by name, as an editor's tree shows them. */
export function fileIcon(name: string): { icon: LucideIcon; color: string } {
  const lower = name.toLowerCase();
  if (/(\.lockb?|-lock\.(json|yaml))$/.test(lower)) return { icon: Lock, color: "text-muted-foreground" };
  if (lower.startsWith(".git")) return { icon: GitBranch, color: "text-orange-600 dark:text-orange-400" };
  if (lower.startsWith(".env")) return BY_EXTENSION.toml!;
  const dot = lower.lastIndexOf(".");
  return (dot > 0 && BY_EXTENSION[lower.slice(dot + 1)]) || { icon: File, color: "text-muted-foreground" };
}

/** One file's contents, read-only, in its own tab. */
export function FileView({ workspaceId, path }: { workspaceId: string; path: string }) {
  const controller = useController();
  const [file, setFile] = useState<FileContent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const loads = useRef(0);

  const refresh = useCallback(async () => {
    const load = ++loads.current;
    setError(null);
    setLoading(true);
    try {
      const content = await controller.client.request("files.read", { workspace_id: workspaceId, path });
      if (load === loads.current) setFile(content);
    } catch (e) {
      if (load === loads.current) {
        setFile(null);
        setError(errorMessage(e));
      }
    } finally {
      if (load === loads.current) setLoading(false);
    }
  }, [controller, workspaceId, path]);

  // Again when the workspace's files change.
  const revision = useHost((s) => s.diffRevision[workspaceId] ?? 0);
  useEffect(() => {
    void refresh();
  }, [refresh, revision]);

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="file-view">
      <div className="text-muted-foreground flex h-10 shrink-0 items-center gap-2 border-b px-4 text-xs">
        <span className="text-foreground truncate font-mono">{path}</span>
        {file && <span className="ml-auto shrink-0">read-only</span>}
        <Button
          className={cn(!file && "ml-auto")}
          size="icon-sm"
          variant="ghost"
          title="Reload"
          onClick={refresh}
          disabled={loading}
        >
          <RefreshCw className={cn(loading && "animate-spin")} />
        </Button>
      </div>
      {error ? (
        <p className="text-destructive p-4 text-xs" role="alert" data-testid="file-error">
          {error}
        </p>
      ) : file ? (
        <FileText text={file.content} />
      ) : (
        <p className="text-muted-foreground flex items-center gap-1.5 p-4 text-xs">
          <Loader2 className="size-3 animate-spin" /> Opening…
        </p>
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
