import { ChevronDown, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { PlaybookRow } from "./data";
import { InputsCard } from "./InputsCard";
import { ItemTile } from "./ItemTile";
import { failureText, runCounts, sectionLine, type Dot } from "./status";
import { StatusDot } from "./StatusDot";

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
  onOpen,
  pickFile,
}: {
  row: PlaybookRow;
  open: boolean;
  /** Why the last action was refused. */
  error: string | null;
  busy: boolean;
  onToggle(): void;
  onAction(action: SectionAction): void;
  /** Open an item in the focus view. */
  onOpen(itemId: string): void;
  /** A file on the Host's computer for an input, or null when cancelled. */
  pickFile(): Promise<string | null>;
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
          <span className="text-ui truncate font-medium">
            {row.playbook.name}
          </span>
          {open ? (
            row.run && (
              <span className="text-muted-foreground font-mono text-xs tabular-nums">
                {counts.sent}/{counts.total} sent
                {counts.need > 0 && (
                  <span className="text-brand"> · {counts.need} need you</span>
                )}
                {row.run.outcome === "nothing_new" && (
                  <span className="font-sans">
                    {" "}
                    · Nothing new: every item was done before
                  </span>
                )}
                {row.run.status === "failed" && (
                  <span className="text-destructive font-sans">
                    {" "}
                    · Failed · {failureText(row.run.error)}
                  </span>
                )}
              </span>
            )
          ) : (
            <span
              className={cn(
                "truncate text-xs",
                line.need ? "text-brand" : "text-muted-foreground",
              )}
            >
              {line.text}
            </span>
          )}
        </button>
        <Button
          variant={
            action === "start" || action === "again" ? "secondary" : "ghost"
          }
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
          <div
            className="flex gap-3 overflow-x-auto pb-1 pl-5.5"
            data-testid="tile-strip"
          >
            {row.items.map((item) => (
              <ItemTile
                key={item.item_id}
                item={item}
                onOpen={() => onOpen(item.item_id)}
              />
            ))}
          </div>
        ) : (
          <p className="text-muted-foreground pl-5.5 text-xs">
            Start it to watch each item here as it runs.
          </p>
        ))}
      {open && (
        <div className="pl-5.5">
          <InputsCard
            playbookId={row.playbook.playbook_id}
            pickFile={pickFile}
          />
        </div>
      )}
    </section>
  );
}
