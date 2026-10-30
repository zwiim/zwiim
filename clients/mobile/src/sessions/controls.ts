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
  type ChoiceOption,
  type OptionValue,
  type OptionsRequest,
  type SessionMode,
  type SessionOptions,
  type SessionView,
} from "@zwiim/sdk";
import { MODE_LABELS, choiceLabel, modeChoosable } from "./start";

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

/** The composer's one options chip: the mode (needs-you color when unattended) and its text. */
export interface OptionChips {
  mode: { mode: SessionMode; label: string } | null;
  /** `Ask · Opus 5 · High`; effort only while the model offers one. */
  text: string | null;
}

/** A model or effort as shown: the agent's label, the raw value it no longer lists, or "Default". */
function shownValue(option: ChoiceOption): string {
  return option.current === "" ? "Default" : choiceLabel(option, option.current);
}

export function optionChips(options: SessionOptions | null): OptionChips {
  const mode = options?.mode ? { mode: options.mode.current, label: MODE_LABELS[options.mode.current].label } : null;
  const parts: string[] = [];
  if (mode) parts.push(mode.label);
  if (options?.model) parts.push(shownValue(options.model));
  if (options?.effort) parts.push(shownValue(options.effort));
  return { mode, text: parts.length > 0 ? parts.join(" · ") : null };
}

export type OptionKey = "mode" | "model" | "effort";

/** One summary row in Session options; `open` while it has a page to choose from. */
export interface OptionRow {
  key: OptionKey;
  label: "Mode" | "Model" | "Effort";
  value: string;
  open: boolean;
}

export function optionRows(options: SessionOptions | null, editable: boolean): OptionRow[] {
  const rows: OptionRow[] = [];
  if (!options) return rows;
  if (options.mode) {
    rows.push({ key: "mode", label: "Mode", value: MODE_LABELS[options.mode.current].label, open: editable && modeChoosable(options) });
  }
  if (options.model) {
    rows.push({ key: "model", label: "Model", value: shownValue(options.model), open: editable && options.model.values.length >= 2 });
  }
  if (options.effort) {
    rows.push({ key: "effort", label: "Effort", value: shownValue(options.effort), open: editable && options.effort.values.length >= 2 });
  }
  return rows;
}

/** Which page of Session options shows. */
export type OptionsPage = "root" | OptionKey | "context";

/** The page to show: an option's page falls back to the summary once it can no longer change. */
export function pageFor(page: OptionsPage, rows: readonly OptionRow[]): OptionsPage {
  if (page === "root" || page === "context") return page;
  return rows.some((r) => r.key === page && r.open) ? page : "root";
}

export interface OptionsState {
  page: OptionsPage;
  query: string;
}

/** The page to keep: one that fell back stays on the summary, its search cleared, when its option unlocks. */
export function settleOptions(state: OptionsState, rows: readonly OptionRow[]): OptionsState {
  return pageFor(state.page, rows) === state.page ? state : { page: "root", query: "" };
}

/** Unattended turns approvals off, so the phone asks first. */
export function needsConfirm(change: OptionChange): boolean {
  return "mode" in change && change.mode === "unattended";
}

/** Model lists this long get a search field. */
export const SEARCH_FROM = 8;

/** The values matching a search by name, id or description, in the agent's order. */
export function filterChoices(values: readonly OptionValue[], query: string): OptionValue[] {
  const q = query.trim().toLowerCase();
  if (!q) return [...values];
  return values.filter((v) => [v.label, v.value, v.description ?? ""].some((s) => s.toLowerCase().includes(q)));
}

/** The composer's line while a turn runs: the options wait for it to end. */
export function lockedNote(options: SessionOptions | null, running: boolean): string | null {
  return running && options ? "Options change when the turn ends" : null;
}

/** A token count in full, grouped: `46,210`. */
export function tokens(n: number): string {
  return String(Math.round(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** A token count in short: `950`, `200K`, `1.5M`. */
export function compactTokens(n: number): string {
  if (n < 1_000) return String(Math.round(n));
  if (n < 1_000_000) return `${Math.round(n / 1_000)}K`;
  return `${Number((n / 1_000_000).toFixed(1))}M`;
}

/** The context line under the summary rows: `23% of 200K context`, naming what the agent didn't report. */
export function contextLine(view: Pick<SessionView, "usage"> | undefined): string {
  const used = view?.usage?.used ?? null;
  const size = view?.usage?.size ?? null;
  const fraction = contextFraction(view?.usage);
  if (fraction !== null && size !== null) return `${Math.round(fraction * 100)}% of ${compactTokens(size)} context`;
  if (used !== null) return `${tokens(used)} in use`;
  if (size !== null) return `${compactTokens(size)} window`;
  return "Context not reported";
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
