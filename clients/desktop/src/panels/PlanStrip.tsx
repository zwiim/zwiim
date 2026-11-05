import { useEffect, useRef, useState } from "react";
import { ChevronDown, ChevronUp, Loader2 } from "lucide-react";
import {
  derivePlanUi,
  hasCapability,
  turnDuration,
  ZwiimClientError,
  type AgentType,
  type ProgressStep,
  type SessionView,
} from "@zwiim/sdk";

import { WorkingMark } from "@/components/WorkingMark";
import { Button } from "@/components/ui/button";
import { errorMessage, useController, useHost, useOnline } from "@/lib/host-context";
import { cn } from "@/lib/utils";

import { ContinuedInRow } from "./ContinuedCard";
import { focusMenu } from "./ControlCenter";
import { offeredAgents } from "./SessionPanel";
import { turnStart } from "./Transcript";

export const ROOT_REASON = "Continue needs a worktree. Start a new session from a worktree.";
export const ARCHIVED_REASON = "This worktree was archived. Its tasks can't be continued here.";

/**
 * The session's plan, docked on the composer's top edge: a slim strip while
 * the agent works through it, a resume bar with Continue when it stopped
 * part way, and where it went once another session took it over. Nothing on
 * a Host without `agent.progress`, nor once the plan is done.
 */
export function PlanStrip({
  view,
  root,
  archived,
  cardShown,
  onOpenSession,
  onNotice,
}: {
  view: SessionView;
  /** The session ran in the repository folder, which takes no new work. */
  root: boolean;
  /** Its worktree was archived. */
  archived: boolean;
  /** The can't-reach card shows: it is the only card, so there is no resume bar (spec §11a). */
  cardShown: boolean;
  /** A session this plan was continued in, in this workspace. */
  onOpenSession(id: string): void;
  /** What a refused Continue says, for the composer's notice slot; null clears it. */
  onNotice(message: string | null): void;
}) {
  const progress = useHost((s) => hasCapability(s.connection.compat, "agent.progress"));
  const canContinue = useHost((s) => hasCapability(s.connection.compat, "session.continue_plan"));
  // A plan the Host said there is nothing to continue for, until history says otherwise.
  const [hidden, setHidden] = useState<string | null>(null);
  if (!progress) return null;
  const ui = derivePlanUi(view);
  if (ui.kind === "continued") return <ContinuedInRow sessionId={ui.sessionId} onOpenSession={onOpenSession} />;
  if (ui.kind === "working")
    return <WorkingStrip steps={ui.progress.steps} completed={ui.progress.completed} total={ui.progress.total} current={ui.current} turnId={view.live.userMessage?.message_id} />;
  if (ui.kind !== "stopped" || !canContinue || cardShown || hidden === ui.plan.plan_id) return null;
  return (
    <ResumeBar
      key={ui.plan.plan_id}
      view={view}
      stoppedAt={ui.stoppedAt}
      reason={root ? ROOT_REASON : archived ? ARCHIVED_REASON : null}
      onOpenSession={onOpenSession}
      onNotice={onNotice}
      onGone={() => setHidden(ui.plan.plan_id)}
    />
  );
}

/**
 * The strip's frame: on the composer's surface, its foot tucked under the
 * composer's top edge (past the column's gap), so it reads as docked there.
 */
function Dock({ children, below }: { children: React.ReactNode; below?: React.ReactNode }) {
  return (
    <div className="bg-card relative mx-4 -mb-4 rounded-t-xl border border-b-0 pb-2" data-testid="plan-strip">
      <div className="text-ui flex h-9 items-center gap-2.5 px-3">{children}</div>
      {below}
    </div>
  );
}

function WorkingStrip({
  steps,
  completed,
  total,
  current,
  turnId,
}: {
  steps: ProgressStep[];
  completed: number;
  total: number;
  current: number | null;
  turnId: string | undefined;
}) {
  const [since] = useState(() => turnStart(turnId));
  const [now, setNow] = useState(since);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  const title = current === null ? null : steps[current]?.title;
  return (
    <Dock>
      <WorkingMark />
      <span className="min-w-0 flex-1 truncate">{title ?? "Working"}</span>
      <span className="text-muted-foreground font-mono text-xs tabular-nums">{`${completed} of ${total}`}</span>
      <Pips steps={steps} />
      <span className="text-muted-foreground font-mono text-xs tabular-nums">{turnDuration(Math.max(0, now - since))}</span>
      <StepsButton steps={steps} />
    </Dock>
  );
}

