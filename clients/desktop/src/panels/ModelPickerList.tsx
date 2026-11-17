import { memo, useEffect, useRef, useState, type KeyboardEvent, type MutableRefObject, type ReactNode } from "react";
import { Check, Star } from "lucide-react";
import { effortFor, type AgentSummary } from "@zwiim/sdk";
import { AgentDot, AgentIcon, agentBrand } from "@/components/AgentIcon";
import { favoriteModels, recentModels, modelEffort } from "@/lib/prefs";
import { cn } from "@/lib/utils";
import { matchCount, modelRows, quickRows } from "./model-picker-state";

/** One row of the picker's list: a model, the agent itself (models not learned yet), or "Show all". */
export interface Entry {
  id: string;
  kind: "quick" | "model" | "agent" | "more";
  agent: string;
  agentName: string;
  model: string | null;
  label: string;
  description: string | null;
  /** The effort the row shows, and its label. */
  effort: string | null;
  effortLabel: string | null;
  /** The effort a same-agent pick sends: one remembered for the model and still offered. */
  send: string | null;
  favorite: boolean;
  /** Why it cannot be picked. */
  blocked: string | null;
}

export interface Sections {
  /** Null while searching. */
  quick: Entry[] | null;
  browsed: AgentSummary;
  models: Entry[];
  note: string | null;
}

export function reasonText(reason: string | null): string {
  return reason ? reason.replace(/_/g, " ") : "unavailable";
}

function effortText(agent: AgentSummary | undefined, model: string, value: string | null): string | null {
  if (!value) return null;
  return (agent && effortFor(agent, model)?.values.find((v) => v.value === value)?.label) ?? value;
}

export function buildSections(
  agents: AgentSummary[],
  browsed: AgentSummary,
  query: string,
  expanded: boolean,
  inUse: string,
  canSwitch: boolean,
  idPrefix: string,
): Sections {
  const short = (a: AgentSummary) => agentBrand(a.agent, a.name).short;
  const blocked = (a: AgentSummary | undefined): string | null =>
    !a || a.agent === inUse
      ? null
      : !a.available
        ? `${a.name} is ${reasonText(a.reason)}`
        : canSwitch
          ? null
          : `Start a new session to use ${short(a)}`;
  const favorites = favoriteModels();
  const isFavorite = (agent: string, model: string) => favorites.some((f) => f.agent === agent && f.model === model);

  const quick = query.trim()
    ? null
    : quickRows(agents, favorites, recentModels()).map((r, i): Entry => {
        const owner = agents.find((a) => a.agent === r.ref.agent);
        return {
          id: `${idPrefix}-q:${r.ref.agent}/${r.ref.model}`,
          kind: "quick",
          agent: r.ref.agent,
          agentName: owner?.name ?? r.ref.agent,
          model: r.ref.model,
          label: r.label,
          description: null,
          effort: r.effort,
          effortLabel: effortText(owner, r.ref.model, r.effort),
          send: r.effort,
          favorite: r.favorite,
          blocked: blocked(owner),
        };
      });

  const base = {
    agent: browsed.agent,
    agentName: browsed.name,
    description: null,
    effort: null,
    effortLabel: null,
    send: null,
    favorite: false,
    blocked: null,
  };
  if (!browsed.available) return { quick, browsed, models: [], note: `${browsed.name} — ${reasonText(browsed.reason)}` };
  if (!browsed.options) {
    const row: Entry = { ...base, id: `${idPrefix}-a`, kind: "agent", model: null, label: browsed.name, description: "Learning its models…", blocked: blocked(browsed) };
    return { quick, browsed, models: [row], note: null };
  }
  const { rows, more } = modelRows(browsed, query, expanded);
  const models = rows.map(
    (r): Entry => ({
      id: `${idPrefix}-m:${browsed.agent}/${r.value}`,
      kind: "model",
      agent: browsed.agent,
      agentName: browsed.name,
      model: r.value,
      label: r.label,
      description: r.description,
      effort: r.effort,
      effortLabel: effortText(browsed, r.value, r.effort),
      send: r.effort && modelEffort({ agent: browsed.agent, model: r.value }) === r.effort ? r.effort : null,
      favorite: isFavorite(browsed.agent, r.value),
      blocked: blocked(browsed),
    }),
  );
  if (more > 0) models.push({ ...base, id: `${idPrefix}-more`, kind: "more", model: null, label: `Show all ${rows.length + more} models` });
  const note = query.trim() && !rows.length ? `No ${short(browsed)} models match` : null;
  return { quick, browsed, models, note };
}

