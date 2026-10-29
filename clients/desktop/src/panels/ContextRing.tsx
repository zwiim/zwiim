import { useEffect, useRef, useState } from "react";
import { contextFraction, contextLeft, type SessionView } from "@zwiim/sdk";
import { alwaysShowContext } from "@/lib/prefs";
import { usePresence } from "@/lib/presence";
import { usePref } from "@/lib/use-pref";
import { cn } from "@/lib/utils";

/** A token count in full, grouped: `46,210`. */
export function tokens(n: number): string {
  return String(Math.round(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

interface Row {
  key: string;
  label: string;
  /** Null: the agent didn't report it. */
  value: string | null;
}

/** The window and the last turn's tokens, only as the agent reported them. */
export function contextRows(view: Pick<SessionView, "usage" | "turnUsage"> | undefined): {
  window: Row[];
  lastTurn: Row[] | null;
} {
  const usage = view?.usage ?? null;
  const n = (v: number | null | undefined) => (v == null ? null : tokens(v));
  const t = view?.turnUsage ?? null;
  return {
    window: [
      { key: "used", label: "In use", value: n(usage?.used) },
      { key: "size", label: "Window", value: n(usage?.size) },
      { key: "left", label: "Left", value: n(contextLeft(usage)) },
    ],
    lastTurn: t && [
      { key: "input", label: "Input", value: n(t.input_tokens) },
      { key: "output", label: "Output", value: n(t.output_tokens) },
      { key: "cached_read", label: "Cache read", value: n(t.cached_read_tokens) },
      { key: "cached_write", label: "Cache write", value: n(t.cached_write_tokens) },
      { key: "thought", label: "Reasoning", value: n(t.thought_tokens) },
      { key: "total", label: "Total", value: n(t.total_tokens) },
    ],
  };
}

const R = 6.5;
const C = 2 * Math.PI * R;

/** The share of the window in use from which the ring shows even where it isn't always shown. */
export const CONTEXT_NEARLY_FULL = 0.7;

/**
 * The share of the context window in use, as a ring beside send: always, or
 * (Settings) only once the window is nearly full. Hover or click shows the
 * figures the agent reported; nothing is estimated, and a figure it didn't
 * report says so.
 */
export function ContextRing({ view, agent }: { view: SessionView | undefined; agent: string | null }) {
  const [hover, setHover] = useState(false);
  const [pinned, setPinned] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const leave = useRef<ReturnType<typeof setTimeout> | null>(null);
  const open = hover || pinned;
  const presence = usePresence(open);
  const always = usePref(alwaysShowContext);

  const fraction = contextFraction(view?.usage);
  const percent = fraction === null ? "—" : `${Math.round(fraction * 100)}%`;
  const rows = contextRows(view);
  const reported = view?.usage != null && (view.usage.used != null || view.usage.size != null);

  useEffect(() => {
    if (!pinned) return;
    const close = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) setPinned(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [pinned]);

  useEffect(() => () => {
    if (leave.current) clearTimeout(leave.current);
  }, []);

  if (!always && (fraction === null || fraction < CONTEXT_NEARLY_FULL)) return null;
  return (
    <div
      ref={root}
      className="relative"
      onMouseEnter={() => {
        if (leave.current) clearTimeout(leave.current);
        setHover(true);
      }}
      onMouseLeave={() => {
        leave.current = setTimeout(() => setHover(false), 120);
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setPinned(false);
          setHover(false);
        }
      }}
    >
      <button
        type="button"
        aria-label={fraction === null ? "Context: not reported" : `Context: ${percent} in use`}
        aria-expanded={open}
        aria-haspopup="dialog"
        onClick={() => setPinned((p) => !p)}
        className="text-muted-foreground hover:text-foreground hover:bg-muted focus-visible:ring-ring/50 flex h-7 items-center gap-1.5 rounded-md px-2 font-mono text-xs tabular-nums transition-colors outline-none focus-visible:ring-2"
        data-testid="context-ring"
      >
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
          {fraction === null ? (
            <circle cx="8" cy="8" r={R} fill="none" stroke="currentColor" strokeOpacity={0.5} strokeWidth={1.5} strokeDasharray="2 2.5" />
          ) : (
            <>
              <circle cx="8" cy="8" r={R} fill="none" stroke="currentColor" strokeOpacity={0.25} strokeWidth={2} />
              <circle
                cx="8"
                cy="8"
                r={R}
                fill="none"
                className="stroke-brand ease-std transition-[stroke-dasharray] duration-300"
                strokeWidth={2}
                strokeLinecap="round"
                strokeDasharray={`${Math.max(0.02, fraction) * C} ${C}`}
                transform="rotate(-90 8 8)"
              />
            </>
          )}
        </svg>
        {percent}
      </button>
      {presence.mounted && (
        <div
          role="dialog"
          aria-label="Context"
          className="bg-card text-card-foreground presence-pop shadow-pop absolute right-0 bottom-full z-20 mb-2 flex w-72 origin-bottom-right flex-col gap-2.5 rounded-xl border p-3 text-xs"
          data-testid="context-popover"
          data-state={presence.state}
          aria-hidden={!open || undefined}
          inert={!open}
        >
          <p className="text-muted-foreground text-2xs font-semibold tracking-wide uppercase">Context</p>
          {fraction !== null && (
            <div className="bg-muted h-1.5 overflow-hidden rounded-full">
              <div className="bg-brand ease-std h-full rounded-full transition-[width] duration-300" style={{ width: `${Math.max(2, fraction * 100)}%` }} />
            </div>
          )}
          {reported ? (
            <Figures rows={rows.window} />
          ) : (
            <p className="text-muted-foreground" data-testid="context-unreported">
              {agent ?? "The agent"} hasn't reported its context window yet.
            </p>
          )}
          {rows.lastTurn && (
            <>
              <p className="text-muted-foreground text-2xs mt-1 font-semibold tracking-wide uppercase">Last turn · tokens</p>
              <Figures rows={rows.lastTurn} />
            </>
          )}
          <p className="text-muted-foreground">As {agent ?? "the agent"} reports them. Nothing here is estimated.</p>
        </div>
      )}
    </div>
  );
}

function Figures({ rows }: { rows: Row[] }) {
  return (
    <dl className="grid grid-cols-[1fr_auto] gap-x-4 gap-y-1">
      {rows.map((r) => (
        <div key={r.key} className="contents" data-testid={`context-${r.key}`}>
          <dt className="text-muted-foreground">{r.label}</dt>
          <dd className={cn("text-right font-mono tabular-nums", r.value === null && "text-muted-foreground")}>
            {r.value ?? "not reported"}
          </dd>
        </div>
      ))}
    </dl>
  );
}
