import type { AgentSummary } from "@zwiim/sdk";
import { beforeEach, describe, expect, it } from "vitest";
import {
  favoriteModels,
  modelEffort,
  recentModels,
  rememberModelEffort,
  rememberRecentModel,
  toggleFavoriteModel,
} from "../src/lib/prefs";
import { matchCount, modelRows, quickRows, searchTarget } from "../src/panels/model-picker-state";

function agent(id: string, models: string[], effortByModel: Record<string, string[]> = {}, available = true): AgentSummary {
  return {
    agent: id,
    name: id,
    available,
    reason: null,
    support: "supported",
    adapter: null,
    options: {
      model: {
        current: models[0] ?? "",
        values: models.map((m) => ({ value: m, label: `${m} label`, description: `about ${m}` })),
      },
      effort: null,
      mode: null,
    },
    effort_by_model: Object.fromEntries(
      Object.entries(effortByModel).map(([m, vals]) => [
        m,
        { current: vals[0]!, values: vals.map((v) => ({ value: v, label: v, description: null })) },
      ]),
    ),
    commands: null,
  } as unknown as AgentSummary;
}

beforeEach(() => window.localStorage.clear());

describe("quickRows", () => {
  const agents = [agent("codex", ["gpt-5.5", "gpt-5"], { "gpt-5": ["low", "high"] }), agent("opencode", ["gpt-5.5", "x"]), agent("claude_code", ["opus", "sonnet"])];

  it("puts favorites first, then unfavorited recents, at most 2 recents", () => {
    const rows = quickRows(
      agents,
      [{ agent: "claude_code", model: "sonnet" }],
      [
        { agent: "claude_code", model: "sonnet", effort: null },
        { agent: "codex", model: "gpt-5", effort: "high" },
        { agent: "claude_code", model: "opus", effort: null },
        { agent: "opencode", model: "x", effort: null },
      ],
    );
    expect(rows.map((r) => `${r.ref.agent}/${r.ref.model}`)).toEqual(["claude_code/sonnet", "codex/gpt-5", "claude_code/opus"]);
    expect(rows[0]).toMatchObject({ label: "sonnet label", favorite: true });
    expect(rows[1]).toMatchObject({ effort: "high", favorite: false });
  });

  it("skips a favorite the agent no longer lists", () => {
    expect(quickRows(agents, [{ agent: "codex", model: "gone" }], [])).toEqual([]);
  });

  it("keys on agent and model", () => {
    const rows = quickRows(
      agents,
      [{ agent: "codex", model: "gpt-5.5" }],
      [{ agent: "opencode", model: "gpt-5.5", effort: null }],
    );
    expect(rows.map((r) => r.ref.agent)).toEqual(["codex", "opencode"]);
  });
});

describe("quickRows effort", () => {
  const agents = [agent("a", ["m", "n"], { m: ["low", "high"], n: ["low", "high"] })];

  it("falls back to the current effort when the stored one is no longer offered", () => {
    rememberModelEffort({ agent: "a", model: "m" }, "gone");
    const rows = quickRows(agents, [{ agent: "a", model: "m" }], [{ agent: "a", model: "n", effort: "gone" }]);
    expect(rows.map((r) => r.effort)).toEqual(["low", "low"]);
  });

  it("uses the current effort for a favorite with nothing remembered, and a valid stored one otherwise", () => {
    expect(quickRows(agents, [{ agent: "a", model: "m" }], [])[0]!.effort).toBe("low");
    expect(quickRows(agents, [], [{ agent: "a", model: "n", effort: "high" }])[0]!.effort).toBe("high");
  });
});

describe("modelRows", () => {
  const many = agent("a", Array.from({ length: 9 }, (_, i) => `m${i}`));

  it("caps at 5 and reports the rest", () => {
    const { rows, more } = modelRows(many, "", false);
    expect(rows).toHaveLength(5);
    expect(more).toBe(4);
  });

  it("shows all when expanded", () => {
    expect(modelRows(many, "", true)).toMatchObject({ more: 0 });
    expect(modelRows(many, "", true).rows).toHaveLength(9);
  });

  it("filters case-insensitively and never caps a query", () => {
    const { rows, more } = modelRows(many, "M", false);
    expect(rows).toHaveLength(9);
    expect(more).toBe(0);
    expect(modelRows(many, "ABOUT M3", false).rows.map((r) => r.value)).toEqual(["m3"]);
    expect(matchCount(many, "m1")).toBe(1);
  });

  it("resolves effort from the remembered value, else current, else null", () => {
    const a = agent("a", ["m", "n"], { m: ["low", "high"] });
    expect(modelRows(a, "", true).rows.map((r) => r.effort)).toEqual(["low", null]);
    rememberModelEffort({ agent: "a", model: "m" }, "high");
    expect(modelRows(a, "", true).rows[0]!.effort).toBe("high");
    rememberModelEffort({ agent: "a", model: "m" }, "gone");
    expect(modelRows(a, "", true).rows[0]!.effort).toBe("low");
  });
});

describe("searchTarget", () => {
  const agents = [agent("claude_code", ["opus"]), agent("codex", ["gpt-5"])];
  it("moves to the agent with a match", () => {
    expect(searchTarget(agents, "claude_code", "gpt")).toBe("codex");
  });
  it("stays when the browsed agent matches or nothing does", () => {
    expect(searchTarget(agents, "claude_code", "opus")).toBe("claude_code");
    expect(searchTarget(agents, "claude_code", "zzz")).toBe("claude_code");
  });
});

describe("picker prefs", () => {
  it("dedupes recents and keeps 5", () => {
    for (let i = 0; i < 7; i++) rememberRecentModel({ agent: "a", model: `m${i}` }, null);
    rememberRecentModel({ agent: "a", model: "m4" }, "high");
    const recents = recentModels();
    expect(recents).toHaveLength(5);
    expect(recents[0]).toEqual({ agent: "a", model: "m4", effort: "high" });
    expect(recents.filter((r) => r.model === "m4")).toHaveLength(1);
  });

  it("toggles favorites and remembers effort", () => {
    const ref = { agent: "a", model: "m" };
    expect(toggleFavoriteModel(ref)).toBe(true);
    expect(favoriteModels()).toEqual([ref]);
    expect(toggleFavoriteModel(ref)).toBe(false);
    expect(favoriteModels()).toEqual([]);
    expect(modelEffort(ref)).toBeNull();
    rememberModelEffort(ref, "high");
    expect(modelEffort(ref)).toBe("high");
  });
});
