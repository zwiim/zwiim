import { ChevronDown, ListChecks, ShieldAlert, ShieldCheck } from "lucide-react";
import type { OptionsRequest, SessionMode, SessionOptions } from "@zwiim/sdk";
import { cn } from "@/lib/utils";

export const MODE_LABEL: Record<SessionMode, string> = {
  normal: "Ask",
  plan: "Plan",
  unattended: "Auto",
};

export const MODE_ICON: Record<SessionMode, typeof ShieldCheck> = {
  normal: ShieldCheck,
  plan: ListChecks,
  unattended: ShieldAlert,
};

export const MODE_HINT: Record<SessionMode, string> = {
  normal: "Asks before running commands or editing files",
  plan: "Plans first and asks you to approve the plan",
  unattended: "Runs commands and edits files without asking",
};

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
