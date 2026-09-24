import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, ListChecks, ShieldAlert, ShieldCheck } from "lucide-react";
import type { ChoiceOption, OptionsRequest, SessionMode, SessionOptions } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const MODE_LABEL: Record<SessionMode, string> = {
  normal: "Ask",
  plan: "Plan",
  unattended: "Unattended",
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
 * the composer. Options the agent does not offer are not shown; unattended
 * appears only where the agent supports it and is never chosen without an
 * explicit confirmation.
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
        <OptionSelect label="Model" option={options.model} disabled={disabled} onPick={(model) => onChange({ model })} />
      )}
      {options.effort && (
        <OptionSelect label="Effort" option={options.effort} disabled={disabled} onPick={(effort) => onChange({ effort })} />
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

/**
 * Ask / Plan / Unattended as one choice. Picking unattended asks for a
 * confirmation inside the menu before anything changes.
 */
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
  const [confirming, setConfirming] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const items = useRef<(HTMLButtonElement | null)[]>([]);

  useEffect(() => {
    if (!open) {
      setConfirming(false);
      return;
    }
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
    if (open && !confirming) items.current[Math.max(0, available.indexOf(current))]?.focus();
  }, [open, confirming, available, current]);

  function pick(next: SessionMode) {
    if (next === current) return setOpen(false);
    if (next === "unattended") return setConfirming(true);
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
        onClick={() => setOpen((o) => !o)}
        className={cn(
          "focus-visible:ring-ring/50 flex h-7 items-center gap-1 rounded-md px-2 text-xs outline-none focus-visible:ring-2 disabled:opacity-50",
          unattended
            ? "bg-amber-500/15 font-medium text-amber-600 hover:bg-amber-500/25 dark:text-amber-400"
            : "text-muted-foreground hover:text-foreground hover:bg-muted disabled:hover:bg-transparent",
        )}
      >
        <Icon className="size-3.5" />
        {MODE_LABEL[current]}
        <ChevronDown className="size-3" />
      </button>
      {open && (
        <div
          className="bg-card text-card-foreground absolute right-0 bottom-full z-20 mb-2 w-72 rounded-xl border p-1 shadow-xl"
          role="menu"
          aria-label="Mode"
          onKeyDown={(e) => {
            if (confirming || (e.key !== "ArrowDown" && e.key !== "ArrowUp")) return;
            e.preventDefault();
            const at = items.current.findIndex((el) => el === document.activeElement);
            const n = available.length;
            items.current[(at + (e.key === "ArrowDown" ? 1 : n - 1)) % n]?.focus();
          }}
        >
          {confirming ? (
            <div className="flex flex-col gap-2 p-2 text-xs" data-testid="confirm-unattended">
              <p className="flex items-center gap-1.5 font-medium text-amber-600 dark:text-amber-400">
                <ShieldAlert className="size-4" /> Turn off approvals?
              </p>
              <p className="text-muted-foreground">
                The agent will run commands and edit files in this workspace without asking you first.
              </p>
              <div className="flex justify-end gap-1.5">
                <Button size="xs" variant="ghost" onClick={() => setOpen(false)} autoFocus>
                  Keep asking
                </Button>
                <Button
                  size="xs"
                  className="bg-amber-600 text-white hover:bg-amber-700"
                  onClick={() => {
                    setOpen(false);
                    onPick("unattended");
                  }}
                >
                  Run unattended
                </Button>
              </div>
            </div>
          ) : (
            available.map((m, i) => (
              <button
                key={m}
                ref={(el) => {
                  items.current[i] = el;
                }}
                type="button"
                role="menuitemradio"
                aria-checked={current === m}
                className="hover:bg-muted focus-visible:bg-muted flex w-full items-start gap-2 rounded-lg px-2.5 py-2 text-left outline-none"
                onClick={() => pick(m)}
              >
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className={cn("text-sm", m === "unattended" && "text-amber-600 dark:text-amber-400")}>
                    {MODE_LABEL[m]}
                  </span>
                  <span className="text-muted-foreground text-xs">{MODE_HINT[m]}</span>
                </span>
                {current === m && <Check className="mt-0.5 size-4 shrink-0" />}
              </button>
            ))
          )}
        </div>
      )}
    </div>
  );
}

/** While unattended is on, the composer says so and offers the way back. */
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
      className="flex items-center gap-2 rounded-t-[inherit] border-b border-amber-500/30 bg-amber-500/10 px-3 py-1.5 text-xs text-amber-700 dark:text-amber-300"
      role="status"
      data-testid="unattended-banner"
    >
      <ShieldAlert className="size-3.5 shrink-0" />
      <span className="flex-1">Unattended — the agent runs commands and edits files without asking.</span>
      {mode.available.includes("normal") && (
        <button
          className="font-medium underline-offset-2 hover:underline disabled:opacity-60"
          disabled={disabled}
          onClick={() => onChange({ mode: "normal" })}
        >
          Ask first again
        </button>
      )}
    </div>
  );
}
