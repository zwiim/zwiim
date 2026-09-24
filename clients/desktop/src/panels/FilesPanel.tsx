import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronRight, File, Folder, Loader2, RefreshCw } from "lucide-react";
import type { FileContent, FileTreeNode } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";
import { cn } from "@/lib/utils";

/** A directory listing: loaded, or why it could not be. */
type Listing = { nodes: FileTreeNode[] } | { error: string };

/** Read-only workspace browser: the tree and file contents come from the host. */
export function FilesPanel({ workspaceId }: { workspaceId: string }) {
  const controller = useController();
  const [listings, setListings] = useState<Record<string, Listing>>({});
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [file, setFile] = useState<FileContent | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const selectedRef = useRef<string | null>(null);

  const load = useCallback(
    async (path: string) => {
      try {
        const nodes = await controller.client.request("workspace.tree", {
          workspace_id: workspaceId,
          ...(path ? { path } : {}),
          max_depth: 1,
        });
        setListings((c) => ({ ...c, [path]: { nodes } }));
      } catch (e) {
        setListings((c) => ({ ...c, [path]: { error: errorMessage(e) } }));
      }
    },
    [controller, workspaceId],
  );

  const reset = useCallback(() => {
    setListings({});
    setOpen(new Set());
    void load("");
  }, [load]);

  useEffect(() => {
    selectedRef.current = null;
    setSelected(null);
    setFile(null);
    setFileError(null);
    reset();
  }, [reset]);

  async function toggle(node: FileTreeNode) {
    if (node.kind === "dir") {
      const next = new Set(open);
      if (next.has(node.path)) next.delete(node.path);
      else {
        next.add(node.path);
        if (!listings[node.path] || "error" in listings[node.path]!) {
          setListings(({ [node.path]: _, ...rest }) => rest);
          void load(node.path);
        }
      }
      setOpen(next);
      return;
    }
    selectedRef.current = node.path;
    setSelected(node.path);
    setFile(null);
    setFileError(null);
    try {
      const content = await controller.client.request("files.read", { workspace_id: workspaceId, path: node.path });
      if (selectedRef.current === node.path) setFile(content);
    } catch (e) {
      if (selectedRef.current === node.path) setFileError(errorMessage(e));
    }
  }

  const renderListing = (path: string, depth: number): React.ReactNode => {
    const listing = listings[path];
    const pad = { paddingLeft: depth * 12 + 20 };
    if (!listing) {
      return (
        <p className="text-muted-foreground flex items-center gap-1 py-0.5 text-xs" style={pad}>
          <Loader2 className="size-3 animate-spin" /> Loading…
        </p>
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
    return listing.nodes.map((n) => (
      <div key={n.path}>
        <button
          className={cn(
            "hover:bg-muted flex w-full items-center gap-1 rounded px-1 py-0.5 text-left text-xs",
            selected === n.path && "bg-muted font-medium",
          )}
          style={{ paddingLeft: depth * 12 + 4 }}
          onClick={() => toggle(n)}
        >
          {n.kind === "dir" ? (
            <>
              {open.has(n.path) ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
              <Folder className="text-muted-foreground size-3.5" />
            </>
          ) : (
            <File className="text-muted-foreground ml-4 size-3.5" />
          )}
          <span className="truncate">{n.name}</span>
        </button>
        {n.kind === "dir" && open.has(n.path) && renderListing(n.path, depth + 1)}
      </div>
    ));
  };

  return (
    <div className="flex h-full min-h-0">
      <div className="flex w-72 shrink-0 flex-col border-r">
        <div className="flex items-center justify-between border-b px-3 py-1.5">
          <span className="text-muted-foreground text-xs font-medium uppercase">Files</span>
          <Button size="icon-sm" variant="ghost" title="Refresh" onClick={reset}>
            <RefreshCw />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-auto p-1" data-testid="file-tree">
          {renderListing("", 0)}
        </div>
      </div>
      <div className="flex min-w-0 flex-1 flex-col">
        {selected === null ? (
          <p className="text-muted-foreground m-auto text-xs">Select a file to view it</p>
        ) : (
          <>
            <div className="text-muted-foreground flex items-center gap-2 border-b px-3 py-1.5 text-xs">
              <span className="truncate font-mono">{selected}</span>
              {file && <span className="ml-auto shrink-0">read-only</span>}
            </div>
            {fileError ? (
              <p className="text-destructive p-3 text-xs" role="alert" data-testid="file-error">
                {fileError}
              </p>
            ) : file ? (
              <FileText text={file.content} />
            ) : (
              <p className="text-muted-foreground flex items-center gap-1.5 p-3 text-xs">
                <Loader2 className="size-3 animate-spin" /> Opening…
              </p>
            )}
          </>
        )}
      </div>
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
