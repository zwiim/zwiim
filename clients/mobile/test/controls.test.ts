import { initialSessionView, ZwiimClientError, type AgentSummary, type SessionOptions } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import {
  agentName,
  changeRequest,
  compactTokens,
  contextChip,
  contextFigures,
  contextLine,
  contextSummary,
  filterChoices,
  lockedNote,
  needsConfirm,
  optionChips,
  optionRows,
  optionsEditable,
  optionsError,
  pageFor,
  settleOptions,
  SEARCH_FROM,
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

  it("chip reads mode, model and effort as one", () => {
    expect(optionChips(claudeLike("opus[1m]", "high"))).toEqual({ mode: { mode: "normal", label: "Ask" }, text: "Ask · Opus 5 · High" });
    // After a switch to Haiku the Host reports no effort, so none is shown or sent.
    expect(optionChips(claudeLike("haiku", null)).text).toBe("Ask · Haiku 4.5");
    expect(optionChips(codexLike).text).toBe("Plan · GPT-5.3 Codex · Medium");
    expect(optionChips(claudeLike("", null)).text).toBe("Ask · Default");
    expect(optionChips({ ...codexLike, mode: { current: "unattended", label: "Full access", available: ["normal", "unattended"] } }).mode)
      .toEqual({ mode: "unattended", label: "Unattended" });
    expect(optionChips({ model: null, effort: null, mode: null })).toEqual({ mode: null, text: null });
    expect(optionChips(null)).toEqual({ mode: null, text: null });
  });

  it("summary rows open only what can change", () => {
    expect(optionRows(claudeLike("opus[1m]", "high"), true)).toEqual([
      { key: "mode", label: "Mode", value: "Ask", open: true },
      { key: "model", label: "Model", value: "Opus 5", open: true },
      { key: "effort", label: "Effort", value: "High", open: true },
    ]);
    // Codex offers one model: shown, not openable.
    expect(optionRows(codexLike, true).find((r) => r.key === "model")).toEqual({ key: "model", label: "Model", value: "GPT-5.3 Codex", open: false });
    expect(optionRows(claudeLike("haiku", null), true).map((r) => r.key)).toEqual(["mode", "model"]);
    expect(optionRows(claudeLike("opus[1m]", "high"), false).every((r) => !r.open)).toBe(true);
    expect(optionRows({ ...codexLike, mode: { current: "normal", label: "", available: ["normal"] } }, true)[0]?.open).toBe(false);
    expect(optionRows(null, true)).toEqual([]);
  });

  it("summary rows keep an unlisted current value", () => {
    expect(optionRows(claudeLike("opus-old", null), true)[1]?.value).toBe("opus-old");
    expect(optionRows(claudeLike("", null), true)[1]?.value).toBe("Default");
  });

  it("a page that can no longer open falls back to the summary", () => {
    const rows = optionRows(claudeLike("opus[1m]", "high"), true);
    expect(pageFor("model", rows)).toBe("model");
    expect(pageFor("root", rows)).toBe("root");
    expect(pageFor("model", optionRows(claudeLike("opus[1m]", "high"), false))).toBe("root");
    expect(pageFor("effort", optionRows(claudeLike("haiku", null), true))).toBe("root");
    expect(pageFor("context", [])).toBe("context");
  });

  it("a page that fell back stays on the summary once its option unlocks", () => {
    const locked = optionRows(claudeLike("opus[1m]", "high"), false);
    const open = optionRows(claudeLike("opus[1m]", "high"), true);
    const settled = settleOptions({ page: "model", query: "op" }, locked);
    expect(settled).toEqual({ page: "root", query: "" });
    expect(pageFor(settled.page, open)).toBe("root");
    const kept = { page: "model" as const, query: "op" };
    expect(settleOptions(kept, open)).toBe(kept);
  });

  it("asks first only before Unattended", () => {
    expect(needsConfirm({ mode: "unattended" })).toBe(true);
    expect(needsConfirm({ mode: "plan" })).toBe(false);
    expect(needsConfirm({ model: "unattended" })).toBe(false);
  });

  it("filters models by name, id or description, in agent order", () => {
    const v = [
      { value: "opus", label: "Opus 5.5", description: "For complex work" },
      { value: "claude-opus-4-8", label: "Opus 4.8", description: null },
      { value: "haiku", label: "Haiku 5.5", description: "Fastest for quick answers" },
    ];
    expect(filterChoices(v, "  ")).toEqual(v);
    expect(filterChoices(v, "OPUS").map((x) => x.value)).toEqual(["opus", "claude-opus-4-8"]);
    expect(filterChoices(v, "4-8").map((x) => x.value)).toEqual(["claude-opus-4-8"]);
    expect(filterChoices(v, "quick").map((x) => x.value)).toEqual(["haiku"]);
    expect(filterChoices(v, "xyz")).toEqual([]);
    expect(SEARCH_FROM).toBe(8);
  });

  it("context line is short and names what is missing", () => {
    expect(compactTokens(950)).toBe("950");
    expect(compactTokens(200_000)).toBe("200K");
    expect(compactTokens(1_000_000)).toBe("1M");
    expect(compactTokens(1_500_000)).toBe("1.5M");
    expect(contextLine({ usage: { used: 46_000, size: 200_000 } } as never)).toBe("23% of 200K context");
    expect(contextLine({ usage: { used: 37_738, size: null } } as never)).toBe("37,738 in use");
    expect(contextLine({ usage: { used: null, size: 200_000 } } as never)).toBe("200K window");
    expect(contextLine(undefined)).toBe("Context not reported");
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
