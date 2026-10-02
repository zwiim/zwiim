import { buildWorkspaceTree, ZwiimClientError, type AgentSummary, type SessionOptions, type WorkspaceInfo } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  agentNote,
  choiceValue,
  defaultAgent,
  modeChoosable,
  modeValue,
  offeredAgents,
  startChoices,
  startError,
  startOptions,
  startParams,
  startTarget,
  type StartTarget,
} from "../src/sessions/start";
import { worktreeChoices } from "../src/host/workspace";
import { worktreeSessions } from "../src/sessions/worktree";

const ws = (id: string, over: Partial<WorkspaceInfo> = {}): WorkspaceInfo => ({
  workspace_id: id,
  name: id,
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
  is_git_repo: true,
  display_path: null,
  ...over,
});
const wt = (id: string, root: string, over: Partial<WorkspaceInfo> = {}) =>
  ws(id, { kind: "worktree", parent_workspace_id: root, branch: `zwiim/${id}`, base_branch: "main", ...over });

const LIST = [
  ws("zwiim", { display_path: "~/code/zwiim" }),
  wt("fix", "zwiim", { branch: "fix-auth" }),
  wt("old", "zwiim", { removed: true, branch: "old-branch" }),
  ws("notes", { is_git_repo: false, branch: null }),
];

const OPTIONS: SessionOptions = {
  model: {
    current: "sonnet",
    values: [
      { value: "sonnet", label: "Claude Sonnet", description: null },
      { value: "opus", label: "Claude Opus", description: "Most capable" },
    ],
  },
  effort: { current: "medium", values: ["low", "medium", "high"].map((v) => ({ value: v, label: v, description: null })) },
  mode: { current: "normal", label: "Default", available: ["normal", "plan", "unattended"] },
};

const agent = (id: string, over: Partial<AgentSummary> = {}): AgentSummary => ({
  agent: id,
  name: id,
  available: true,
  reason: null,
  support: "experimental",
  adapter: null,
  options: null,
  effort_by_model: {},
  commands: null,
  ...over,
});
const CLAUDE = agent("claude_code", { name: "Claude Code", support: "supported", options: OPTIONS });

const ready = (id: string): StartTarget => startTarget(LIST, id);

describe("where a new session starts", () => {
  it("starts in the root checkout when the root is chosen", () => {
    expect(ready("zwiim")).toMatchObject({ kind: "ready", workspace: { workspace_id: "zwiim" }, label: "zwiim · main" });
    const params = startParams({ target: ready("zwiim"), agent: CLAUDE, picks: {}, prompt: "hi", client_message_id: "m" });
    expect(params?.workspace_id).toBe("zwiim");
  });

  it("starts in the chosen worktree, labelled workspace · worktree", () => {
    expect(ready("fix")).toMatchObject({ kind: "ready", workspace: { workspace_id: "fix" }, label: "zwiim · fix-auth" });
    const params = startParams({ target: ready("fix"), agent: CLAUDE, picks: {}, prompt: "hi", client_message_id: "m" });
    expect(params?.workspace_id).toBe("fix");
  });

  it("never swaps a chosen worktree for its root", () => {
    // Removed: no target at all, not the root that Files falls back to.
    expect(startTarget(LIST, "old")).toEqual({ kind: "removed", label: "zwiim · old-branch" });
    // Gone from the host entirely.
    expect(startTarget(LIST, "vanished")).toEqual({ kind: "removed", label: null });
    // Only when nothing was ever chosen does it default to the first root.
    expect(startTarget(LIST, null)).toMatchObject({ kind: "ready", workspace: { workspace_id: "zwiim" } });
    expect(startTarget(null, "fix")).toEqual({ kind: "loading" });
    expect(startTarget([], null)).toEqual({ kind: "none" });
  });

  it("can't start in a removed worktree", () => {
    const target = startTarget(LIST, "old");
    expect(startParams({ target, agent: CLAUDE, picks: {}, prompt: "hi", client_message_id: "m" })).toBeNull();
    const tree = buildWorkspaceTree({ workspaces: LIST, sessionsByWorkspace: {}, sessions: {} });
    expect(worktreeChoices(tree, "zwiim").map((n) => n.workspace.workspace_id)).toEqual(["zwiim", "fix"]);
  });
});

