import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { NativeBridge } from "@/lib/bridge";
import {
  registryProblems,
  SETTINGS,
  TABS,
  visibleSettings,
  visibleTabs,
  type SettingDef,
  type SettingsContext,
  type TabDef,
} from "@/settings/registry";
import { searchSettings } from "@/settings/search";

function bridge(extra: Partial<NativeBridge> = {}): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: true, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u", email: null })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
    ...extra,
  };
}

function ctx(over: Partial<SettingsContext> = {}): SettingsContext {
  return {
    bridge: bridge(),
    remote: false,
    account: { phase: "loading" } as unknown as SettingsContext["account"],
    updateHint: false,
    githubOffered: false,
    hostSettings: false,
    instructions: false,
    gitSettings: false,
    promptSettings: false,
    repositorySettings: false,
    advancedSettings: false,
    browser: false,
    steer: false,
    attachments: false,
    autoMerge: false,
    ...over,
  };
}

const ids = (xs: { id: string }[]) => xs.map((x) => x.id);

describe("settings registry", () => {
  it("declares every tab and setting once, each setting in a declared tab", () => {
    expect(registryProblems()).toEqual([]);
  });

  it("names an id used twice and a setting in an unknown tab", () => {
    const tab: TabDef = { ...TABS[0]! };
    const row: SettingDef = { ...SETTINGS.find((s) => s.tab === tab.id)! };
    const stray = { ...row, id: "stray", tab: "nowhere" } as unknown as SettingDef;
    expect(registryProblems([tab, tab], [row, row, stray])).toEqual([
      `tab "${tab.id}" is declared twice`,
      `setting "${row.id}" is declared twice`,
      `setting "stray" is in unknown tab "nowhere"`,
    ]);
  });

  it("offers devices for this computer's host, and the connection for a remote one", () => {
    expect(ids(visibleTabs(ctx()))).toEqual(["general", "appearance", "account", "agents", "devices", "about"]);
    const link = {} as SettingsContext["link"];
    expect(ids(visibleTabs(ctx({ remote: true, link, githubOffered: true })))).toEqual([
      "general",
      "appearance",
      "account",
      "agents",
      "github",
      "connection",
      "about",
    ]);
  });

  it("offers the Models tab where the host keeps shared settings", () => {
    expect(ids(visibleTabs(ctx({ hostSettings: true })))).toEqual([
      "general",
      "appearance",
      "account",
      "agents",
      "models",
      "devices",
      "about",
    ]);
    expect(searchSettings("thinking", ctx({ hostSettings: true })).map((h) => h.target)).toContainEqual({ tab: "models" });
    expect(searchSettings("thinking", ctx()).map((h) => h.target)).not.toContainEqual({ tab: "models" });
  });

  it("offers the Git tab where the host keeps how it names branches", () => {
    expect(ids(visibleTabs(ctx({ hostSettings: true, gitSettings: true })))).toEqual([
      "general",
      "appearance",
      "account",
      "agents",
      "models",
      "git",
      "devices",
      "about",
    ]);
    expect(searchSettings("prefix", ctx({ gitSettings: true })).map((h) => h.target)).toContainEqual({ tab: "git" });
    expect(searchSettings("prefix", ctx()).map((h) => h.target)).not.toContainEqual({ tab: "git" });
  });

  it("offers the Prompts tab where the host keeps the user's pull request and branch prompts", () => {
    expect(ids(visibleTabs(ctx({ hostSettings: true, gitSettings: true, promptSettings: true })))).toEqual([
      "general",
      "appearance",
      "account",
      "agents",
      "models",
      "git",
      "prompts",
      "devices",
      "about",
    ]);
    expect(searchSettings("pull request", ctx({ promptSettings: true })).map((h) => h.target)).toContainEqual({
      tab: "prompts",
    });
    expect(searchSettings("pull request", ctx()).map((h) => h.target)).not.toContainEqual({ tab: "prompts" });
  });

  it("offers the Instructions tab where the host keeps the user's instructions", () => {
    expect(ids(visibleTabs(ctx({ hostSettings: true, instructions: true })))).toEqual([
      "general",
      "appearance",
      "account",
      "agents",
      "models",
      "instructions",
      "devices",
      "about",
    ]);
    expect(searchSettings("instructions", ctx({ instructions: true })).map((h) => h.target)).toContainEqual({
      tab: "instructions",
    });
    expect(searchSettings("instructions", ctx()).map((h) => h.target)).not.toContainEqual({ tab: "instructions" });
  });

  it("offers the Advanced tab where the host keeps its terminal and push settings", () => {
    expect(ids(visibleTabs(ctx({ advancedSettings: true })))).toEqual([
      "general",
      "appearance",
      "account",
      "agents",
      "devices",
      "advanced",
      "about",
    ]);
    expect(searchSettings("scrollback", ctx({ advancedSettings: true })).map((h) => h.target)).toContainEqual({
      tab: "advanced",
    });
    expect(searchSettings("scrollback", ctx()).map((h) => h.target)).not.toContainEqual({ tab: "advanced" });
  });

  it("offers a setting only where this window can do it", () => {
    expect(ids(visibleSettings("general", ctx()))).toEqual([
      "send-key",
      "expand-tool-calls",
      "always-show-context",
      "turn-sound",
      "keep-reachable",
    ]);
    const login = { status: vi.fn(), set: vi.fn() };
    const notifier = { notify: vi.fn() };
    expect(ids(visibleSettings("general", ctx({ bridge: bridge({ hostLogin: login, notifier }) })))).toEqual([
      "send-key",
      "expand-tool-calls",
      "always-show-context",
      "turn-notify",
      "turn-sound",
      "host-login",
      "keep-reachable",
    ]);
    expect(ids(visibleSettings("general", ctx({ remote: true, bridge: bridge({ hostLogin: login }) })))).toEqual([
      "send-key",
      "expand-tool-calls",
      "always-show-context",
      "turn-sound",
    ]);
    expect(ids(visibleSettings("appearance", ctx()))).toEqual([
      "theme",
      "soft-dark",
      "color-vision",
      "code-font",
      "code-ligatures",
      "code-theme",
      "terminal-font",
      "terminal-size",
      "markdown-style",
    ]);
    const zoom = { set: vi.fn() };
    expect(ids(visibleSettings("appearance", ctx({ bridge: bridge({ zoom }) })))).toEqual([
      "theme",
      "soft-dark",
      "color-vision",
      "zoom",
      "code-font",
      "code-ligatures",
      "code-theme",
      "terminal-font",
      "terminal-size",
      "markdown-style",
    ]);
  });

  it("offers the Auto-merge toggle in Git where the host can turn auto-merge on", () => {
    expect(ids(visibleSettings("git", ctx({ gitSettings: true })))).toEqual([]);
    expect(ids(visibleSettings("git", ctx({ gitSettings: true, autoMerge: true })))).toEqual(["auto-merge"]);
    expect(searchSettings("automerge", ctx({ gitSettings: true, autoMerge: true })).map((h) => h.target)).toContainEqual({
      tab: "git",
      settingId: "auto-merge",
    });
  });

  it("points at an update from About only while one is waiting", () => {
    const about = TABS.find((t) => t.id === "about")!;
    expect(about.hint?.(ctx())).toBeNull();
    expect(about.hint?.(ctx({ updateHint: true }))).toBe("Update available");
  });
});

describe("searching settings", () => {
  const targets = (q: string, c = ctx()) => searchSettings(q, c).map((h) => h.target);

  it("finds a setting by a keyword, naming its tab", () => {
    const [hit] = searchSettings("light", ctx());
    expect(hit?.target).toEqual({ tab: "appearance", settingId: "theme" });
    expect(hit?.tabLabel).toBe("Appearance");
  });

  it("finds a tab by its label or keywords", () => {
    expect(targets("about")[0]).toEqual({ tab: "about" });
    expect(targets("codex")).toContainEqual({ tab: "agents" });
  });

  it("marks what matched in the title", () => {
    const [hit] = searchSettings("them", ctx());
    const { container } = render(<span>{hit!.title}</span>);
    expect(container.querySelector("mark")?.textContent).toBe("Them");
  });

  it("finds only what this window offers", () => {
    expect(targets("devices")).toContainEqual({ tab: "devices" });
    expect(targets("devices", ctx({ remote: true, link: {} as SettingsContext["link"] }))).not.toContainEqual({
      tab: "devices",
    });
    expect(targets("startup")).toEqual([]);
  });

  it("finds nothing for an empty query", () => {
    expect(searchSettings("  ", ctx())).toEqual([]);
  });
});
