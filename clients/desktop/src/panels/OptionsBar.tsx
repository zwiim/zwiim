import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, ChevronRight, ListChecks, Search, ShieldAlert, ShieldCheck } from "lucide-react";
import type { ChoiceOption, OptionsRequest, OptionValue, SessionMode, SessionOptions } from "@zwiim/sdk";
import { usePresence } from "@/lib/presence";
import { cn } from "@/lib/utils";

export const MODE_LABEL: Record<SessionMode, string> = {
  normal: "Ask",
  plan: "Plan",
  unattended: "Auto",
};

const MODE_ICON: Record<SessionMode, typeof ShieldCheck> = {
  normal: ShieldCheck,
  plan: ListChecks,
  unattended: ShieldAlert,
};

const MODE_HINT: Record<SessionMode, string> = {
  normal: "Asks before running commands or editing files",
  plan: "Plans first and asks you to approve the plan",
  unattended: "Runs commands and edits files without asking",
};

type OptionKey = "mode" | "model" | "effort";

interface OptionRow {
  key: OptionKey;
  label: string;
  value: string;
  /** Whether there is anything else to choose. */
  enabled: boolean;
}

/** From this many models, the Model submenu offers a search. */
const SEARCH_FROM = 8;

/** The label of the current value, or the raw value the agent holds when it no longer lists it. */
function currentLabel(option: ChoiceOption): string {
  return option.values.find((v) => v.value === option.current)?.label ?? (option.current || "Default");
}

/** The model values whose label, value or description holds `query`, in order. */
function filterModels(values: OptionValue[], query: string): OptionValue[] {
  const q = query.trim().toLowerCase();
  if (!q) return values;
  return values.filter((v) => [v.label, v.value, v.description ?? ""].some((s) => s.toLowerCase().includes(q)));
}

/** Moves focus to the next or previous of `items`, wrapping. */
function rove(items: HTMLElement[], down: boolean) {
  const at = items.findIndex((el) => el === document.activeElement);
  const n = items.length;
  if (n) items[(at + (down ? 1 : n - 1) + (at < 0 && !down ? 1 : 0)) % n]?.focus();
}

/**
 * Mode, model and effort for the agent's next turn, as one composer chip that
 * opens a menu of the three with a submenu each. Options the agent does not
 * offer are not shown; Auto (unattended) appears only where the agent
 * supports it.
 */