function ResumeBar({
  view,
  stoppedAt,
  reason,
  onOpenSession,
  onNotice,
  onGone,
}: {
  view: SessionView;
  stoppedAt: number;
  /** Why Continue cannot run here at all, said under the bar; null when it can. */
  reason: string | null;
  onOpenSession(id: string): void;
  onNotice(message: string | null): void;
  onGone(): void;
}) {
  const controller = useController();
  const online = useOnline();
  const plan = view.savedPlan!;
  const own = useHost(
    (s) => Object.values(s.sessionsByWorkspace).flat().find((x) => x.session_id === view.session_id)?.agent ?? plan.agent,
  );
  const agents = useHost((s) => s.agents);
  const ownName = agents?.find((a) => a.agent === own)?.name ?? own;
  const others = offeredAgents(agents ?? []).filter((a) => a.agent !== own && a.available);
  const [pending, setPending] = useState(false);
  const [menu, setMenu] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const busy = pending || !online || reason !== null;
  const step = plan.steps[stoppedAt];
  // What a refused Continue said goes with the bar: another plan, or none, has its own.
  useEffect(() => () => onNotice(null), [onNotice]);

  function closeMenu() {
    setMenu(false);
    trigger.current?.focus();
  }

  async function go(agent: AgentType | null) {
    if (pending) return;
    if (menu) closeMenu();
    setPending(true);
    onNotice(null);
    let started: string | null = null;
    try {
      const params = agent ? { session_id: view.session_id, agent } : { session_id: view.session_id };
      started = (await controller.client.request("session.continue_plan", params)).session_id;
    } catch (e) {
      setPending(false);
      const code = e instanceof ZwiimClientError ? (e.code as string) : null;
      if (code === "nothing_to_continue") {
        // Another device may have continued it: what the Host holds now
        // decides what shows, and until then the bar stays away.
        onGone();
        controller.openSession(view.session_id);
      } else if (code === "turn_in_progress") {
        // A turn runs: the history read brings the working strip, and the
        // bar back once the turn stops short again.
        controller.openSession(view.session_id);
      } else {
        onNotice(
          code === "root_workspace" ? ROOT_REASON : code === "workspace_removed" ? ARCHIVED_REASON : errorMessage(e),
        );
      }
    }
    // On success the button stays busy until the new session opens.
    // Outside the try: a throwing callback is not the Host refusing.
    if (started) onOpenSession(started);
  }

  return (
    <Dock
      below={reason && <p className="text-muted-foreground px-3 pb-2 text-xs">{reason}</p>}
    >
      <span
        // The stopped step, as a hollow accent ring: stopped is not an error.
        className="border-brand size-2.5 shrink-0 rounded-full border-[1.5px]"
        aria-hidden
        data-testid="plan-mark"
      />
      <span className="min-w-0 flex-1 truncate font-medium">
        {step ? `Stopped at task ${stoppedAt + 1} · ${step.title}` : `Stopped at task ${stoppedAt + 1}`}
      </span>
      <span className="text-muted-foreground font-mono text-xs tabular-nums">{`${plan.completed} of ${plan.total} done`}</span>
      <Pips steps={plan.steps} stoppedAt={stoppedAt} />
      <StepsButton steps={plan.steps} stoppedAt={stoppedAt} />
      <div className="relative flex shrink-0">
        <Button
          size="xs"
          variant="brand"
          className={cn(others.length > 0 && "rounded-r-none")}
          disabled={busy}
          title={reason ?? (online ? "A new session gets the tasks and full history" : "The Host is offline")}
          onClick={() => void go(null)}
        >
          {pending && <Loader2 className="animate-spin" />}
          {`Continue with ${ownName}`}
        </Button>
        {others.length > 0 && (
          <Button
            ref={trigger}
            size="xs"
            variant="brand"
            className="border-l-brand-foreground/20 rounded-l-none px-1"
            aria-label="Continue with another agent"
            aria-haspopup="menu"
            aria-expanded={menu}
            disabled={busy}
            onClick={() => setMenu((m) => !m)}
          >
            <ChevronDown />
          </Button>
        )}
        {menu && (
          <Popover
            onClose={(how) => (how === "escape" ? closeMenu() : setMenu(false))}
            className="right-0 w-48 origin-bottom-right p-1"
            role="menu"
            label="Continue with"
          >
            {others.map((a) => (
              <button
                key={a.agent}
                type="button"
                role="menuitem"
                className="hover:bg-muted focus-visible:bg-muted flex w-full items-center rounded-lg px-2 py-1.5 text-left text-ui transition-colors outline-none"
                onClick={() => void go(a.agent)}
              >
                {a.name}
              </button>
            ))}
          </Popover>
        )}
      </div>
    </Dock>
  );
}