describe("the session.start request", () => {
  it("sends every option the agent offers, as Desktop does", () => {
    const params = startParams({
      target: ready("fix"),
      agent: CLAUDE,
      picks: { model: "opus", effort: "high", mode: "plan" },
      prompt: "  fix the login bug \n",
      client_message_id: "msg-1",
    });
    expect(params).toEqual({
      workspace_id: "fix",
      agent: "claude_code",
      blocks: [{ type: "text", text: "fix the login bug" }],
      client_message_id: "msg-1",
      options: { model: "opus", effort: "high", mode: "plan" },
    });
  });

  it("sends model and effort as the agent has them, and mode only when changed", () => {
    expect(startOptions(OPTIONS, {})).toEqual({ model: "sonnet", effort: "medium" });
    expect(startOptions(OPTIONS, { mode: "normal" })).toEqual({ model: "sonnet", effort: "medium" });
    expect(startOptions(OPTIONS, { mode: "unattended" })).toEqual({ model: "sonnet", effort: "medium", mode: "unattended" });
    // A pick the agent no longer offers falls back to its own.
    expect(startOptions(OPTIONS, { model: "gone" }).model).toBe("sonnet");
    expect(startOptions({ ...OPTIONS, mode: { ...OPTIONS.mode!, available: ["normal"] } }, { mode: "plan" }).mode).toBeUndefined();
  });

  it("offers and sends the effort of the model picked, none on a model without one", () => {
    const low_high = { current: "low", values: ["low", "high"].map((v) => ({ value: v, label: v, description: null })) };
    const claude = agent("claude_code", { options: OPTIONS, effort_by_model: { sonnet: low_high, opus: null } });
    // Sonnet (the agent's model) has run here: its own levels, not the agent-wide ones.
    expect(startChoices(claude, {})?.effort).toEqual(low_high);
    expect(startOptions(startChoices(claude, {}), {})).toEqual({ model: "sonnet", effort: "low" });
    // Opus offers none: no Effort row, and none is sent even if one was picked.
    const opus = { model: "opus", effort: "high" };
    expect(startChoices(claude, opus)?.effort).toBeNull();
    const params = startParams({ target: ready("zwiim"), agent: claude, picks: opus, prompt: "hi", client_message_id: "m" });
    expect(params?.options).toEqual({ model: "opus" });
    // Back on Sonnet the pick holds while that model offers it.
    expect(startOptions(startChoices(claude, { ...opus, model: "sonnet" }), { ...opus, model: "sonnet" })).toEqual({ model: "sonnet", effort: "high" });
    // A model that hasn't run here yet keeps the agent-wide effort.
    expect(startChoices(agent("claude_code", { options: OPTIONS }), { model: "opus" })?.effort).toEqual(OPTIONS.effort);
  });

  it("sends no options for an agent that hasn't advertised any", () => {
    const params = startParams({ target: ready("zwiim"), agent: agent("codex"), picks: {}, prompt: "hi", client_message_id: "m" });
    expect(params).toEqual({ workspace_id: "zwiim", agent: "codex", blocks: [{ type: "text", text: "hi" }], client_message_id: "m" });
  });

  it("needs a runnable agent and a prompt", () => {
    const base = { target: ready("zwiim"), picks: {}, client_message_id: "m" };
    expect(startParams({ ...base, agent: CLAUDE, prompt: "   " })).toBeNull();
    expect(startParams({ ...base, agent: null, prompt: "hi" })).toBeNull();
    expect(startParams({ ...base, agent: agent("gemini", { available: false, reason: "not_installed" }), prompt: "hi" })).toBeNull();
  });
});

