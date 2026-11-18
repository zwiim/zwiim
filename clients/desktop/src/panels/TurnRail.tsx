import { useEffect, useRef, useState } from "react";
import { ChevronUp } from "lucide-react";
import { contextFraction, derivePlanUi, hasCapability, turnDuration, type QueuedFollowUp, type SessionView } from "@zwiim/sdk";

import { WorkingMark } from "@/components/WorkingMark";
import { useHost } from "@/lib/host-context";
import { cn } from "@/lib/utils";

import { ContextDetails } from "./ContextRing";
import { FollowUpQueue } from "./FollowUpQueue";
import { Popover, StepList } from "./PlanStrip";
import { turnStart } from "./Transcript";

type Panel = "queue" | "context";

/** Whether the turn rail shows: a turn runs, or follow-ups wait. */
export function showsTurnRail(running: boolean, followUps: readonly QueuedFollowUp[]): boolean {
  return running || followUps.length > 0;
}

/**
 * One rail on the composer's top edge while a turn runs or follow-ups wait:
 * how long the agent has worked (or that the queue is paused), the plan's
 * progress with its steps above, the queue and the context window. Queue and
 * Context open inline; a newly queued follow-up opens the queue.
 */
export function TurnRail({
  view,
  sessionId,
  followUps,
  running,
  disabled,
  agent,
}: {
  view: SessionView;
  sessionId: string;
  followUps: readonly QueuedFollowUp[];
  running: boolean;
  disabled: boolean;
  /** The agent's name, for what it has not reported. */
  agent: string | null;
}) {
  const progress = useHost((s) => hasCapability(s.connection.compat, "agent.progress"));
  const plan = progress ? derivePlanUi(view) : null;
  const working = plan?.kind === "working" ? plan : null;
  const fraction = contextFraction(view.usage);
  const [open, setOpen] = useState<Panel | null>(followUps.length > 0 ? "queue" : null);
  const [steps, setSteps] = useState(false);
  const stepsButton = useRef<HTMLButtonElement>(null);
  const queued = useRef(followUps.length);
  useEffect(() => {
    if (followUps.length > queued.current) setOpen("queue");
    queued.current = followUps.length;
  }, [followUps.length]);
  const panel = open === "queue" && followUps.length === 0 ? null : open;
  const toggle = (p: Panel) => setOpen((o) => (o === p ? null : p));
  const tab = (on: boolean) =>
    cn(
      "focus-visible:ring-ring/50 flex h-6 shrink-0 items-center gap-1.5 rounded-md px-2 transition-colors outline-none focus-visible:ring-2",
      on ? "bg-secondary text-foreground" : "text-muted-foreground hover:text-foreground",
    );
  const current = working && working.current !== null ? working.progress.steps[working.current]?.title : null;

  return (
    <div className="bg-card animate-enter relative mx-4 -mb-4 rounded-t-xl border border-b-0 pb-4" data-testid="turn-rail">
      <div className="flex h-9 min-w-0 items-center gap-0.5 px-2 text-xs">
        <span className="text-muted-foreground flex shrink-0 items-center gap-1.5 pr-2 pl-1 font-mono" role="status">
          {running ? (
            <>
              <WorkingMark />
              <Elapsed key={view.live.userMessage?.message_id} turnId={view.live.userMessage?.message_id} />
            </>
          ) : (
            "Paused · Send one to carry on"
          )}
        </span>
        {working && (
          <button
            ref={stepsButton}
            type="button"
            className={cn(tab(steps), "min-w-0")}
            aria-label={`Plan, ${working.progress.completed} of ${working.progress.total} done${current ? ` · ${current}` : ""}`}
            aria-haspopup="dialog"
            aria-expanded={steps}
            onClick={() => setSteps((s) => !s)}
          >
            Plan
            <span className="text-foreground font-mono tabular-nums">{`${working.progress.completed}/${working.progress.total}`}</span>
            {current && <span className="min-w-0 truncate">{current}</span>}
            <ChevronUp aria-hidden className={cn("text-faint size-3 shrink-0 transition-transform", steps && "rotate-180")} />
          </button>
        )}
        {followUps.length > 0 && (
          <button
            type="button"
            className={tab(panel === "queue")}
            aria-label={`Queue, ${followUps.length} waiting`}
            aria-expanded={panel === "queue"}
            onClick={() => toggle("queue")}
          >
            Queue
            <span className="text-foreground font-mono tabular-nums">{followUps.length}</span>
          </button>
        )}
        {fraction !== null && (
          <button
            type="button"
            className={tab(panel === "context")}
            aria-label={`Context, ${Math.round(fraction * 100)}% in use`}
            aria-expanded={panel === "context"}
            onClick={() => toggle("context")}
          >
            Context
            <span className="text-foreground font-mono tabular-nums">{`${Math.round(fraction * 100)}%`}</span>
          </button>
        )}
      </div>
      {panel === "queue" && <FollowUpQueue sessionId={sessionId} items={followUps} running={running} disabled={disabled} />}
      {panel === "context" && (
        <div className="flex flex-col gap-2.5 px-3 pb-1 text-xs" data-testid="rail-context">
          <ContextDetails view={view} agent={agent} />
        </div>
      )}
      {steps && working && (
        <Popover
          onClose={(how) => {
            setSteps(false);
            if (how === "escape") stepsButton.current?.focus();
          }}
          className="right-0 left-0 max-h-[50vh] overflow-auto p-2"
          role="dialog"
          label="Tasks"
        >
          <StepList steps={working.progress.steps} />
        </Popover>
      )}
    </div>
  );
}

/** `Working · 1m 05s`, from when the turn's message was first seen. */
function Elapsed({ turnId }: { turnId: string | undefined }) {
  const [since] = useState(() => turnStart(turnId));
  const [now, setNow] = useState(since);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return <span className="tabular-nums">{`Working · ${turnDuration(Math.max(0, now - since))}`}</span>;
}
