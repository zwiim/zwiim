import { useEffect, useId, useImperativeHandle, useRef, useState, type KeyboardEvent, type Ref } from "react";
import { Check, ChevronDown, ChevronRight, Search } from "lucide-react";
import type { AgentSummary, ChoiceOption, OptionsRequest, SessionMode, SessionOptions } from "@zwiim/sdk";
import { AgentIcon, agentBrand } from "@/components/AgentIcon";
import { rememberModelEffort, rememberRecentModel, toggleFavoriteModel } from "@/lib/prefs";
import { usePresence } from "@/lib/presence";
import { cn } from "@/lib/utils";
import { searchTarget } from "./model-picker-state";
import { AgentStrip, buildSections, ModelList, type Entry } from "./ModelPickerList";
import { MODE_HINT, MODE_ICON, MODE_LABEL } from "./OptionsBar";

type Side = "effort" | "mode";

/** The label of the current value, or the raw value the agent holds when it no longer lists it. */
function currentLabel(option: ChoiceOption): string {
  return option.values.find((v) => v.value === option.current)?.label ?? (option.current || "Default");
}

/** Moves focus to the next or previous of `items`, wrapping. */
function rove(items: HTMLElement[], down: boolean) {
  const at = items.findIndex((el) => el === document.activeElement);
  const n = items.length;
  if (n) items[(at + (down ? 1 : n - 1) + (at < 0 && !down ? 1 : 0)) % n]?.focus();
}

const kbd = "text-muted-foreground rounded border px-1 font-mono text-[11px] leading-4";
const ring = "focus-visible:ring-ring/50 outline-none focus-visible:ring-2";

/**
 * The composer's agent, model, effort and mode, as one chip that opens a
 * panel: search, a strip of agents, their models, and Effort and Mode side
 * lists. Another agent's model starts that agent in a new session only.
 */
