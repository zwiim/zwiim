// Starting a session from the phone (DESIGN.md, Screen 4, New session): where
// it runs, which agent, the options that agent offers, and the first prompt,
// sent as one `session.start` the way Desktop sends it. The target is the
// worktree chosen in Files, never quietly swapped for its root: a chosen
// worktree that was removed asks for another choice instead.

import {
  isClientError,
  type AgentSummary,
  type ChoiceOption,
  type OpParams,
  type OptionsRequest,
  type SessionMode,
  type SessionOptions,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { contextLabel, contextText, currentWorkspace, workspaceLabel } from "../host/workspace";

/** Where a new session would run. */
export type StartTarget =
  | { kind: "loading" }
  /** No workspace on the host yet. */
  | { kind: "none" }
  /** The chosen worktree was removed (or is gone): choose another. */
  | { kind: "removed"; label: string | null }
  | { kind: "ready"; workspace: WorkspaceInfo; label: string };

/**
 * The workspace a new session starts in: the one chosen, while the host still
 * has it. Only when nothing was ever chosen does it default to the first root.
 */
export function startTarget(workspaces: readonly WorkspaceInfo[] | null, chosen: string | null): StartTarget {
  if (workspaces === null) return { kind: "loading" };
  if (chosen !== null) {
    const w = workspaces.find((x) => x.workspace_id === chosen);
    if (!w || w.removed) return { kind: "removed", label: w ? contextText(contextLabel(workspaces, chosen)) : null };
    return { kind: "ready", workspace: w, label: targetLabel(workspaces, w) };
  }
  const w = currentWorkspace(workspaces, null);
  return w ? { kind: "ready", workspace: w, label: targetLabel(workspaces, w) } : { kind: "none" };
}

function targetLabel(workspaces: readonly WorkspaceInfo[], w: WorkspaceInfo): string {
  return contextText(contextLabel(workspaces, w.workspace_id)) ?? workspaceLabel(w);
}

/** The first agent the host can run, in the host's order. */
export function defaultAgent(agents: readonly AgentSummary[] | null): string | null {
  return agents?.find((a) => a.available)?.agent ?? null;
}

/** Why an agent can't start a session here, in Desktop's words; null when it can. */
export function agentNote(agent: AgentSummary): string | null {
  if (agent.available) return null;
  if (agent.reason === "not_installed") return "not installed";
  if (agent.reason === "unsupported_platform") return "not supported here";
  if (agent.reason === "disabled") return "disabled";
  return "unavailable";
}

/** What the user picked in the sheet; unpicked options keep the agent's own. */
export type StartPicks = OptionsRequest;

export const MODE_LABELS: Record<SessionMode, { label: string; hint: string }> = {
  normal: { label: "Ask", hint: "Asks before running commands or editing files" },
  plan: { label: "Plan", hint: "Plans first and asks you to approve the plan" },
  unattended: { label: "Unattended", hint: "Runs commands and edits files without asking" },
};

/** The value a choice shows: the pick while the agent still offers it, else the agent's current. */
export function choiceValue(option: ChoiceOption, pick: string | undefined): string {
  return pick !== undefined && option.values.some((v) => v.value === pick) ? pick : option.current;
}

export function choiceLabel(option: ChoiceOption, value: string): string {
  return option.values.find((v) => v.value === value)?.label ?? value;
}

/** The mode a new session starts in: the pick while offered, else the agent's (always Ask for a new session). */
export function modeValue(options: SessionOptions | null, pick: SessionMode | undefined): SessionMode | null {
  const mode = options?.mode;
  if (!mode) return null;
  return pick !== undefined && mode.available.includes(pick) ? pick : mode.current;
}

/** Whether the mode is a choice: the agent offers more than one. */
export function modeChoosable(options: SessionOptions | null): boolean {
  return (options?.mode?.available.length ?? 0) > 1;
}

/**
 * `session.start`'s `options`, as Desktop builds them: model and effort
 * whenever the agent offers them, mode only when picked and different from
 * the agent's.
 */
export function startOptions(options: SessionOptions | null, picks: StartPicks): OptionsRequest {
  const request: OptionsRequest = {};
  if (options?.model) request.model = choiceValue(options.model, picks.model);
  if (options?.effort) request.effort = choiceValue(options.effort, picks.effort);
  const mode = options?.mode;
  if (mode && picks.mode && picks.mode !== mode.current && mode.available.includes(picks.mode)) request.mode = picks.mode;
  return request;
}

export interface StartInput {
  target: StartTarget;
  agent: AgentSummary | null;
  picks: StartPicks;
  prompt: string;
  client_message_id: string;
}

/** The `session.start` request, or null while something is missing (a live target, a runnable agent, a prompt). */
export function startParams({ target, agent, picks, prompt, client_message_id }: StartInput): OpParams<"session.start"> | null {
  const text = prompt.trim();
  if (target.kind !== "ready" || !agent?.available || !text) return null;
  const options = startOptions(agent.options, picks);
  return {
    workspace_id: target.workspace.workspace_id,
    agent: agent.agent,
    blocks: [{ type: "text", text }],
    client_message_id,
    ...(Object.keys(options).length > 0 ? { options } : {}),
  };
}

/** Why a start failed, and what to do. Nothing was created unless it says so. */
export function startError(e: unknown, hostName: string, agentName: string | null): string {
  if (isClientError(e, "workspace_removed")) return "That worktree was removed. Choose another workspace.";
  if (isClientError(e, "not_found")) return `That workspace is gone from ${hostName}. Choose another.`;
  if (isClientError(e, "agent_not_installed")) return `${agentName ?? "That agent"} isn't installed on ${hostName}.`;
  if (isClientError(e, "option_unavailable")) return "The agent no longer offers that option. Choose again.";
  if (isClientError(e, "unknown_outcome")) return "Not sure the session started. Check Sessions before starting again.";
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't start the session. Try again.";
}