const FADE = {
  up: "linear-gradient(transparent, #000 22px)",
  down: "linear-gradient(#000 calc(100% - 22px), transparent)",
  both: "linear-gradient(transparent, #000 22px, #000 calc(100% - 22px), transparent)",
};

/** The vertical strip of agent tiles: scrolls on its own, fading whichever edge has more. */
export function AgentStrip({
  agents,
  view,
  inUse,
  query,
  listId,
  tabEls,
  onSelect,
  onKeyDown,
}: {
  agents: AgentSummary[];
  view: string;
  inUse: string;
  query: string;
  listId: string;
  tabEls: React.RefObject<Record<string, HTMLButtonElement | null>>;
  onSelect(agent: string): void;
  onKeyDown(e: KeyboardEvent<HTMLButtonElement>): void;
}) {
  const strip = useRef<HTMLDivElement>(null);
  const [fade, setFade] = useState<keyof typeof FADE | null>(null);
  const measure = () => {
    const el = strip.current;
    if (!el) return;
    const up = el.scrollTop > 1;
    const down = el.scrollTop < el.scrollHeight - el.clientHeight - 1;
    setFade(up && down ? "both" : up ? "up" : down ? "down" : null);
  };
  useEffect(measure);
  return (
    <div
      ref={strip}
      role="tablist"
      aria-label="Agents"
      aria-orientation="vertical"
      onScroll={measure}
      className="flex shrink-0 flex-col gap-0.5 overflow-y-auto overscroll-contain border-r px-[5px] py-1.5 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      style={fade ? { maskImage: FADE[fade] } : undefined}
    >
      {agents.map((a) => {
        const on = a.agent === view;
        const n = query.trim() ? matchCount(a, query) : 0;
        return (
          <button
            key={a.agent}
            ref={(el) => {
              tabEls.current[a.agent] = el;
            }}
            type="button"
            role="tab"
            aria-selected={on}
            aria-controls={listId}
            aria-label={`${a.name}${a.available ? "" : `, ${reasonText(a.reason)}`}${a.agent === inUse ? ", in use" : ""}`}
            title={a.available ? a.name : `${a.name} · ${reasonText(a.reason)}`}
            tabIndex={on ? 0 : -1}
            onClick={() => onSelect(a.agent)}
            onKeyDown={onKeyDown}
            className={cn(
              "focus-visible:ring-ring/50 relative flex size-[38px] shrink-0 items-center justify-center rounded-lg transition-colors duration-120 outline-none focus-visible:ring-2",
              on ? "bg-brand-soft" : "hover:bg-muted",
              !a.available && "opacity-50 grayscale",
            )}
          >
            <span className={cn("rounded-[7px]", on && "ring-brand ring-offset-popover ring-[1.5px] ring-offset-2")}>
              <AgentIcon agent={a.agent} name={a.name} />
            </span>
            {a.agent === inUse && <span aria-hidden className="bg-done absolute top-[3px] right-[3px] size-1.5 rounded-full" />}
            {n > 0 && <span className="text-brand absolute right-1 bottom-0.5 font-mono text-[10px]">{n}</span>}
          </button>
        );
      })}
    </div>
  );
}

function Group({ id, label, aside, children }: { id: string; label: string; aside?: ReactNode; children: ReactNode }) {
  return (
    <div role="group" aria-labelledby={id}>
      <div
        id={id}
        role="presentation"
        className="text-muted-foreground flex items-center justify-between px-2 pt-2.5 pb-1 text-[10.5px] font-semibold tracking-[.06em] uppercase"
      >
        {label}
        {aside}
      </div>
      {children}
    </div>
  );
}

/** The list: Recent & favorites, then the browsed agent's models. Hover only moves the highlight. */
type Handlers = { onHover(e: Entry): void; onPick(e: Entry): void; onStar(e: Entry): void };