export function OptionsBar({
  options,
  disabled,
  disabledReason,
  onChange,
}: {
  options: SessionOptions;
  disabled: boolean;
  /** Why the options cannot change right now (shown as a hint). */
  disabledReason?: string | null;
  onChange(request: OptionsRequest): void;
}) {
  const hint = disabled && disabledReason ? disabledReason : undefined;
  const { mode, model, effort } = options;
  const [open, setOpen] = useState(false);
  const [sub, setSub] = useState<OptionKey | null>(null);
  const [query, setQuery] = useState("");
  const root = useRef<HTMLDivElement>(null);
  const chip = useRef<HTMLButtonElement>(null);
  const rowEls = useRef<Partial<Record<OptionKey, HTMLButtonElement | null>>>({});
  const subEl = useRef<HTMLDivElement>(null);
  // Whether the submenu just opened should take focus (not when opened by hover).
  const focusSub = useRef(false);
  // The menu stays mounted for its exit.
  const menu = usePresence(open);

  const rows: OptionRow[] = [];
  if (mode) rows.push({ key: "mode", label: "Mode", value: MODE_LABEL[mode.current], enabled: mode.available.length > 1 });
  if (model) rows.push({ key: "model", label: "Model", value: currentLabel(model), enabled: model.values.length >= 2 });
  if (effort) rows.push({ key: "effort", label: "Effort", value: currentLabel(effort), enabled: effort.values.length >= 2 });
  const text = rows.map((r) => r.value).join(" · ");

  function close(refocus = false) {
    setOpen(false);
    setSub(null);
    setQuery("");
    if (refocus) chip.current?.focus();
  }

  useEffect(() => {
    if (!open) return;
    const outside = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) close();
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);

  useEffect(() => {
    if (disabled) close();
  }, [disabled]);

  useEffect(() => {
    if (open) rowEls.current[rows.find((r) => r.enabled)?.key ?? rows[0]?.key ?? "mode"]?.focus();
    // Only on opening: later renders keep the focus where the user moved it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  function focusIntoSub() {
    const el = subEl.current;
    (
      el?.querySelector<HTMLElement>("input") ??
      el?.querySelector<HTMLElement>('[aria-checked="true"]') ??
      el?.querySelector<HTMLElement>('[role="menuitemradio"]')
    )?.focus();
  }

  useEffect(() => {
    if (sub && focusSub.current) focusIntoSub();
  }, [sub]);

  function openSub(row: OptionRow, focus: boolean) {
    if (!row.enabled) return setSub(null);
    if (sub === row.key) {
      if (focus) focusIntoSub();
      return;
    }
    focusSub.current = focus;
    setQuery("");
    setSub(row.key);
  }

  function closeSub() {
    const key = sub;
    setSub(null);
    setQuery("");
    if (key) rowEls.current[key]?.focus();
  }

  function pick(key: OptionKey, value: string) {
    const current = key === "mode" ? mode?.current : key === "model" ? model?.current : effort?.current;
    close(true);
    if (value === current) return;
    onChange(key === "mode" ? { mode: value as SessionMode } : { [key]: value });
  }

  const unattended = mode?.current === "unattended";
  const Icon = mode ? MODE_ICON[mode.current] : null;
  const item =
    "hover:bg-muted focus-visible:bg-muted ease-std flex w-full items-start gap-2 rounded-lg px-3 py-2 text-left transition-colors duration-120 outline-none";
  return (
    <div className="flex min-w-0 items-center gap-0.5" data-testid="options-bar" title={hint}>
      {rows.length > 0 && (
        <div
          ref={root}
          className="relative min-w-0"
          onKeyDown={(e) => {
            if (e.key === "Escape" && open) {
              e.stopPropagation();
              close(true);
            }
          }}
        >
          <button
            ref={chip}
            type="button"
            aria-haspopup="menu"
            aria-expanded={open}
            aria-label={`Session options: ${text}`}
            title={hint}
            disabled={disabled}
            onClick={() => (open ? close() : setOpen(true))}
            className={cn(
              "ease-std flex h-7 max-w-full min-w-0 items-center gap-1 rounded-md px-2 text-xs transition-colors duration-120 disabled:opacity-50",
              unattended
                ? "bg-brand-soft text-brand hover:bg-brand-halo font-medium"
                : "text-muted-foreground hover:text-foreground hover:bg-muted disabled:hover:bg-transparent",
            )}
          >
            {Icon && <Icon className="size-3.5 shrink-0" />}
            {rows.map((r, i) => (
              <span key={r.key} className={cn("flex min-w-0 items-center gap-1", r.key !== "mode" && "font-mono")}>
                {i > 0 && <span aria-hidden>·</span>}
                <span className="truncate">{r.value}</span>
              </span>
            ))}
            <ChevronDown className="size-3 shrink-0" />
          </button>
          {menu.mounted && (
            <div
              className="bg-popover text-popover-foreground presence-pop shadow-pop absolute right-0 bottom-full z-20 mb-2 w-[260px] origin-bottom-right rounded-xl border p-1"
              role="menu"
              aria-label="Session options"
              data-state={menu.state}
              aria-hidden={!open || undefined}
              inert={!open}
              onKeyDown={(e) => {
                if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
                e.preventDefault();
                rove(
                  rows.map((r) => rowEls.current[r.key]).filter((el): el is HTMLButtonElement => !!el),
                  e.key === "ArrowDown",
                );
              }}
            >
              {rows.map((r) => (
                <div key={r.key} className="relative" role="none">
                  <button
                    ref={(el) => {
                      rowEls.current[r.key] = el;
                    }}
                    type="button"
                    role="menuitem"
                    aria-haspopup="menu"
                    aria-expanded={sub === r.key}
                    aria-disabled={!r.enabled || undefined}
                    aria-label={`${r.label} ${r.value}`}
                    className={cn(item, "items-center", sub === r.key && "bg-muted", !r.enabled && "cursor-default")}
                    onClick={() => openSub(r, true)}
                    onPointerEnter={() => openSub(r, false)}
                    onKeyDown={(e) => {
                      if (e.key !== "ArrowRight") return;
                      e.preventDefault();
                      openSub(r, true);
                    }}
                  >
                    <span className="text-ui flex-1">{r.label}</span>
                    <span className="text-muted-foreground max-w-[60%] truncate font-mono text-xs">{r.value}</span>
                    <ChevronRight className={cn("size-3.5 shrink-0", r.enabled ? "text-muted-foreground" : "opacity-0")} />
                  </button>
                  {sub === r.key && (
                    <div
                      ref={subEl}
                      className="bg-popover text-popover-foreground shadow-pop absolute right-full bottom-0 z-30 mr-1 max-h-[min(420px,70vh)] w-72 overflow-y-auto rounded-xl border p-1"
                      role="menu"
                      aria-label={r.label}
                      onKeyDown={(e) => {
                        const inSearch = e.target instanceof HTMLInputElement;
                        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                          e.preventDefault();
                          e.stopPropagation();
                          rove(
                            [...(subEl.current?.querySelectorAll<HTMLElement>('input, [role="menuitemradio"]') ?? [])],
                            e.key === "ArrowDown",
                          );
                        } else if (e.key === "Escape" || (e.key === "ArrowLeft" && !inSearch)) {
                          e.preventDefault();
                          e.stopPropagation();
                          closeSub();
                        }
                      }}
                    >
                      {r.key === "mode" &&
                        mode?.available.map((m) => (
                          <button
                            key={m}
                            type="button"
                            role="menuitemradio"
                            aria-checked={mode.current === m}
                            aria-label={MODE_LABEL[m]}
                            title={m === mode.current && mode.label ? `${MODE_HINT[m]} (${mode.label})` : undefined}
                            className={item}
                            onClick={() => pick("mode", m)}
                          >
                            <span className="flex min-w-0 flex-1 flex-col">
                              <span className={cn("text-ui", m === "unattended" && "text-brand")}>{MODE_LABEL[m]}</span>
                              <span className="text-muted-foreground text-xs">{MODE_HINT[m]}</span>
                            </span>
                            {mode.current === m && <Check className="mt-0.5 size-4 shrink-0" />}
                          </button>
                        ))}
                      {r.key === "model" && model && (
                        <ModelChoices model={model} query={query} onQuery={setQuery} onPick={(v) => pick("model", v)} item={item} />
                      )}
                      {r.key === "effort" &&
                        effort?.values.map((v) => (
                          <button
                            key={v.value}
                            type="button"
                            role="menuitemradio"
                            aria-checked={effort.current === v.value}
                            aria-label={v.label}
                            title={v.description ?? undefined}
                            className={item}
                            onClick={() => pick("effort", v.value)}
                          >
                            <span className="text-ui flex-1">{v.label}</span>
                            {effort.current === v.value && <Check className="mt-0.5 size-4 shrink-0" />}
                          </button>
                        ))}
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
      {hint && <span className="sr-only">{hint}</span>}
    </div>
  );
}

/** The Model submenu's items, with a search once the agent offers many. */
function ModelChoices({
  model,
  query,
  onQuery,
  onPick,
  item,
}: {
  model: ChoiceOption;
  query: string;
  onQuery(query: string): void;
  onPick(value: string): void;
  item: string;
}) {
  const shown = filterModels(model.values, query);
  return (
    <>
      {model.values.length >= SEARCH_FROM && (
        <div className="bg-popover sticky -top-1 z-10 -mt-1 -mx-1 mb-1 border-b px-1 pt-1 pb-1">
          <div className="flex h-8 items-center gap-2 rounded-lg px-2">
            <Search className="text-muted-foreground size-3.5 shrink-0" />
            <input
              aria-label="Search models"
              placeholder="Search models"
              value={query}
              onChange={(e) => onQuery(e.target.value)}
              className="placeholder:text-muted-foreground min-w-0 flex-1 bg-transparent text-xs outline-none"
            />
          </div>
        </div>
      )}
      {shown.map((v) => (
        <button
          key={v.value}
          type="button"
          role="menuitemradio"
          aria-checked={model.current === v.value}
          aria-label={v.label}
          className={item}
          onClick={() => onPick(v.value)}
        >
          <span className="flex min-w-0 flex-1 flex-col">
            <span className="text-ui truncate">{v.label}</span>
            {v.description && <span className="text-muted-foreground text-xs">{v.description}</span>}
          </span>
          {model.current === v.value && <Check className="mt-0.5 size-4 shrink-0" />}
        </button>
      ))}
      {shown.length === 0 && (
        <p className="text-muted-foreground px-3 py-2 text-xs">No models match "{query.trim()}"</p>
      )}
    </>
  );
}

export function ControlSelect({
  label,
  value,
  disabled,
  onChange,
  options,
  className,
}: {
  label: string;
  value: string;
  disabled?: boolean;
  onChange(value: string): void;
  options: { value: string; label: string; title?: string; disabled?: boolean }[];
  className?: string;
}) {
  // The select itself is invisible over its label, so the control is as wide
  // as the current choice (a native select is as wide as its longest option).
  const shown = options.find((o) => o.value === value)?.label ?? value;
  return (
    <span
      className={cn(
        "text-muted-foreground has-[select:enabled]:hover:text-foreground has-[select:enabled]:hover:bg-muted has-[:focus-visible]:ring-ring/50 relative inline-flex h-7 min-w-0 items-center gap-1 rounded-md px-2 text-xs has-[select:disabled]:opacity-50 has-[:focus-visible]:ring-2",
        className,
      )}
    >
      <span className="truncate">{shown}</span>
      <ChevronDown className="size-3 shrink-0" />
      <select
        className="absolute inset-0 cursor-pointer appearance-none opacity-0 outline-none disabled:cursor-default"
        aria-label={label}
        title={label}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value} title={o.title} disabled={o.disabled}>
            {o.label}
          </option>
        ))}
      </select>
    </span>
  );
}

/** While Auto (unattended) is on, the composer says so and offers the way back. */
export function UnattendedBanner({
  options,
  disabled,
  onChange,
}: {
  options: SessionOptions;
  disabled: boolean;
  onChange(request: OptionsRequest): void;
}) {
  const mode = options.mode;
  if (mode?.current !== "unattended") return null;
  return (
    <div
      className="border-brand/30 bg-brand-soft text-brand flex items-center gap-2 rounded-t-[inherit] border-b px-3 py-1.5 text-xs"
      role="status"
      data-testid="unattended-banner"
    >
      <ShieldAlert className="size-3.5 shrink-0" />
      <span className="flex-1">Auto — the agent runs commands and edits files without asking.</span>
      {mode.available.includes("normal") && (
        <button
          type="button"
          className="rounded-sm font-medium underline-offset-2 hover:underline disabled:opacity-50"
          disabled={disabled}
          onClick={() => onChange({ mode: "normal" })}
        >
          Ask first again
        </button>
      )}
    </div>
  );
}
