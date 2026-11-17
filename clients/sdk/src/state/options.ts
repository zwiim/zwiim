import type { AgentSummary, ChoiceOption } from "../types/protocol";

/**
 * The effort a new session of `agent` can choose on `model`; null when that
 * model offers none. Effort belongs to the model (Claude has none on Haiku),
 * so the Host keeps each model's own once it has run there; for a model that
 * hasn't, the agent-wide effort is the best guess.
 */
export function effortFor(agent: AgentSummary, model: string | null | undefined): ChoiceOption | null {
  const known = agent.effort_by_model ?? {};
  if (model != null && Object.prototype.hasOwnProperty.call(known, model)) return known[model] ?? null;
  return agent.options?.effort ?? null;
}

/**
 * Whether a new session of `agent` can turn on fast mode on `model`. Like
 * effort, it belongs to the model; for a model that hasn't run here, the
 * agent-wide option is the best guess.
 */
export function fastFor(agent: AgentSummary, model: string | null | undefined): boolean {
  const known = agent.fast_by_model ?? {};
  if (model != null && Object.prototype.hasOwnProperty.call(known, model)) return known[model] === true;
  return agent.options?.fast != null;
}
