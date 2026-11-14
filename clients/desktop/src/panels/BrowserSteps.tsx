// The session's browser recording (browser design 10.7): what its agent did
// in its tabs, a small picture and a line a step, newest first, so a person
// can look back at what happened while they were away.

import { useEffect, useRef, useState } from "react";
import { History } from "lucide-react";
import type { BrowserStep } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { errorMessage, useController } from "@/lib/host-context";
import { POP, useDismiss } from "@/panels/BrowserPageUi";
import { cn } from "@/lib/utils";

/** Steps the Host sends at once; fewer means there are no older ones. */
const PAGE = 50;

export function BrowserSteps({ sessionId, disabled }: { sessionId: string; disabled: boolean }) {
  const controller = useController();
  const [open, setOpen] = useState(false);
  const [steps, setSteps] = useState<BrowserStep[] | null>(null);
  const [more, setMore] = useState(true);
  const [shown, setShown] = useState<BrowserStep | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, () => setOpen(false));

  // Read again on each open: the agent keeps working.
  useEffect(() => {
    if (!open) return;
    let live = true;
    setError(null);
    setShown(null);
    controller.browserSteps(sessionId).then(
      (s) => {
        if (!live) return;
        setSteps(s);
        setMore(s.length >= PAGE);
      },
      (e) => live && setError(errorMessage(e)),
    );
    return () => {
      live = false;
    };
  }, [controller, sessionId, open]);

  const older = () => {
    const last = steps?.at(-1);
    if (!last) return;
    controller.browserSteps(sessionId, last.seq).then(
      (s) => {
        setSteps((was) => [...(was ?? []), ...s]);
        setMore(s.length >= PAGE);
      },
      (e) => setError(errorMessage(e)),
    );
  };

  return (
    <div ref={ref} className="relative shrink-0">
      <Button
        size="icon-sm"
        variant="ghost"
        title="What the agent did"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
      >
        <History />
      </Button>
      {open && (
        <div
          className={cn(POP, "top-full right-0 mt-1 flex max-h-[70vh] w-72 flex-col gap-2 p-3")}
          role="dialog"
          aria-label="What the agent did"
          onKeyDown={(e) => {
            e.stopPropagation();
            if (e.key !== "Escape") return;
            if (shown) setShown(null);
            else setOpen(false);
          }}
        >
          <p className="text-ui font-medium">What the agent did</p>
          {shown ? (
            <div className="flex min-h-0 flex-col gap-2">
              <StepImage sessionId={sessionId} seq={shown.seq} className="w-full" />
              <StepLine step={shown} />
              <Button size="sm" variant="outline" onClick={() => setShown(null)}>
                Back to all steps
              </Button>
            </div>
          ) : steps && steps.length === 0 ? (
            <p className="text-muted-foreground text-xs">
              Nothing yet. When this session's agent uses its browser, each step shows here with a picture of the page.
            </p>
          ) : (
            <ul className="flex min-h-0 flex-col gap-1 overflow-y-auto" aria-label="Steps">
              {steps?.map((step) => (
                <li key={step.seq}>
                  <button
                    className="hover:bg-muted flex w-full items-start gap-2 rounded-md p-1 text-left"
                    onClick={() => setShown(step)}
                  >
                    <StepImage sessionId={sessionId} seq={step.seq} className="aspect-[4/3] w-20 shrink-0 object-cover object-top" />
                    <StepLine step={step} />
                  </button>
                </li>
              ))}
              {steps && more && (
                <li>
                  <Button size="xs" variant="ghost" className="w-full" onClick={older}>
                    Older steps
                  </Button>
                </li>
              )}
            </ul>
          )}
          {error && (
            <p className="text-destructive text-xs" role="alert">
              {error}
            </p>
          )}
        </div>
      )}
    </div>
  );
}

function StepLine({ step }: { step: BrowserStep }) {
  return (
    <span className="flex min-w-0 flex-1 flex-col">
      <span className="text-ui truncate">{step.action}</span>
      <span className="text-muted-foreground truncate text-xs">
        {new Date(step.at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })} · {step.url}
      </span>
    </span>
  );
}

function StepImage({ sessionId, seq, className }: { sessionId: string; seq: number; className?: string }) {
  const controller = useController();
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    controller.browserStepImage(sessionId, seq).then(
      (jpeg) => live && setSrc(`data:image/jpeg;base64,${jpeg}`),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [controller, sessionId, seq]);
  return src ? (
    <img src={src} alt="" className={cn("border-border rounded-sm border", className)} />
  ) : (
    <span className={cn("bg-muted aspect-video rounded-sm", className)} aria-hidden />
  );
}
