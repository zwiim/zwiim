import { useState } from "react";
import { ShieldAlert } from "lucide-react";
import type { ChoiceOption, OptionsRequest, SessionMode, SessionOptions } from "@zwiim/sdk";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const MODE_LABEL: Record<SessionMode, string> = {
  normal: "Ask",
  plan: "Plan",
  unattended: "Unattended",
};

const MODE_HINT: Record<SessionMode, string> = {
  normal: "The agent asks before running commands or editing files",
  plan: "The agent plans first and asks you to approve the plan",
  unattended: "The agent runs commands and edits files without asking",
};

/**
 * Model, effort and mode, as the agent offers them. Options the agent does
 * not offer are not shown; unattended appears only where the agent supports
 * it and is never chosen without an explicit confirmation.
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
  const [confirming, setConfirming] = useState(false);
  const mode = options.mode;

  function pickMode(next: SessionMode) {
    if (next === mode?.current) return;
    if (next === "unattended") {
      setConfirming(true);
      return;
    }
    setConfirming(false);
    onChange({ mode: next });
  }

  return (
    <div className="flex flex-col gap-2" data-testid="options-bar">
      <div className="flex flex-wrap items-center gap-2 text-xs">
        {options.model && (
          <Choice label="Model" option={options.model} disabled={disabled} onPick={(model) => onChange({ model })} />
        )}
        {options.effort && (
          <Choice label="Effort" option={options.effort} disabled={disabled} onPick={(effort) => onChange({ effort })} />
        )}
        {mode && mode.available.length > 1 && (
          <div className="flex items-center rounded-md border p-0.5" role="radiogroup" aria-label="Mode">
            {mode.available.map((m) => (
              <button
                key={m}
                role="radio"
                aria-checked={mode.current === m}
                title={MODE_HINT[m]}
                disabled={disabled}
                onClick={() => pickMode(m)}
                className={cn(
                  "rounded px-2 py-0.5 disabled:opacity-50",
                  mode.current === m
                    ? m === "unattended"
                      ? "bg-amber-500 font-medium text-white"
                      : "bg-muted font-medium"
                    : "text-muted-foreground hover:bg-muted/60",
                )}
              >
                {MODE_LABEL[m]}
              </button>
            ))}
          </div>
        )}
        {mode?.label && mode.current !== "unattended" && (
          <span className="text-muted-foreground" title="The agent's own name for this mode">
            {mode.label}
          </span>
        )}
        {disabled && disabledReason && <span className="text-muted-foreground">{disabledReason}</span>}
      </div>
      {confirming && (
        <div
          className="flex flex-wrap items-center gap-2 rounded-md border border-amber-500/50 bg-amber-500/10 px-3 py-2 text-xs"
          data-testid="confirm-unattended"
        >
          <ShieldAlert className="size-4 shrink-0 text-amber-600" />
          <span className="flex-1">
            Unattended lets the agent run commands and edit files in this workspace without asking you first.
          </span>
          <Button
            size="xs"
            className="bg-amber-600 text-white hover:bg-amber-700"
            disabled={disabled}
            onClick={() => {
              setConfirming(false);
              onChange({ mode: "unattended" });
            }}
          >
            Run unattended
          </Button>
          <Button size="xs" variant="ghost" onClick={() => setConfirming(false)}>
            Keep asking
          </Button>
        </div>
      )}
      {mode?.current === "unattended" && (
        <div
          className="flex items-center gap-2 rounded-md bg-amber-500 px-3 py-1.5 text-xs font-medium text-white"
          role="status"
          data-testid="unattended-banner"
        >
          <ShieldAlert className="size-4 shrink-0" />
          <span className="flex-1">Unattended — the agent runs commands and edits files without asking.</span>
          {mode.available.includes("normal") && (
            <button className="underline disabled:opacity-60" disabled={disabled} onClick={() => pickMode("normal")}>
              Ask first again
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function Choice({
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
    <label className="text-muted-foreground flex items-center gap-1">
      {label}
      <select
        className="bg-background text-foreground rounded-md border px-1.5 py-0.5 disabled:opacity-50"
        aria-label={label}
        value={option.current}
        disabled={disabled}
        onChange={(e) => onPick(e.target.value)}
      >
        {!listed && <option value={option.current}>{option.current || "Default"}</option>}
        {option.values.map((v) => (
          <option key={v.value} value={v.value} title={v.description ?? undefined}>
            {v.label}
          </option>
        ))}
      </select>
    </label>
  );
}
