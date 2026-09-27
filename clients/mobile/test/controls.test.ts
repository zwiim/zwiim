import { initialSessionView, ZwiimClientError, type AgentSummary, type SessionOptions } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  agentName,
  changeRequest,
  contextChip,
  contextFigures,
  contextSummary,
  lockedNote,
  optionChips,
  optionsEditable,
  optionsError,
  tokens,
} from "../src/sessions/controls";

const choice = (current: string, ...values: [string, string][]) => ({
  current,
  values: values.map(([value, label]) => ({ value, label, description: null })),
});

// Two agents as the Host reports them; nothing below branches on which is which.
const claudeLike = (model: string, effort: string | null): SessionOptions => ({
  model: choice(model, ["opus[1m]", "Opus 5"], ["haiku", "Haiku 4.5"]),
  effort: effort === null ? null : choice(effort, ["low", "Low"], ["high", "High"]),
  mode: { current: "normal", label: "Manual", available: ["normal", "plan", "unattended"] },
});
const codexLike: SessionOptions = {
  model: choice("gpt-5.3-codex", ["gpt-5.3-codex", "GPT-5.3 Codex"]),
  effort: choice("medium", ["low", "Low"], ["medium", "Medium"], ["high", "High"]),
  mode: { current: "plan", label: "Plan", available: ["normal", "plan", "unattended"] },
};

describe("session controls", () => {
  it("names the agent as the Host does, never by vendor logic", () => {
    const agents = [{ agent: "claude_code", name: "Claude Code" }, { agent: "codex", name: "Codex" }] as AgentSummary[];
    expect(agentName(agents, "claude_code")).toBe("Claude Code");
    expect(agentName(agents, "codex")).toBe("Codex");
    expect(agentName(null, "gemini")).toBe("gemini");
    expect(agentName(agents, null)).toBeNull();
  });

  it("sends one option per change", () => {
    expect(changeRequest({ model: "haiku" })).toEqual({ model: "haiku" });
    expect(changeRequest({ effort: "low" })).toEqual({ effort: "low" });
    expect(changeRequest({ mode: "plan" })).toEqual({ mode: "plan" });
  });

  it("chips show the model's effort only while it offers one", () => {
    expect(optionChips(claudeLike("opus[1m]", "high"))).toEqual({ mode: { mode: "normal", label: "Ask" }, model: "Opus 5 · High" });
    // After a switch to Haiku the Host reports no effort, so none is shown or sent.
    expect(optionChips(claudeLike("haiku", null))).toEqual({ mode: { mode: "normal", label: "Ask" }, model: "Haiku 4.5" });
    expect(optionChips(codexLike)).toEqual({ mode: { mode: "plan", label: "Plan" }, model: "GPT-5.3 Codex · Medium" });
    expect(optionChips({ ...codexLike, mode: { current: "unattended", label: "Full access", available: ["normal", "unattended"] } }).mode)
      .toEqual({ mode: "unattended", label: "Unattended" });
    expect(optionChips({ model: null, effort: null, mode: null })).toEqual({ mode: null, model: null });
    expect(optionChips(null)).toEqual({ mode: null, model: null });
  });

  it("changes only between turns, while connected, once the agent has reported its options", () => {
    const base = { options: claudeLike("opus[1m]", "high"), connected: true, running: false, changing: false, hostName: "Studio Mac" };
    expect(optionsEditable(base)).toEqual({ ok: true });
    expect(optionsEditable({ ...base, running: true })).toEqual({ ok: false, reason: "Change these when the agent finishes its turn." });
    expect(optionsEditable({ ...base, connected: false })).toEqual({ ok: false, reason: "Not connected to Studio Mac." });
    expect(optionsEditable({ ...base, options: null }).ok).toBe(false);
    expect(optionsEditable({ ...base, changing: true }).ok).toBe(false);
  });

  it("says in the composer why the options wait for the turn", () => {
    expect(lockedNote(codexLike, true)).toBe("Options change when the turn ends");
    expect(lockedNote(codexLike, false)).toBeNull();
    expect(lockedNote(null, true)).toBeNull();
  });

  it("explains a refused change", () => {
    const e = (code: string) => new ZwiimClientError(code, "x", 409);
    expect(optionsError(e("turn_in_progress"), "Mac")).toBe("The agent is still working. Change this when it finishes.");
    expect(optionsError(e("option_unavailable"), "Mac")).toBe("The agent doesn't offer that now. Choose again.");
    expect(optionsError(e("network"), "Mac")).toBe("Not connected to Mac. Try again when it's back.");
    expect(optionsError(new Error("?"), "Mac")).toBe("Couldn't change that. Try again.");
  });
});

describe("context figures", () => {
  it("show what the agent reported, and say when it reported nothing", () => {
    const none = initialSessionView("s");
    expect(contextChip(none)).toBe("—");
    const empty = contextFigures(none);
    expect(empty.fraction).toBeNull();
    expect(empty.window.map((r) => r.value)).toEqual([null, null, null]);
    expect(empty.lastTurn).toBeNull();

    const reported = {
      ...none,
      usage: { used: 46210, size: 200000 },
      turnUsage: {
        total_tokens: 1200,
        input_tokens: 1000,
        output_tokens: 200,
        thought_tokens: null,
        cached_read_tokens: 800,
        cached_write_tokens: null,
      },
    };
    expect(contextChip(reported)).toBe("23%");
    const f = contextFigures(reported);
    expect(f.window).toEqual([
      { key: "used", label: "In use", value: "46,210" },
      { key: "size", label: "Window", value: "200,000" },
      { key: "left", label: "Left", value: "153,790" },
    ]);
    expect(Object.fromEntries(f.lastTurn!.map((r) => [r.key, r.value]))).toEqual({
      input: "1,000",
      output: "200",
      cached_read: "800",
      cached_write: null,
      thought: null,
      total: "1,200",
    });
  });

  it("never derive a figure from a missing one", () => {
    const view = { ...initialSessionView("s"), usage: { used: 5000, size: null } };
    expect(contextChip(view)).toBe("—");
    const f = contextFigures(view);
    expect(f.fraction).toBeNull();
    expect(f.window.map((r) => r.value)).toEqual(["5,000", null, null]);
  });

  it("summarise the window in one line, naming what wasn't reported", () => {
    const none = initialSessionView("s");
    expect(contextSummary(none)).toBeNull();
    expect(contextSummary({ usage: { used: null, size: null } })).toBeNull();
    expect(contextSummary({ usage: { used: 37738, size: 1000000 } })).toEqual({
      main: "37,738 of 1,000,000",
      aside: "962,262 left",
    });
    expect(contextSummary({ usage: { used: 5000, size: null } })).toEqual({
      main: "5,000 in use",
      aside: "window not reported",
    });
    expect(contextSummary({ usage: { used: null, size: 200000 } })).toEqual({
      main: "200,000 window",
      aside: "in use not reported",
    });
  });

  it("formats counts", () => {
    expect(tokens(1234567)).toBe("1,234,567");
    expect(tokens(0)).toBe("0");
  });
});
