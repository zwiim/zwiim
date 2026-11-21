import { WorkingMark } from "@/components/WorkingMark";
import { cn } from "@/lib/utils";
import type { Dot } from "./status";

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