/** Re-renders only when its own row changes, so hovering never rebuilds the rows under the pointer. */
const Row = memo(
  function Row({ e, i, active, selected, view, h }: { e: Entry; i: number; active: boolean; selected: boolean; view: string; h: MutableRefObject<Handlers> }) {
    const quickRow = e.kind === "quick";
    const effort = e.effortLabel;
    const agentShort = agentBrand(e.agent, e.agentName).short;
    const label =
      e.kind === "more" || e.kind === "agent"
        ? e.label
        : `${e.label}${quickRow ? `, ${agentShort}` : ""}${effort ? `, ${effort} effort` : ""}`;
    const description =
      e.blocked ??
      (e.kind === "model" || quickRow
        ? `${e.favorite ? "Favorite. " : ""}Press Command D to ${e.favorite ? "remove from" : "add to"} favorites`
        : undefined);
    return (
      <div
        id={e.id}
        role="option"
        aria-selected={selected}
        aria-disabled={e.blocked ? true : undefined}
        aria-label={label}
        aria-description={description}
        onPointerMove={() => !active && h.current.onHover(e)}
        onClick={() => !e.blocked && h.current.onPick(e)}
        className={cn(
          "group/row relative flex min-h-8 items-center gap-2.5 rounded-lg px-2 py-[7px]",
          active && !e.blocked && "bg-muted",
          e.blocked ? "cursor-default opacity-50" : "cursor-pointer",
          e.kind === "more" && "text-muted-foreground text-xs",
          selected && "before:bg-brand before:absolute before:top-2 before:bottom-2 before:left-0 before:w-0.5 before:rounded-sm",
        )}
      >
        {e.kind === "more" ? (
          e.label
        ) : (
          <>
            {quickRow && <AgentDot agent={e.agent} name={e.agentName} />}
            <span className="text-foreground max-w-[62%] shrink-0 truncate font-mono text-[12.5px]">{e.label}</span>
            {quickRow && e.favorite && <Star aria-hidden className="text-muted-foreground -ml-1 size-[11px] fill-current" />}
            {quickRow && e.agent !== view && <span className="text-muted-foreground text-xs">{agentShort}</span>}
            {!quickRow && (e.blocked ?? e.description) && (
              <span className="text-muted-foreground min-w-0 truncate text-xs">{e.blocked ?? e.description}</span>
            )}
            <span className="ml-auto flex shrink-0 items-center gap-1.5">
              {effort && <span className="text-muted-foreground font-mono text-[11.5px]">{effort}</span>}
              {selected && <Check aria-hidden className="text-brand size-3.5" />}
              {quickRow && i < 9 && (
                <kbd className={cn("text-muted-foreground rounded border px-1 font-mono text-[11px] leading-4", !active && "invisible")}>
                  ⌃{i + 1}
                </kbd>
              )}
              {e.kind === "model" && (
                <span
                  aria-hidden
                  title={e.favorite ? "Remove from favorites (⌘D)" : "Add to favorites (⌘D)"}
                  onClick={(ev) => {
                    ev.stopPropagation();
                    h.current.onStar(e);
                  }}
                  className={cn(
                    "hover:bg-secondary hover:text-foreground grid size-6 place-items-center rounded-md",
                    e.favorite ? "text-foreground" : "text-faint opacity-0 group-hover/row:opacity-100",
                    active && "opacity-100",
                  )}
                >
                  <Star className={cn("size-[13px]", e.favorite && "fill-current")} />
                </span>
              )}
            </span>
          </>
        )}
      </div>
    );
  },
  (a, b) =>
    a.active === b.active &&
    a.selected === b.selected &&
    a.view === b.view &&
    a.i === b.i &&
    a.e.id === b.e.id &&
    a.e.label === b.e.label &&
    a.e.description === b.e.description &&
    a.e.effortLabel === b.e.effortLabel &&
    a.e.favorite === b.e.favorite &&
    a.e.blocked === b.e.blocked,
);

export function ModelList({
  sections,
  listId,
  view,
  activeId,
  isSelected,
  onHover,
  onPick,
  onStar,
}: {
  sections: Sections;
  listId: string;
  view: string;
  activeId: string | null;
  isSelected(e: Entry): boolean;
  onHover(e: Entry): void;
  onPick(e: Entry): void;
  onStar(e: Entry): void;
}) {
  const { quick, browsed, models, note } = sections;
  const hasFavorite = quick?.some((q) => q.favorite) ?? false;

  const h = useRef<Handlers>({ onHover, onPick, onStar });
  h.current = { onHover, onPick, onStar };
  const row = (e: Entry, i: number) => (
    <Row key={e.id} e={e} i={i} active={e.id === activeId} selected={isSelected(e)} view={view} h={h} />
  );
  return (
    <div id={listId} role="listbox" aria-label="Models" className="min-h-0 flex-1 overflow-y-auto px-1.5 pb-1.5">
      {quick && (
        <Group id={`${listId}-quick`} label="Recent & favorites">
          {quick.map(row)}
          {!hasFavorite && (
            <div className="text-muted-foreground flex items-center gap-1.5 px-2 pt-1 pb-1.5 text-xs">
              <Star aria-hidden className="size-[13px]" />
              Star a model to keep it here ·{" "}
              <kbd className="text-muted-foreground rounded border px-1 font-mono text-[11px] leading-4">⌘D</kbd>
            </div>
          )}
        </Group>
      )}
      <Group id={`${listId}-agent`} label={browsed.name}>
        {models.map(row)}
        {note && <p className="text-muted-foreground px-2 py-4 text-center text-xs">{note}</p>}
      </Group>
    </div>
  );
}