/** One pip per step: done green, the current one muted (or a hollow accent ring where it stopped), the rest faint. */
function Pips({ steps, stoppedAt }: { steps: ProgressStep[]; stoppedAt?: number }) {
  return (
    <span className="flex shrink-0 items-center gap-1" aria-hidden>
      {steps.map((s, i) => (
        <span key={i} className={cn("size-1.5 rounded-full", pipClass(s, i === stoppedAt))} />
      ))}
    </span>
  );
}

function pipClass(step: ProgressStep, stopped: boolean): string {
  if (stopped) return "border-brand border";
  if (step.status === "completed") return "bg-done";
  if (step.status === "in_progress") return "bg-muted-foreground";
  return "bg-faint";
}

/** `⌃`: the full step list, in a popover above the strip. */
function StepsButton({ steps, stoppedAt }: { steps: ProgressStep[]; stoppedAt?: number }) {
  const [open, setOpen] = useState(false);
  const toggle = useRef<HTMLButtonElement>(null);
  return (
    <span className="flex shrink-0">
      <button
        ref={toggle}
        type="button"
        className="text-muted-foreground hover:text-foreground hover:bg-accent grid size-6 place-items-center rounded-md transition-colors"
        aria-label={open ? "Hide tasks" : "Show tasks"}
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
      >
        <ChevronUp className="size-3.5" />
      </button>
      {open && (
        <Popover
          onClose={(how) => {
            setOpen(false);
            if (how === "escape") toggle.current?.focus();
          }}
          className="right-0 left-0 p-2"
          role="dialog"
          label="Tasks"
        >
          <ol className="flex flex-col gap-0.5" data-testid="plan-steps">
            {steps.map((s, i) => (
              <li key={i} className="text-ui flex items-center gap-2.5 rounded-md px-2 py-1">
                <span className={cn("size-1.5 shrink-0 rounded-full", pipClass(s, i === stoppedAt))} aria-hidden />
                <span className="text-muted-foreground w-4 shrink-0 font-mono text-xs tabular-nums">{i + 1}</span>
                <span className={cn("min-w-0 flex-1 truncate", s.status === "completed" && "text-muted-foreground")}>
                  {s.title}
                </span>
              </li>
            ))}
          </ol>
        </Popover>
      )}
    </span>
  );
}

/**
 * A popover above the strip. Esc or a click outside it closes it; a click on
 * the button that opened it is left to that button. A menu takes focus on its
 * first item.
 */
function Popover({
  children,
  className,
  role,
  label,
  onClose,
}: {
  children: React.ReactNode;
  className?: string;
  role?: string;
  label: string;
  /** Esc hands focus back to the opener; a click elsewhere leaves it where it landed. */
  onClose(how: "escape" | "outside"): void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    focusMenu(ref.current);
    const outside = (e: PointerEvent) => {
      if (!ref.current?.parentElement?.contains(e.target as Node)) close.current("outside");
    };
    const escape = (e: KeyboardEvent) => {
      if (e.key === "Escape") close.current("escape");
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape);
    };
  }, []);
  return (
    <div
      ref={ref}
      role={role}
      tabIndex={role === "menu" ? -1 : undefined}
      aria-label={label}
      className={cn(
        "bg-popover text-foreground shadow-pop animate-pop-in absolute bottom-full z-20 mb-2 origin-bottom rounded-xl border outline-none",
        className,
      )}
    >
      {children}
    </div>
  );
}
