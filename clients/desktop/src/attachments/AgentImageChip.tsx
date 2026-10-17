import { useContext } from "react";
import { FileLinksContext } from "@/lib/file-links";
import { fileIcon } from "@/panels/FilesPanel";
import { cn } from "@/lib/utils";

const CHIP = "bg-muted inline-flex h-7 max-w-56 min-w-0 items-center gap-1.5 rounded-md px-2 align-middle text-xs";

/**
 * An image the agent produced, in the workspace: its icon and name; opens the
 * preview, which shows the picture. `compact` fits it in a tool call's row.
 * Spans and buttons only: it sits inside Markdown paragraphs.
 */
export function AgentImageChip({ path, name, compact }: { path: string; name?: string; compact?: boolean }) {
  const links = useContext(FileLinksContext);
  const label = name ?? path.split("/").pop() ?? path;
  const { icon: Icon, color } = fileIcon(path);
  return (
    <span className={cn("inline-flex", compact && "min-w-0")} data-testid="agent-image">
      <button
        type="button"
        className={cn(
          CHIP,
          compact && "h-5 min-w-0 gap-1 px-1.5",
          "hover:bg-accent hover:text-foreground transition-colors",
        )}
        title={path}
        aria-label={`Open ${label}`}
        disabled={!links}
        onClick={() => links?.open(path)}
      >
        <Icon className={cn("size-3.5 shrink-0", color)} />
        <span className="text-foreground/90 min-w-0 truncate font-mono">{label}</span>
      </button>
    </span>
  );
}
