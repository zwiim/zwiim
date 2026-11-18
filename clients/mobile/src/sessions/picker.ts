// The Agent & model sheet's decisions (DESIGN.md, Screen 5): which models
// show, how hot each effort level looks, how the switch travels and snaps,
// the three modes, and when the agent can still change.

import type { ModeOption, OptionValue, SessionMode } from "@zwiim/sdk";
import { filterChoices } from "./controls";

/** Models shown before "Show all". */
export const MODEL_PREVIEW = 5;

/** The models to list: a search covers every one; otherwise the first five unless expanded. */
export function visibleModels(
  values: readonly OptionValue[],
  { expanded, query }: { expanded: boolean; query: string },
): { shown: OptionValue[]; hidden: number } {
  if (query.trim()) return { shown: filterChoices(values, query), hidden: 0 };
  if (expanded || values.length <= MODEL_PREVIEW) return { shown: [...values], hidden: 0 };
  return { shown: values.slice(0, MODEL_PREVIEW), hidden: values.length - MODEL_PREVIEW };
}

export function showAllLabel(total: number, expanded: boolean): string | null {
  if (total <= MODEL_PREVIEW) return null;
  return expanded ? "Show less" : `Show all ${total}`;
}

/** How an effort level's highlight looks: its gradient, and how much glow, shine and sparks. */
export interface Heat {
  from: string;
  to: string;
  glow: number;
  shine: number;
  sparks: number;
  text: string;
}

/** Calm blue to fire orange, spread over however many levels the agent has. */
const HEAT_STOPS: readonly { from: string; to: string; text: string }[] = [
  { from: "#2c4a63", to: "#3a5f7d", text: "#8fb8d8" },
  { from: "#2f6fa3", to: "#4a90c9", text: "#6CB4F0" },
  { from: "#4b6cf0", to: "#8a5cf6", text: "#a48bff" },
  { from: "#8a5cf6", to: "#e0569b", text: "#f07ab8" },
  { from: "#f0565c", to: "#e8892b", text: "#ffc062" },
];

export function heatLook(index: number, count: number): Heat {
  const t = count < 2 ? 1 : Math.min(1, Math.max(0, index / (count - 1)));
  const stop = HEAT_STOPS[Math.round(t * (HEAT_STOPS.length - 1))]!;
  return {
    ...stop,
    glow: t,
    shine: t < 0.1 ? 0 : 0.2 + 0.55 * t,
    sparks: t > 0.6 ? Math.min(1, (t - 0.6) * 2.2) : 0,
  };
}

/** Where the Host's value sits in the switch; null when it is not one the switch lists. */
export function levelAt(values: readonly OptionValue[], current: string | null): number | null {
  const i = values.findIndex((v) => v.value === current);
  return i < 0 ? null : i;
}

/** The levels a tap passes through, after `from` up to and including `to`. */
export function travelPath(from: number, to: number): number[] {
  const step = to > from ? 1 : -1;
  const path: number[] = [];
  for (let i = from; i !== to; ) path.push((i += step));
  return path;
}

/** The segment under a finger `x` points into a track `width` wide. */
export function segmentAt(x: number, width: number, count: number): number {
  const i = Math.round(x / (width / count) - 0.5);
  return Math.min(count - 1, Math.max(0, i));
}

export interface ModeSegment {
  mode: SessionMode;
  label: "Ask first" | "Plan" | "Auto";
  hint: string;
  enabled: boolean;
}

const MODE_SEGMENTS: readonly Omit<ModeSegment, "enabled">[] = [
  { mode: "normal", label: "Ask first", hint: "Asks before running commands or editing files." },
  { mode: "plan", label: "Plan", hint: "Plans first, waits for your go before changing anything." },
  { mode: "unattended", label: "Auto", hint: "Runs commands and edits files without asking." },
];

/** The three modes in order; the ones the agent lacks are disabled. */
export function modeSegments(mode: ModeOption | null): ModeSegment[] {
  if (!mode) return [];
  return MODE_SEGMENTS.map((s) => ({ ...s, enabled: mode.available.includes(s.mode) }));
}

/** A session's agent can change only in a fork nothing was sent to yet, on a Host that allows it. */
export function agentLock({
  canSetAgent,
  forkedFrom,
  sent,
}: {
  canSetAgent: boolean;
  forkedFrom: boolean;
  sent: boolean;
}): "switchable" | "locked" {
  return canSetAgent && forkedFrom && !sent ? "switchable" : "locked";
}