describe("the choices", () => {
  it("defaults to the first available agent and names why others can't run", () => {
    const list = [agent("gemini", { available: false, reason: "not_installed" }), agent("codex", { support: "supported" }), CLAUDE];
    expect(defaultAgent(list)).toBe("codex");
    expect(defaultAgent([list[0]!])).toBeNull();
    expect(defaultAgent(null)).toBeNull();
    expect(agentNote(list[0]!)).toBe("not installed");
    expect(agentNote(agent("x", { available: false, reason: "unsupported_platform" }))).toBe("not supported here");
    expect(agentNote(agent("x", { available: false, reason: "disabled" }))).toBe("disabled");
    expect(agentNote(agent("x", { available: false, reason: "installing" }))).toBe("setting up");
    expect(agentNote(agent("x", { available: false, reason: "needs_network" }))).toBe("setup needs network");
    expect(agentNote(agent("x", { available: false, reason: "setup_failed" }))).toBe("setup failed");
    expect(agentNote(CLAUDE)).toBeNull();
  });

  it("offers only Claude Code and Codex for a new session", () => {
    const list = [agent("gemini", { support: "experimental" }), agent("codex", { support: "supported" }), CLAUDE];
    expect(offeredAgents(list)?.map((a) => a.agent)).toEqual(["codex", CLAUDE.agent]);
    expect(offeredAgents(null)).toBeNull();
  });

  it("labels agents Zwiim does not support yet, and prefers a supported one", () => {
    const list = [agent("gemini"), agent("opencode", { available: false, reason: "not_installed" }), CLAUDE];
    expect(agentNote(list[0]!)).toBe("experimental");
    expect(agentNote(list[1]!)).toBe("not installed");
    expect(defaultAgent(list)).toBe("claude_code");
    expect(defaultAgent([list[0]!])).toBe("gemini");
  });

  it("shows the pick while offered, else the agent's current", () => {
    expect(choiceValue(OPTIONS.model!, "opus")).toBe("opus");
    expect(choiceValue(OPTIONS.model!, "gone")).toBe("sonnet");
    expect(modeValue(OPTIONS, undefined)).toBe("normal");
    expect(modeValue(OPTIONS, "plan")).toBe("plan");
    expect(modeValue(null, "plan")).toBeNull();
    expect(modeChoosable(OPTIONS)).toBe(true);
    expect(modeChoosable({ ...OPTIONS, mode: { ...OPTIONS.mode!, available: ["normal"] } })).toBe(false);
  });

  it("says why a start failed", () => {
    const err = (code: string) => new ZwiimClientError(code, "x");
    expect(startError(err("workspace_removed"), "Studio", null)).toBe("That worktree was removed. Choose another workspace.");
    expect(startError(err("agent_not_installed"), "Studio", "Codex")).toBe("Codex isn't installed on Studio.");
    expect(startError(err("unknown_outcome"), "Studio", null)).toBe("Not sure the session started. Check Sessions before starting again.");
    expect(startError(err("network"), "Studio", null)).toBe("Not connected to Studio. Try again when it's back.");
    expect(startError(new Error("boom"), "Studio", null)).toBe("Couldn't start the session. Try again.");
  });
});

describe("a worktree's session strip", () => {
  it("names a worktree by branch and a plain folder by name; an unlisted one has no strip", () => {
    const state = { workspaces: LIST, sessionsByWorkspace: {}, sessions: {} };
    expect(worktreeSessions(state, "fix")).toEqual({ workspace_id: "fix", label: "⎇ fix-auth", removed: false, sessions: [] });
    expect(worktreeSessions(state, "notes")?.label).toBe("notes");
    expect(worktreeSessions(state, "old")?.removed).toBe(true);
    expect(worktreeSessions(state, "vanished")).toBeNull();
  });
});
