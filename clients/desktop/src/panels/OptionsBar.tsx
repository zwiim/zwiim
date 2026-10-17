import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, ListChecks, ShieldAlert, ShieldCheck } from "lucide-react";
import type { ChoiceOption, OptionsRequest, SessionMode, SessionOptions } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { usePresence } from "@/lib/presence";
import { cn } from "@/lib/utils";

const MODE_LABEL: Record<SessionMode, string> = {
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

/**
 * Model, effort and mode for the agent's next turn, as compact controls for
 * the composer. Options the agent does not offer are not shown; Auto
 * (unattended) appears only where the agent supports it.
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
  const mode = options.mode;
  const hint = disabled && disabledReason ? disabledReason : undefined;
  return (
    <div className="flex min-w-0 items-center gap-0.5" data-testid="options-bar" title={hint}>
      {options.model && (
        <OptionSelect
          label="Model"
          option={options.model}
          disabled={disabled}
          onPick={(model) => onChange({ model })}
        />
      )}
      {options.effort && (
        <OptionSelect
          label="Effort"
          option={options.effort}
          disabled={disabled}
          onPick={(effort) => onChange({ effort })}
        />
      )}
      {mode && mode.available.length > 1 && (
        <ModeMenu
          current={mode.current}
          available={mode.available}
          agentLabel={mode.label}
          disabled={disabled}
          onPick={(next) => onChange({ mode: next })}
        />
      )}
      {hint && <span className="sr-only">{hint}</span>}
    </div>
  );
}

/** A native select dressed as a quiet composer control. */
export function OptionSelect({
  label,
  option,
  disabled,
  onPick,
}: {
  label: string;
  option: ChoiceOption;
  disabled: boolean;
  onPick(value: string): void;
}) {
  // A current value the agent no longer lists is still shown, as the agent holds it.
  const listed = option.values.some((v) => v.value === option.current);
  return (
    <ControlSelect
      label={label}
      value={option.current}
      disabled={disabled}
      onChange={onPick}
      options={[
        ...(listed ? [] : [{ value: option.current, label: option.current || "Default" }]),
        ...option.values.map((v) => ({ value: v.value, label: v.label, title: v.description ?? undefined })),
      ]}
    />
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

/** Ask / Plan / Auto as one choice. */
function ModeMenu({
  current,
  available,
  agentLabel,
  disabled,
  onPick,
}: {
  current: SessionMode;
  available: SessionMode[];
  agentLabel: string;
  disabled: boolean;
  onPick(mode: SessionMode): void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const items = useRef<(HTMLButtonElement | null)[]>([]);
  // The menu stays mounted for its exit.
  const menu = usePresence(open);

  useEffect(() => {
    if (!open) return;
    const close = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);

  useEffect(() => {
    if (disabled) setOpen(false);
  }, [disabled]);

  useEffect(() => {
    if (open) items.current[Math.max(0, available.indexOf(current))]?.focus();
  }, [open, available, current]);

  function pick(next: SessionMode) {
    if (next === current) return setOpen(false);
    setOpen(false);
    onPick(next);
  }

  const unattended = current === "unattended";
  const Icon = MODE_ICON[current];
  return (
    <div
      ref={root}
      className="relative"
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
          root.current?.querySelector<HTMLButtonElement>("[aria-haspopup]")?.focus();
        }
      }}
    >
      <button
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`Mode: ${MODE_LABEL[current]}`}
        title={agentLabel ? `${MODE_HINT[current]} (${agentLabel})` : MODE_HINT[current]}
        disabled={disabled}
        onClick={() => setOpen(!open)}
        className={cn(
          "ease-std flex h-7 items-center gap-1 rounded-md px-2 text-xs transition-colors duration-120 disabled:opacity-50",
          unattended
            ? "bg-brand-soft text-brand hover:bg-brand-halo font-medium"
            : "text-muted-foreground hover:text-foreground hover:bg-muted disabled:hover:bg-transparent",
        )}
      >
        <Icon className="size-3.5" />
        {MODE_LABEL[current]}
        <ChevronDown className="size-3" />
      </button>
      {menu.mounted && (
        <div
          className="bg-popover text-popover-foreground presence-pop shadow-pop absolute right-0 bottom-full z-20 mb-2 w-72 origin-bottom-right rounded-xl border p-1"
          role="menu"
          aria-label="Mode"
          data-state={menu.state}
          aria-hidden={!open || undefined}
          inert={!open}
          onKeyDown={(e) => {
            if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
            e.preventDefault();
            const at = items.current.findIndex((el) => el === document.activeElement);
            const n = available.length;
            items.current[(at + (e.key === "ArrowDown" ? 1 : n - 1)) % n]?.focus();
          }}
        >
          {available.map((m, i) => (
            <button
              key={m}
              ref={(el) => {
                items.current[i] = el;
              }}
              type="button"
              role="menuitemradio"
              aria-checked={current === m}
              className="hover:bg-muted focus-visible:bg-muted ease-std flex w-full items-start gap-2 rounded-lg px-3 py-2 text-left transition-colors duration-120"
              onClick={() => pick(m)}
            >
              <span className="flex min-w-0 flex-1 flex-col">
                <span className={cn("text-ui", m === "unattended" && "text-brand")}>{MODE_LABEL[m]}</span>
                <span className="text-muted-foreground text-xs">{MODE_HINT[m]}</span>
              </span>
              {current === m && <Check className="mt-0.5 size-4 shrink-0" />}
            </button>
          ))}
        </div>
      )}
    </div>
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
