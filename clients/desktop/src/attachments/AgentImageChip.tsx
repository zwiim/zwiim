import { useContext, useEffect, useState } from "react";
import { isClientError, readFile, type HostClient } from "@zwiim/sdk";
import { imageMime } from "@zwiim/preview";
import { FileLinksContext } from "@/components/Markdown";
import { useOptionalController } from "@/lib/host-context";
import { fileIcon } from "@/panels/FilesPanel";
import { cn } from "@/lib/utils";

const CHIP = "bg-muted inline-flex h-7 max-w-56 min-w-0 items-center gap-1.5 rounded-md px-2 align-middle text-xs";

type Thumb = { status: "loading" } | { status: "ready"; url: string | null } | { status: "missing" };

function objectUrl(bytes: Uint8Array, path: string): string | null {
  if (typeof URL.createObjectURL !== "function") return null;
  try {
    return URL.createObjectURL(new Blob([bytes as BlobPart], { type: imageMime(path) }));
  } catch {
    return null; // the icon stands in
  }
}

/** A copy the Host named by its content hash: its bytes never change. */
const HASHED = /^\.zwiim\/images\/[0-9a-f]{16}\.[a-z]+$/;
const shared = new WeakMap<HostClient, Map<string, Promise<string | null>>>();

/** One fetch and one object URL per hashed image, however many chips show it. */
function sharedUrl(client: HostClient, workspaceId: string, path: string): Promise<string | null> {
  let urls = shared.get(client);
  if (!urls) shared.set(client, (urls = new Map()));
  const key = `${workspaceId}\0${path}`;
  let url = urls.get(key);
  if (!url) {
    url = readFile(client, workspaceId, path).then((bytes) => objectUrl(bytes, path));
    url.catch(() => urls.delete(key)); // a missing file may be written again
    urls.set(key, url);
  }
  return url;
}

/** The image's bytes from the Host as an object URL; "missing" once the file is gone. Not fetched unless `wanted`. */
function useWorkspaceImage(path: string, wanted: boolean): Thumb {
  const controller = useOptionalController();
  const links = useContext(FileLinksContext);
  const workspaceId = links?.workspace.workspace_id;
  const [thumb, setThumb] = useState<Thumb>({ status: "loading" });
  useEffect(() => {
    if (!wanted || !controller || !workspaceId) return;
    const client = controller.client;
    let owned: string | null = null;
    let live = true;
    const url = HASHED.test(path)
      ? sharedUrl(client, workspaceId, path)
      : readFile(client, workspaceId, path).then((bytes) => (live ? (owned = objectUrl(bytes, path)) : null));
    url.then(
      (u) => live && setThumb({ status: "ready", url: u }),
      (e) => live && setThumb(isClientError(e, "not_found") ? { status: "missing" } : { status: "ready", url: null }),
    );
    return () => {
      live = false;
      if (owned) URL.revokeObjectURL(owned);
    };
  }, [wanted, controller, workspaceId, path]);
  return thumb;
}

/**
 * An image the agent produced, in the workspace: thumbnail and name; opens the
 * preview. With `inline`, the image also shows under the chip; `compact`
 * fits it in a tool call's row, its icon in place of the thumbnail. Spans and buttons only: it sits inside
 * Markdown paragraphs.
 */
export function AgentImageChip({
  path,
  name,
  inline,
  compact,
}: {
  path: string;
  name?: string;
  inline?: boolean;
  compact?: boolean;
}) {
  const links = useContext(FileLinksContext);
  const label = name ?? path.split("/").pop() ?? path;
  const thumb = useWorkspaceImage(path, !compact);
  const missing = thumb.status === "missing";
  const url = thumb.status === "ready" ? thumb.url : null;
  const { icon: Icon, color } = fileIcon(path);
  return (
    <span className={cn("inline-flex flex-col items-start gap-1.5", compact && "min-w-0")} data-testid="agent-image">
      <button
        type="button"
        className={cn(
          CHIP,
          compact && "h-5 min-w-0 gap-1 px-1.5",
          url && "pl-0",
          !missing && "hover:bg-accent hover:text-foreground transition-colors",
        )}
        title={path}
        aria-label={`Open ${label}`}
        disabled={missing || !links}
        onClick={() => links?.open(path)}
      >
        {url ? (
          <img className="size-7 shrink-0 rounded-l-md object-cover" src={url} alt="" />
        ) : (
          <Icon className={cn("size-3.5 shrink-0", color)} />
        )}
        <span className="text-foreground/90 min-w-0 truncate font-mono">{label}</span>
        {missing && <span className="text-muted-foreground shrink-0">missing</span>}
      </button>
      {inline && url && (
        <button type="button" className="block" aria-label={`Preview ${label}`} onClick={() => links?.open(path)}>
          <img className="max-h-80 max-w-sm rounded-lg border" src={url} alt={label} />
        </button>
      )}
    </span>
  );
}
