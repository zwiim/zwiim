import { ChevronDown, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { WorkingMark } from "@/components/WorkingMark";
import { cn } from "@/lib/utils";
import type { PlaybookRow } from "./data";
import { itemStatus, runCounts, sectionLine, type Dot } from "./status";

const DOT: Record<Exclude<Dot, "work">, string> = {
  idle: "bg-faint",
  need: "bg-brand",
  done: "bg-done",
  off: "bg-muted-foreground/40",
  err: "bg-destructive",
};

export function StatusDot({ dot }: { dot: Dot }) {
  if (dot === "work") return <WorkingMark />;
  return (
    <span className="grid size-3.5 shrink-0 place-items-center" aria-hidden>
      <span className={cn("size-2 rounded-full", DOT[dot])} />
    </span>
  );
}

/** The dot a playbook shows: needs-you first, then its run's state. */
function sectionDot(row: PlaybookRow, need: number): Dot {
  if (need > 0) return "need";
  switch (row.run?.status) {
    case "running":
    case "discovering":
      return "work";
    case "failed":
      return "err";
    case "done":
      return "done";
    case "paused":
    case "stopped":
      return "off";
    default:
      return "idle";
  }
}

export type SectionAction = "start" | "again" | "pause" | "resume";

function actionFor(row: PlaybookRow): SectionAction {
  switch (row.run?.status) {
    case undefined:
      return "start";
    case "running":
    case "discovering":
      return "pause";
    case "paused":
      return "resume";
    default:
      return "again";
  }
}

const ACTION_LABEL: Record<SectionAction, string> = {
  start: "Start",
  again: "Run again",
  pause: "Pause",
  resume: "Resume",
};

/**
 * One playbook on the canvas. Open, it shows its latest run's items as a
 * strip; closed, one quiet line. Its one action starts, pauses or resumes.
 */
export function PlaybookSection({
  row,
  open,
  error,
  busy,
  onToggle,
  onAction,
}: {
  row: PlaybookRow;
  open: boolean;
  /** Why the last action was refused. */
  error: string | null;
  busy: boolean;
  onToggle(): void;
  onAction(action: SectionAction): void;
}) {
  const counts = runCounts(row.items);
  const line = sectionLine(row.run, counts);
  const action = actionFor(row);
  const dot = sectionDot(row, counts.need);

  return (
    <section
      className="group border-border flex flex-col gap-3 border-b py-3"
      data-testid={`playbook-${row.playbook.playbook_id}`}
    >
      <div className="flex items-center gap-2">
        <button
          className="flex min-w-0 flex-1 items-center gap-2 text-left"
          aria-expanded={open}
          onClick={onToggle}
        >
          {open ? (
            <ChevronDown className="text-muted-foreground size-3.5 shrink-0" />
          ) : (
            <ChevronRight className="text-muted-foreground size-3.5 shrink-0 opacity-0 transition-opacity duration-120 group-focus-within:opacity-100 group-hover:opacity-100" />
          )}
          <StatusDot dot={dot} />
          <span className="text-ui truncate font-medium">{row.playbook.name}</span>
          {open ? (
            row.run && (
              <span className="text-muted-foreground font-mono text-xs tabular-nums">
                {counts.sent}/{counts.total} sent
                {counts.need > 0 && <span className="text-brand"> · {counts.need} need you</span>}
              </span>
            )
          ) : (
            <span className={cn("truncate text-xs", line.need ? "text-brand" : "text-muted-foreground")}>
              {line.text}
            </span>
          )}
        </button>
        <Button
          variant={action === "start" || action === "again" ? "secondary" : "ghost"}
          size="xs"
          disabled={busy}
          className={cn(
            !open &&
              "opacity-0 transition-opacity duration-120 group-focus-within:opacity-100 group-hover:opacity-100",
          )}
          onClick={() => onAction(action)}
        >
          {ACTION_LABEL[action]}
        </Button>
      </div>
      {error && (
        <p className="text-destructive pl-5.5 text-xs" role="alert">
          {error}
        </p>
      )}
      {open &&
        (row.run ? (
          <div className="flex gap-3 overflow-x-auto pb-1 pl-5.5" data-testid="tile-strip">
            {row.items.map((item) => (
              <div key={item.item_id} className="flex w-[156px] shrink-0 flex-col gap-1">
                <span className="text-ui truncate">{item.title ?? item.item_id}</span>
                <span className="text-muted-foreground text-xs">{itemStatus(item).label}</span>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-muted-foreground pl-5.5 text-xs">Start it to watch each item here as it runs.</p>
        ))}
    </section>
  );
}
