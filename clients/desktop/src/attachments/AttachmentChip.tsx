import { useContext, useEffect, useState } from "react";
import { CircleAlert, Loader2, X } from "lucide-react";
import { formatSize, type AttachmentBlock } from "@zwiim/sdk";
import { FileLinksContext } from "@/components/Markdown";
import { fileIcon } from "@/panels/FilesPanel";
import { cn } from "@/lib/utils";
import type { PendingFile } from "./pending";

const CHIP = "bg-muted flex h-7 max-w-56 min-w-0 items-center gap-1.5 rounded-md px-2 text-xs";

/** An image's thumbnail while it is pending; null where the platform has no object URLs. */
function useThumbnail(file: PendingFile): string | null {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!file.mime_type.startsWith("image/") || typeof URL.createObjectURL !== "function") return;
    let u: string;
    try {
      u = URL.createObjectURL(file.file);
    } catch {
      return; // the icon stands in
    }
    setUrl(u);
    return () => URL.revokeObjectURL(u);
  }, [file.file, file.mime_type]);
  return url;
}

/** A file chosen for the next message: uploading, failed, or waiting to send. */
export function PendingChip({ file, disabled, onRemove }: { file: PendingFile; disabled?: boolean; onRemove(): void }) {
  const { icon: Icon, color } = fileIcon(file.name);
  const thumbnail = useThumbnail(file);
  const failed = file.status === "failed";
  return (
    <span
      className={cn(CHIP, thumbnail && "pl-0", failed && "text-destructive ring-destructive/40 ring-1")}
      title={failed ? `Upload failed: ${file.error ?? "unknown error"}` : `${file.name} · ${formatSize(file.size)}`}
      data-testid="pending-attachment"
    >
      {file.status === "uploading" ? (
        <Loader2 className="text-muted-foreground size-3.5 shrink-0 animate-spin" aria-label="Uploading" />
      ) : failed ? (
        <CircleAlert className="size-3.5 shrink-0" />
      ) : thumbnail ? (
        <img className="size-7 shrink-0 rounded-l-md object-cover" src={thumbnail} alt="" />
      ) : (
        <Icon className={cn("size-3.5 shrink-0", color)} />
      )}
      <span className="min-w-0 truncate font-mono">{file.name}</span>
      <span className={cn("shrink-0", !failed && "text-muted-foreground")}>
        {failed ? "Upload failed" : formatSize(file.size)}
      </span>
      <button
        type="button"
        className="text-muted-foreground hover:text-foreground shrink-0 disabled:opacity-50"
        aria-label={`Remove ${file.name}`}
        disabled={disabled || file.status === "uploading"}
        onClick={onRemove}
      >
        <X className="size-3" />
      </button>
    </span>
  );
}

/** A file sent with a message; opens in a tab when its workspace is still there. */
export function SentChip({ block }: { block: AttachmentBlock }) {
  const links = useContext(FileLinksContext);
  const { icon: Icon, color } = fileIcon(block.name);
  const gone = block.size === undefined;
  const content = (
    <>
      <Icon className={cn("size-3.5 shrink-0", color)} />
      <span className="text-foreground/90 min-w-0 truncate font-mono">{block.name}</span>
      <span className="text-muted-foreground shrink-0">{gone ? "removed" : formatSize(block.size!)}</span>
    </>
  );
  return links && !gone ? (
    <button
      type="button"
      className={cn(CHIP, "hover:bg-accent hover:text-foreground transition-colors")}
      title={block.path}
      onClick={() => links.open(block.path)}
      data-testid="sent-attachment"
    >
      {content}
    </button>
  ) : (
    <span
      className={CHIP}
      title={gone ? `${block.name} is no longer in the workspace` : block.path}
      data-testid="sent-attachment"
    >
      {content}
    </span>
  );
}

/** The files a user message carried, above its text. */
export function SentAttachments({ blocks, className }: { blocks: readonly { type: string }[]; className?: string }) {
  const files = blocks.filter((b): b is AttachmentBlock => b.type === "attachment");
  if (files.length === 0) return null;
  return (
    <div className={cn("flex max-w-[85%] flex-wrap justify-end gap-1.5 self-end", className)} data-testid="sent-attachments">
      {files.map((b) => (
        <SentChip key={b.attachment_id} block={b} />
      ))}
    </div>
  );
}

/** The pending files under a composer's message, with what was refused. */
export function PendingAttachments({
  files,
  errors,
  disabled,
  onRemove,
  className,
}: {
  files: readonly PendingFile[];
  errors: readonly string[];
  disabled?: boolean;
  onRemove(id: string): void;
  className?: string;
}) {
  if (files.length === 0 && errors.length === 0) return null;
  return (
    <div className={cn("flex flex-col gap-1.5", className)}>
      {files.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5" data-testid="pending-attachments">
          {files.map((f) => (
            <PendingChip key={f.id} file={f} disabled={disabled} onRemove={() => onRemove(f.id)} />
          ))}
        </div>
      )}
      {errors.map((e) => (
        <p key={e} className="text-destructive text-xs" role="alert">
          {e}
        </p>
      ))}
    </div>
  );
}
