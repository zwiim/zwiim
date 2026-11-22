import { useMemo } from "react";
import { Zap } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { WorkingMark } from "@/components/WorkingMark";
import { useHost } from "@/lib/host-context";
import { cn } from "@/lib/utils";

/** The sidebar's way to the Automations canvas, with what waits on the person. */
export function AutomationsRow({ active, onOpen }: { active: boolean; onOpen(): void }) {
  const approvals = useHost((s) => s.approvals);
  const runItems = useHost((s) => s.runItems);
  const runs = useHost((s) => s.runs);
  const need = useMemo(() => {
    let questions = 0;
    for (const items of Object.values(runItems)) {
      for (const item of Object.values(items)) if (item.status === "waiting_person") questions += 1;
    }
    return Object.keys(approvals).length + questions;
  }, [approvals, runItems]);
  const working = useMemo(
    () => Object.values(runs).some((r) => r.status === "running" || r.status === "discovering"),
    [runs],
  );

  return (
    <button
      className={cn(
        "hover:bg-sidebar-accent text-ui flex items-center gap-2 rounded-md px-2 py-1.5 text-left font-medium transition-colors",
        active && "bg-sidebar-accent",
      )}
      aria-current={active || undefined}
      data-testid="automations-row"
      onClick={onOpen}
    >
      <Zap className="text-muted-foreground size-4 shrink-0" />
      <span className="min-w-0 flex-1 truncate">Automations</span>
      {need > 0 ? (
        <Badge variant="warning" className="font-mono tabular-nums" aria-label={`${need} need you`}>
          {need}
        </Badge>
      ) : (
        working && <WorkingMark />
      )}
    </button>
  );
}
