// A session's controls on the phone (DESIGN.md, Screen 5): the model, effort
// and mode the Host reports for it, changed one at a time between turns the
// way Desktop changes them, and the context figures the agent reported. Nothing
// here knows which agent runs: every choice, label and number is the Host's,
// and a figure the agent didn't report says so instead of being estimated.

import {
  contextFraction,
  contextLeft,
  isClientError,
  type AgentSummary,
  type OptionsRequest,
  type SessionMode,
  type SessionOptions,
  type SessionView,
} from "@zwiim/sdk";
import { MODE_LABELS, choiceLabel } from "./start";

/** The agent's own name for a session's agent (`claude_code` → "Claude Code"). */
export function agentName(agents: readonly AgentSummary[] | null, agent: string | null | undefined): string | null {
  if (!agent) return null;
  return agents?.find((a) => a.agent === agent)?.name ?? agent;
}

/** One option changed on its own, as Desktop sends it: a new model never carries the old model's effort. */
export type OptionChange = { model: string } | { effort: string } | { mode: SessionMode };

export function changeRequest(change: OptionChange): OptionsRequest {
  return { ...change };
}

/** Whether the options can change now, and why not. */
export type Editable = { ok: true } | { ok: false; reason: string };

export function optionsEditable({
  options,
  connected,
  running,
  changing,
  hostName,
}: {
  options: SessionOptions | null;
  connected: boolean;
  running: boolean;
  changing: boolean;
  hostName: string;
}): Editable {
  if (!connected) return { ok: false, reason: `Not connected to ${hostName}.` };
  if (!options) return { ok: false, reason: "The agent's options show once it reports them." };
  if (running) return { ok: false, reason: "Change these when the agent finishes its turn." };
  if (changing) return { ok: false, reason: "Changing…" };
  return { ok: true };
}

/** Why a change failed, and what to do. The options shown are the Host's either way. */
export function optionsError(e: unknown, hostName: string): string {
  if (isClientError(e, "turn_in_progress")) return "The agent is still working. Change this when it finishes.";
  if (isClientError(e, "option_unavailable")) return "The agent doesn't offer that now. Choose again.";
  if (isClientError(e, "session_not_live")) return "The agent stopped. Send a message to start it again.";
  if (isClientError(e, "unknown_outcome")) return "Not sure that changed. Refreshed from your Mac.";
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't change that. Try again.";
}

/** The composer's chips: the mode (needs-you color when unattended) and the model with its effort. */
export interface OptionChips {
  mode: { mode: SessionMode; label: string } | null;
  /** `Opus 5 · High`; effort only while the model offers one. */
  model: string | null;
}

export function optionChips(options: SessionOptions | null): OptionChips {
  const mode = options?.mode ? { mode: options.mode.current, label: MODE_LABELS[options.mode.current].label } : null;
  const parts: string[] = [];
  if (options?.model) parts.push(choiceLabel(options.model, options.model.current));
  if (options?.effort) parts.push(choiceLabel(options.effort, options.effort.current));
  return { mode, model: parts.length > 0 ? parts.join(" · ") : null };
}

/** The composer's line while a turn runs: the options wait for it to end. */
export function lockedNote(options: SessionOptions | null, running: boolean): string | null {
  return running && options ? "Options change when the turn ends" : null;
}

/** A token count in full, grouped: `46,210`. */
export function tokens(n: number): string {
  return String(Math.round(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** The context chip: the share in use, or a dash while the agent hasn't reported both figures. */
export function contextChip(view: Pick<SessionView, "usage"> | undefined): string {
  const fraction = contextFraction(view?.usage);
  return fraction === null ? "—" : `${Math.round(fraction * 100)}%`;
}

/**
 * The Context line in Session options: `37,738 of 1,000,000` and `962,262 left`,
 * naming whichever figure the agent didn't report; null while it reported neither.
 */
export function contextSummary(view: Pick<SessionView, "usage"> | undefined): { main: string; aside: string } | null {
  const used = view?.usage?.used ?? null;
  const size = view?.usage?.size ?? null;
  const left = contextLeft(view?.usage ?? null);
  if (used !== null && size !== null) {
    return { main: `${tokens(used)} of ${tokens(size)}`, aside: left === null ? "left not reported" : `${tokens(left)} left` };
  }
  if (used !== null) return { main: `${tokens(used)} in use`, aside: "window not reported" };
  if (size !== null) return { main: `${tokens(size)} window`, aside: "in use not reported" };
  return null;
}

/** One figure in the Context sheet; `value` null: the agent didn't report it. */
export interface ContextRow {
  key: string;
  label: string;
  value: string | null;
}

export interface ContextFigures {
  /** 0–1 when the agent reported both used and size. */
  fraction: number | null;
  window: ContextRow[];
  /** Null until the agent reports a finished turn's tokens. */
  lastTurn: ContextRow[] | null;
}

export function contextFigures(view: Pick<SessionView, "usage" | "turnUsage"> | undefined): ContextFigures {
  const usage = view?.usage ?? null;
  const n = (v: number | null | undefined) => (v == null ? null : tokens(v));
  const left = contextLeft(usage);
  const window: ContextRow[] = [
    { key: "used", label: "In use", value: n(usage?.used) },
    { key: "size", label: "Window", value: n(usage?.size) },
    { key: "left", label: "Left", value: n(left) },
  ];
  const t = view?.turnUsage ?? null;
  const lastTurn = t
    ? [
        { key: "input", label: "Input", value: n(t.input_tokens) },
        { key: "output", label: "Output", value: n(t.output_tokens) },
        { key: "cached_read", label: "Cache read", value: n(t.cached_read_tokens) },
        { key: "cached_write", label: "Cache write", value: n(t.cached_write_tokens) },
        { key: "thought", label: "Reasoning", value: n(t.thought_tokens) },
        { key: "total", label: "Total", value: n(t.total_tokens) },
      ]
    : null;
  return { fraction: contextFraction(usage), window, lastTurn };
}