export function ModelPicker({
  agents,
  agent,
  options,
  started,
  disabled,
  disabledReason,
  onChange,
  onAgent,
  handle,
}: {
  agents: AgentSummary[];
  agent: string;
  options: SessionOptions;
  started: boolean;
  disabled: boolean;
  disabledReason?: string | null;
  onChange(request: OptionsRequest): void;
  onAgent?(agent: string, request: OptionsRequest): void;
  /** Lets the composer open and close the panel from its keys. */
  handle?: Ref<{ toggle(): void }>;
}) {
  const hint = disabled && disabledReason ? disabledReason : undefined;
  const { mode, model, effort } = options;
  const [open, setOpen] = useState(false);
  const [side, setSide] = useState<Side | null>(null);
  const [query, setQuery] = useState("");
  const [view, setView] = useState(agent);
  const [act, setAct] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string[]>([]);
  const [, setFavorites] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const chip = useRef<HTMLButtonElement>(null);
  const searchEl = useRef<HTMLInputElement>(null);
  const tabEls = useRef<Record<string, HTMLButtonElement | null>>({});
  const rowEls = useRef<Partial<Record<Side, HTMLElement | null>>>({});
  const sideEl = useRef<HTMLDivElement>(null);
  const focusSide = useRef(false);
  const focusTab = useRef(false);
  const panel = usePresence(open);
  const id = useId();
  const listId = `${id}-list`;

  // The agent in use shows the session's own options.
  const own = agents.find((a) => a.agent === agent);
  const browsable: AgentSummary[] = own
    ? agents.map((a) => (a.agent === agent ? { ...a, options } : a))
    : [
        { agent, name: agent, available: true, reason: null, support: "supported", adapter: null, options, effort_by_model: {}, commands: null },
        ...agents,
      ];
  const browsed = browsable.find((a) => a.agent === view) ?? browsable.find((a) => a.agent === agent)!;
  const agentName = own?.name ?? agent;
  const canSwitch = !started && !!onAgent;
  const sections = buildSections(browsable, browsed, query, expanded.includes(browsed.agent), agent, canSwitch, id);
  const entries = [...(sections.quick ?? []), ...sections.models];
  const nav = entries.filter((e) => !e.blocked);
  const isSelected = (e: Entry) => e.agent === agent && e.model != null && e.model === model?.current;
  // When the highlighted row disappears (a star moved it), stay on the same model, else the same place.
  const actAt = useRef(0);
  const gone = act && !nav.some((e) => e.id === act);
  const activeId =
    (act && !gone ? act : null) ??
    (gone ? nav.find((e) => e.id === act.replace("-q:", "-m:"))?.id : null) ??
    (gone ? nav[Math.min(actAt.current, nav.length - 1)]?.id : null) ??
    (query.trim() ? null : nav.find((e) => e.kind === "model" && isSelected(e))?.id) ??
    nav[0]?.id ??
    null;
  const active = nav.find((e) => e.id === activeId);
  actAt.current = Math.max(0, nav.findIndex((e) => e.id === activeId));

  const effortOff = !effort || effort.values.length === 0;
  const modeOff = !mode || mode.available.length === 0;

  function close(refocus = false) {
    setOpen(false);
    setSide(null);
    setQuery("");
    if (refocus) chip.current?.focus();
  }

  function openPanel() {
    if (disabled) return;
    setView(agent);
    setAct(null);
    setExpanded([]);
    setOpen(true);
  }

  useImperativeHandle(handle, () => ({ toggle: () => (open ? close(true) : openPanel()) }));

  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) close();
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);

  useEffect(() => {
    if (disabled && open) close(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [disabled]);

  useEffect(() => {
    if (open) searchEl.current?.focus();
  }, [open]);

  useEffect(() => {
    const tab = tabEls.current[view];
    if (focusTab.current) tab?.focus();
    focusTab.current = false;
    tab?.scrollIntoView?.({ block: "nearest" });
  }, [view, open]);

  function focusIntoSide() {
    const el = sideEl.current;
    (el?.querySelector<HTMLElement>('[aria-checked="true"]') ?? el?.querySelector<HTMLElement>('[role="menuitemradio"]'))?.focus();
  }

  useEffect(() => {
    if (side && focusSide.current) focusIntoSide();
  }, [side]);

  function openSide(key: Side, focus: boolean) {
    if (key === "effort" ? effortOff : modeOff) return;
    if (side === key) {
      if (focus) focusIntoSide();
      return;
    }
    focusSide.current = focus;
    setSide(key);
  }

  function closeSide() {
    const key = side;
    setSide(null);
    if (key) rowEls.current[key]?.focus();
  }

  function browse(next: string) {
    setView(next);
    setAct(null);
  }

  function step(by: 1 | -1) {
    const i = browsable.findIndex((a) => a.agent === browsed.agent);
    browse(browsable[(i + by + browsable.length) % browsable.length].agent);
  }

  function pick(e: Entry) {
    if (e.blocked) return;
    if (e.kind === "more") return setExpanded((x) => [...x, e.agent]);
    if (e.agent !== agent) {
      close(true);
      if (e.model) rememberRecentModel({ agent: e.agent, model: e.model }, e.effort);
      return onAgent?.(e.agent, e.model ? { model: e.model, ...(e.effort ? { effort: e.effort } : {}) } : {});
    }
    if (!e.model) return close(true);
    close(true);
    rememberRecentModel({ agent, model: e.model }, e.effort);
    const request: OptionsRequest = {};
    if (e.model !== model?.current) request.model = e.model;
    if (e.send && (request.model || e.send !== effort?.current)) request.effort = e.send;
    if (request.model || request.effort) onChange(request);
  }

  function star(e: Entry) {
    if (!e.model) return;
    toggleFavoriteModel({ agent: e.agent, model: e.model });
    setAct(e.id);
    setFavorites((n) => n + 1);
  }

  function pickSide(key: Side, value: string) {
    if (key === "effort") {
      if (model) rememberModelEffort({ agent, model: model.current }, value);
      if (value !== effort?.current) onChange({ effort: value });
    } else if (value !== mode?.current) {
      onChange({ mode: value as SessionMode });
    }
    closeSide();
  }

  function onPanelKey(e: KeyboardEvent<HTMLDivElement>) {
    const mod = e.metaKey || e.ctrlKey;
    if (e.key === "Tab") {
      e.preventDefault();
      if (side) return closeSide();
      const order = ([
        searchEl.current,
        tabEls.current[browsed.agent],
        !effortOff && rowEls.current.effort,
        !modeOff && rowEls.current.mode,
      ] as (HTMLElement | null | false | undefined)[]).filter((el): el is HTMLElement => !!el);
      const at = order.findIndex((el) => el === document.activeElement);
      order[(at + (e.shiftKey ? -1 : 1) + order.length) % order.length]?.focus();
      return;
    }
    if (side && sideEl.current?.contains(e.target as Node)) return;
    if (mod && (e.key === "[" || e.key === "]")) {
      e.preventDefault();
      return step(e.key === "]" ? 1 : -1);
    }
    if (mod && e.key.toLowerCase() === "d") {
      e.preventDefault();
      if (active) star(active);
      return;
    }
    if (e.ctrlKey && /^[1-9]$/.test(e.key)) {
      e.preventDefault();
      const row = sections.quick?.[Number(e.key) - 1];
      if (row) pick(row);
      return;
    }
    if (e.target !== searchEl.current) return;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const at = nav.findIndex((x) => x.id === activeId);
      const next = nav[(at + (e.key === "ArrowDown" ? 1 : -1) + nav.length) % nav.length];
      if (next) setAct(next.id);
      document.getElementById(next?.id ?? "")?.scrollIntoView?.({ block: "nearest" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (active) pick(active);
    }
  }

  function onTabKey(e: KeyboardEvent<HTMLButtonElement>) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    focusTab.current = true;
    step(e.key === "ArrowDown" ? 1 : -1);
  }

  const brand = agentBrand(browsed.agent, browsed.name);
  const unattended = mode?.current === "unattended";
  const ModeIcon = mode ? MODE_ICON[mode.current] : null;
  const chipLabel = [
    mode && `${MODE_LABEL[mode.current]} mode`,
    agentName,
    model && currentLabel(model),
    effort && `${currentLabel(effort)} effort`,
  ]
    .filter(Boolean)
    .join(", ");

  const optionRow = (key: Side, label: string, value: string, off: boolean) => (
    <button
      ref={(el) => {
        rowEls.current[key] = el;
      }}
      type="button"
      aria-label={`${label}: ${value}`}
      aria-haspopup="menu"
      aria-expanded={side === key}
      aria-disabled={off || undefined}
      tabIndex={off ? -1 : 0}
      onClick={() => openSide(key, true)}
      onPointerEnter={() => openSide(key, false)}
      onKeyDown={(e) => {
        if (e.key !== "ArrowRight") return;
        e.preventDefault();
        openSide(key, true);
      }}
      className={cn(
        ring,
        "flex h-[38px] w-full items-center gap-2.5 border-t px-3 text-left text-ui focus-visible:ring-inset",
        off ? "text-faint cursor-default" : "hover:bg-muted",
        side === key && "bg-muted",
      )}
    >
      {label}
      <span className={cn("ml-auto flex items-center gap-1.5 font-mono text-xs", off ? "text-faint" : "text-muted-foreground")}>
        {value}
        {!off && <ChevronRight aria-hidden className="text-faint size-[13px]" />}
      </span>
    </button>
  );

  const sideItem = "hover:bg-muted focus-visible:bg-muted flex w-full items-center gap-2 rounded-lg px-2.5 py-[7px] text-left outline-none";

  return (
    <div
      ref={root}
      className="relative min-w-0"
      title={hint}
      onKeyDown={(e) => {
        if (e.key !== "Escape" || !open) return;
        e.preventDefault();
        e.stopPropagation();
        if (side) closeSide();
        else close(true);
      }}
    >
      <button
        ref={chip}
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-disabled={disabled || undefined}
        aria-label={chipLabel}
        title={hint ?? "Agent, model, effort and mode (⌘.)"}
        onClick={() => (open ? close() : openPanel())}
        className={cn(
          ring,
          "flex h-7 max-w-full min-w-0 items-center gap-[7px] rounded-lg pr-2 pl-1 text-xs transition-colors duration-120",
          disabled ? "cursor-default opacity-55" : "hover:bg-secondary",
          open && "bg-secondary",
        )}
      >
        {mode && ModeIcon && (
          <span
            className={cn(
              "flex h-[22px] shrink-0 items-center gap-[5px] rounded-md px-[7px] text-[11px] font-medium",
              unattended ? "bg-brand-soft text-brand" : "bg-background text-muted-foreground",
            )}
          >
            <ModeIcon aria-hidden className="size-[11px]" />
            {MODE_LABEL[mode.current]}
          </span>
        )}
        <AgentIcon agent={agent} name={agentName} size={18} />
        {model && <span className="truncate font-mono">{currentLabel(model)}</span>}
        {effort && <span className="text-muted-foreground shrink-0">{currentLabel(effort)}</span>}
        <ChevronDown aria-hidden className="text-faint size-[13px] shrink-0" />
      </button>
      {hint && <span className="sr-only">{hint}</span>}
      {panel.mounted && (
        <div
          className="absolute bottom-full left-0 z-20 mb-2 flex items-end gap-1.5"
          aria-hidden={!open || undefined}
          inert={!open}
        >
          <div
            role="dialog"
            aria-label="Agent options"
            data-state={panel.state}
            onKeyDown={onPanelKey}
            className="bg-popover text-popover-foreground presence-pop shadow-pop flex w-[440px] origin-bottom-left flex-col overflow-hidden rounded-xl border"
          >
            <div className="text-faint flex h-[42px] items-center gap-2 border-b px-3">
              <Search aria-hidden className="size-[15px] shrink-0" />
              <input
                ref={searchEl}
                role="combobox"
                aria-label="Search models"
                aria-expanded
                aria-controls={listId}
                aria-activedescendant={activeId ?? undefined}
                autoComplete="off"
                placeholder={`Search ${brand.short} models`}
                value={query}
                onChange={(e) => {
                  setQuery(e.target.value);
                  browse(searchTarget(browsable, browsed.agent, e.target.value));
                }}
                className="text-foreground placeholder:text-faint text-ui min-w-0 flex-1 bg-transparent outline-none"
              />
            </div>
            <div
              className="flex h-[300px] min-h-0"
              onPointerEnter={() => {
                if (side && !sideEl.current?.contains(document.activeElement)) setSide(null);
              }}
            >
              <AgentStrip
                agents={browsable}
                view={browsed.agent}
                inUse={agent}
                query={query}
                listId={listId}
                tabEls={tabEls}
                onSelect={(a) => {
                  browse(a);
                  searchEl.current?.focus();
                }}
                onKeyDown={onTabKey}
              />
              <ModelList
                sections={sections}
                listId={listId}
                view={browsed.agent}
                activeId={activeId}
                isSelected={isSelected}
                onHover={(e) => setAct(e.id)}
                onPick={pick}
                onStar={star}
              />
            </div>
            {optionRow("effort", "Effort", effortOff ? "not offered" : currentLabel(effort!), effortOff)}
            {optionRow("mode", "Mode", mode ? MODE_LABEL[mode.current] : "not offered", modeOff)}
            <div className="text-muted-foreground flex items-center gap-3 border-t bg-black/10 px-3 py-2 text-xs">
              {[
                ["↑↓", "move"],
                ["→", "open"],
                ["↵", "pick"],
                ["⌘[ ]", "agent"],
                ["⌘D", "favorite"],
              ].map(([k, label]) => (
                <span key={k} className="flex items-center gap-[5px]">
                  <kbd className={kbd}>{k}</kbd>
                  {label}
                </span>
              ))}
              <kbd className={cn(kbd, "ml-auto")}>esc</kbd>
            </div>
          </div>
          {open && side && (
            <div
              ref={sideEl}
              role="menu"
              aria-label={side === "effort" ? "Effort" : "Mode"}
              className="bg-popover text-popover-foreground shadow-pop w-[220px] rounded-xl border p-1.5"
              // Its foot sits by its row: the footer is 33px, a row 38px.
              style={{ marginBottom: side === "effort" ? 65 : 27 }}
              onKeyDown={(e) => {
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  e.stopPropagation();
                  rove([...(sideEl.current?.querySelectorAll<HTMLElement>('[role="menuitemradio"]') ?? [])], e.key === "ArrowDown");
                } else if (e.key === "Escape" || e.key === "ArrowLeft") {
                  e.preventDefault();
                  e.stopPropagation();
                  closeSide();
                }
              }}
            >
              <div className="text-muted-foreground px-2.5 pt-1.5 pb-1 text-[10px] font-semibold tracking-[.06em] uppercase">
                {side === "effort" ? `Effort · ${model ? currentLabel(model) : ""}` : `Mode · ${agentBrand(agent, agentName).short}`}
              </div>
              {side === "effort" &&
                effort?.values.map((v) => (
                  <button
                    key={v.value}
                    type="button"
                    role="menuitemradio"
                    aria-checked={effort.current === v.value}
                    aria-label={v.label}
                    title={v.description ?? undefined}
                    className={cn(sideItem, "text-ui")}
                    onClick={() => pickSide("effort", v.value)}
                  >
                    <span className="flex-1">{v.label}</span>
                    {effort.current === v.value && <Check aria-hidden className="text-brand size-4" />}
                  </button>
                ))}
              {side === "mode" &&
                mode?.available.map((m) => (
                  <button
                    key={m}
                    type="button"
                    role="menuitemradio"
                    aria-checked={mode.current === m}
                    aria-label={MODE_LABEL[m]}
                    title={m === mode.current && mode.label ? `${MODE_HINT[m]} (${mode.label})` : undefined}
                    className={sideItem}
                    onClick={() => pickSide("mode", m)}
                  >
                    <span className="flex min-w-0 flex-1 flex-col">
                      <span className={cn("text-ui", m === "unattended" && "text-brand")}>{MODE_LABEL[m]}</span>
                      <span className="text-muted-foreground text-xs">{MODE_HINT[m]}</span>
                    </span>
                    {mode.current === m && <Check aria-hidden className="text-brand size-4 shrink-0" />}
                  </button>
                ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
