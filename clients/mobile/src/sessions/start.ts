// Starting a session from the phone (DESIGN.md, Screen 4, New session): where
// it runs, which agent, the options that agent offers, and the first prompt,
// sent as one `session.start` the way Desktop sends it. The target is the
// worktree chosen in Files, never quietly swapped for its root: a chosen
// worktree that was removed asks for another choice instead. Sessions never
// run in a repository's root: chosen there, Start makes a new worktree first,
// the way Desktop opens a repository.

import {
  ATTACHMENT_GONE_MESSAGE,
  effortFor,
  isClientError,
  type AgentSummary,
  type ChoiceOption,
  type HostState,
  type OpParams,
  type OptionsRequest,
  type PromptBlock,
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
  | { kind: "ready"; workspace: WorkspaceInfo; label: string }
  /** A root: Start makes a worktree from its branch (a plain folder becomes a repository first) and runs there. */
  | { kind: "new_worktree"; root: WorkspaceInfo; label: string };

/**
 * The workspace a new session starts in: the one chosen, while the host still
 * has it. Only when nothing was ever chosen does it default: to the worktree
 * Files shows, else a new worktree of the first root.
 */
export function startTarget(workspaces: readonly WorkspaceInfo[] | null, chosen: string | null): StartTarget {
  if (workspaces === null) return { kind: "loading" };
  if (chosen !== null) {
    const w = workspaces.find((x) => x.workspace_id === chosen);
    if (!w || w.removed) return { kind: "removed", label: w ? contextText(contextLabel(workspaces, chosen)) : null };
    return liveTarget(workspaces, w);
  }
  const w = currentWorkspace(workspaces, null) ?? workspaces.find((x) => !x.removed && x.kind === "root");
  return w ? liveTarget(workspaces, w) : { kind: "none" };
}

/**
 * Where New session starts when nothing was picked in it: where Files is. A
 * chosen root stands for the worktree Files opens there, else a new one in
 * it; a chosen worktree that was removed says so instead of moving elsewhere.
 */
export function filesTarget(
  workspaces: readonly WorkspaceInfo[] | null,
  chosen: string | null,
  sessionsByWorkspace: HostState["sessionsByWorkspace"] = {},
): StartTarget {
  if (workspaces === null) return { kind: "loading" };
  const w = workspaces.find((x) => x.workspace_id === chosen);
  if (w?.removed && w.kind === "worktree") return startTarget(workspaces, chosen);
  const open = currentWorkspace(workspaces, chosen, sessionsByWorkspace);
  if (open) return startTarget(workspaces, open.workspace_id);
  return startTarget(workspaces, w && !w.removed ? chosen : null);
}

function liveTarget(workspaces: readonly WorkspaceInfo[], w: WorkspaceInfo): StartTarget {
  if (w.kind === "root") return { kind: "new_worktree", root: w, label: `${workspaceLabel(w)} · new worktree` };
  return { kind: "ready", workspace: w, label: targetLabel(workspaces, w) };
}

function targetLabel(workspaces: readonly WorkspaceInfo[], w: WorkspaceInfo): string {
  return contextText(contextLabel(workspaces, w.workspace_id)) ?? workspaceLabel(w);
}

/** The agents a new session offers: the supported ones (Claude Code, Codex). */
export function offeredAgents(agents: readonly AgentSummary[] | null): AgentSummary[] | null {
  return agents && agents.filter((a) => a.support === "supported");
}

/**
 * The host's default agent if it can run it, else the first supported agent
 * it can run, else the first it can run, in the host's order.
 */
export function defaultAgent(agents: readonly AgentSummary[] | null): string | null {
  const usable = agents?.filter((a) => a.available) ?? [];
  return (usable.find((a) => a.default) ?? usable.find((a) => a.support === "supported") ?? usable[0])?.agent ?? null;
}

/**
 * Why an agent can't start a session here, in Desktop's words, or that Zwiim
 * does not support it yet; null for a supported agent that can start.
 */
export function agentNote(agent: AgentSummary): string | null {
  if (agent.available) return agent.support === "experimental" ? "experimental" : null;
  if (agent.reason === "not_installed") return "not installed";
  if (agent.reason === "unsupported_platform") return "not supported here";
  if (agent.reason === "disabled") return "disabled";
  if (agent.reason === "installing") return "setting up";
  if (agent.reason === "needs_network") return "setup needs network";
  if (agent.reason === "setup_failed") return "setup failed";
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

/**
 * What a new session of `agent` can choose with these picks: the agent's
 * options, with the effort of the model picked (none on a model without one).
 */
export function startChoices(agent: AgentSummary | null, picks: StartPicks): SessionOptions | null {
  const options = agent?.options ?? null;
  if (!agent || !options) return options;
  const model = options.model ? choiceValue(options.model, picks.model) : null;
  return { ...options, effort: effortFor(agent, model) };
}

export interface StartInput {
  target: StartTarget;
  agent: AgentSummary | null;
  picks: StartPicks;
  prompt: string;
  /** The uploaded files, after the text. */
  files?: readonly PromptBlock[];
  client_message_id: string;
}

/** Whether Start can go: a live target (or a root to make a worktree in), a runnable agent, a prompt or a file. */
export function canStart({ target, agent, prompt, files = [] }: StartInput): boolean {
  return (target.kind === "ready" || target.kind === "new_worktree") && !!agent?.available && (!!prompt.trim() || files.length > 0);
}

/** The `session.start` request, or null while something is missing or the worktree isn't made yet. */
export function startParams(input: StartInput): OpParams<"session.start"> | null {
  const { target, agent, picks, prompt, files = [], client_message_id } = input;
  if (target.kind !== "ready" || !agent || !canStart(input)) return null;
  const text = prompt.trim();
  const options = startOptions(startChoices(agent, picks), picks);
  return {
    workspace_id: target.workspace.workspace_id,
    agent: agent.agent,
    blocks: [...(text ? [{ type: "text" as const, text }] : []), ...files],
    client_message_id,
    ...(Object.keys(options).length > 0 ? { options } : {}),
  };
}

/** Why a start failed, and what to do. Nothing was created unless it says so. */
export function startError(e: unknown, hostName: string, agentName: string | null): string {
  if (isClientError(e, "workspace_removed")) return "That worktree was archived. Choose another workspace.";
  if (isClientError(e, "not_found")) return `That workspace is gone from ${hostName}. Choose another.`;
  if (isClientError(e, "root_workspace")) return "Sessions run in a worktree, not the workspace folder. Choose a worktree.";
  if (isClientError(e, "agent_not_installed")) return `${agentName ?? "That agent"} isn't installed on ${hostName}.`;
  if (isClientError(e, "attachment_not_found")) return ATTACHMENT_GONE_MESSAGE;
  if (isClientError(e, "option_unavailable")) return "The agent no longer offers that option. Choose again.";
  if (isClientError(e, "unknown_outcome")) return "Not sure the session started. Check Sessions before starting again.";
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't start the session. Try again.";
}
