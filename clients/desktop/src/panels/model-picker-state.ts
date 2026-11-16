import { effortFor, type AgentSummary, type OptionValue } from "@zwiim/sdk";
import { modelEffort, type ModelRef } from "../lib/prefs";

export interface QuickRow {
  ref: ModelRef;
  label: string;
  effort: string | null;
  favorite: boolean;
}

export interface ModelRow {
  value: string;
  label: string;
  description: string | null;
  effort: string | null;
}

const COLLAPSED_ROWS = 5;

function models(agent: AgentSummary): OptionValue[] {
  return agent.options?.model?.values ?? [];
}

/** Favorites first, then the most recent models that are not favorites. */
export function quickRows(
  agents: AgentSummary[],
  favorites: ModelRef[],
  recents: (ModelRef & { effort: string | null })[],
  maxRecent = 2,
): QuickRow[] {
  const rowFor = (ref: ModelRef, effort: string | null, favorite: boolean): QuickRow[] => {
    const found = agents.find((a) => a.agent === ref.agent && models(a).some((m) => m.value === ref.model));
    const option = found && models(found).find((m) => m.value === ref.model);
    return option ? [{ ref: { agent: ref.agent, model: ref.model }, label: option.label, effort, favorite }] : [];
  };
  const isFavorite = (ref: ModelRef) => favorites.some((f) => f.agent === ref.agent && f.model === ref.model);
  const favoriteRows = favorites.flatMap((f) => rowFor(f, modelEffort(f), true));
  const recentRows = recents.filter((r) => !isFavorite(r)).flatMap((r) => rowFor(r, r.effort, false));
  return [...favoriteRows, ...recentRows.slice(0, maxRecent)];
}

function matches(model: OptionValue, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [model.label, model.value, model.description ?? ""].some((s) => s.toLowerCase().includes(q));
}

export function matchCount(agent: AgentSummary, query: string): number {
  return models(agent).filter((m) => matches(m, query)).length;
}

export function modelRows(agent: AgentSummary, query: string, expanded: boolean): { rows: ModelRow[]; more: number } {
  const found = models(agent).filter((m) => matches(m, query));
  const capped = !query.trim() && !expanded && found.length > COLLAPSED_ROWS;
  const shown = capped ? found.slice(0, COLLAPSED_ROWS) : found;
  const rows = shown.map((m): ModelRow => {
    const effort = effortFor(agent, m.value);
    const remembered = modelEffort({ agent: agent.agent, model: m.value });
    const chosen = remembered && effort?.values.some((v) => v.value === remembered) ? remembered : (effort?.current ?? null);
    return { value: m.value, label: m.label, description: m.description, effort: chosen };
  });
  return { rows, more: found.length - shown.length };
}

/** The agent to browse while searching: the current one if it matches, else the first available that does. */
export function searchTarget(agents: AgentSummary[], browsed: string, query: string): string {
  const current = agents.find((a) => a.agent === browsed);
  if (!query.trim() || (current && matchCount(current, query) > 0)) return browsed;
  return agents.find((a) => a.available && matchCount(a, query) > 0)?.agent ?? browsed;
}
