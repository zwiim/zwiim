import { createRef } from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentSummary, ChoiceOption, OptionValue, SessionMode, SessionOptions } from "@zwiim/sdk";
import { favoriteModels, recentModels, rememberModelEffort, toggleFavoriteModel } from "@/lib/prefs";
import { ModelPicker } from "@/panels/ModelPicker";

afterEach(cleanup);

const v = (value: string, label = value): OptionValue => ({ value, label, description: null });
const choice = (current: string, values: OptionValue[]): ChoiceOption => ({ current, values });

const CLAUDE_EFFORT = [v("low", "Low"), v("medium", "Medium"), v("high", "High"), v("xhigh", "Extra high"), v("max", "Max"), v("ultracode", "Ultracode")];
const SONNET_EFFORT = [v("low", "Low"), v("medium", "Medium"), v("high", "High"), v("max", "Max")];
const CODEX_EFFORT = [v("minimal", "Minimal"), v("low", "Low"), v("medium", "Medium"), v("high", "High")];

function agent(id: string, name: string, extra: Partial<AgentSummary> = {}): AgentSummary {
  return {
    agent: id,
    name,
    available: true,
    reason: null,
    support: "supported",
    adapter: null,
    options: null,
    effort_by_model: {},
    commands: null,
    ...extra,
  };
}

const claudeModels = choice("sonnet", [
  { value: "opus", label: "Opus", description: "Most capable" },
  { value: "sonnet", label: "Sonnet", description: "Everyday coding" },
  { value: "haiku", label: "Haiku", description: "Quick edits" },
]);

function claudeOptions(model = "sonnet", mode: SessionMode = "normal", available: SessionMode[] = ["normal", "plan", "unattended"]): SessionOptions {
  const effort = model === "opus" ? choice("high", CLAUDE_EFFORT) : model === "sonnet" ? choice("medium", SONNET_EFFORT) : null;
  return {
    model: { ...claudeModels, current: model },
    effort,
    mode: { current: mode, label: "", available },
  };
}

const AGENTS: AgentSummary[] = [
  agent("claude_code", "Claude Code", {
    options: claudeOptions(),
    effort_by_model: { opus: choice("high", CLAUDE_EFFORT), sonnet: choice("medium", SONNET_EFFORT), haiku: null },
  }),
  agent("codex", "Codex", {
    options: {
      model: choice("gpt-5.5", [v("gpt-5.5", "GPT-5.5"), v("gpt-5.5-mini", "GPT-5.5 mini")]),
      effort: choice("medium", CODEX_EFFORT),
      mode: { current: "normal", label: "", available: ["normal", "unattended"] },
    },
  }),
  agent("open_code", "OpenCode", {
    support: "experimental",
    options: {
      model: choice(
        "anthropic/claude-sonnet-5.5",
        ["anthropic/claude-sonnet-5.5", "openai/gpt-5.5", "google/gemini-3-pro", "deepseek/deepseek-v4", "qwen/qwen3-coder", "moonshot/kimi-k3", "x-ai/grok-5", "mistral/devstral-2", "zai/glm-5"].map((m) => v(m)),
      ),
      effort: null,
      mode: { current: "normal", label: "", available: ["normal", "plan"] },
    },
  }),
  agent("gemini", "Gemini CLI", { available: false, reason: "not installed" }),
];

interface Props {
  agents?: AgentSummary[];
  options?: SessionOptions;
  started?: boolean;
  disabled?: boolean;
  onChange?: (r: object) => void;
  onAgent?: (agent: string, r: object) => void;
}

function picker(p: Props = {}) {
  return (
    <ModelPicker
      agents={p.agents ?? AGENTS}
      agent="claude_code"
      options={p.options ?? claudeOptions()}
      started={p.started ?? false}
      disabled={p.disabled ?? false}
      disabledReason="Options change between turns"
      onChange={p.onChange ?? (() => {})}
      onAgent={p.onAgent}
    />
  );
}

const chip = () => screen.getByRole("button", { name: / mode, |^Claude Code,/ });
const dialog = () => screen.getByRole("dialog", { name: "Agent options" });
const search = () => within(dialog()).getByRole("combobox") as HTMLInputElement;
const open = () => {
  if (!screen.queryByRole("dialog", { name: "Agent options" })) fireEvent.click(chip());
  return dialog();
};
const group = (name: string) => within(screen.getByRole("listbox", { name: "Models" })).getByRole("group", { name });
/** The browsed agent's section is the last one in the list. */
const agentGroup = () => within(screen.getByRole("listbox", { name: "Models" })).getAllByRole("group").at(-1)!;
const option = (name: RegExp) => within(agentGroup()).getByRole("option", { name });
const effortRow = () => within(dialog()).getByRole("button", { name: /^Effort: / });
const modeRow = () => within(dialog()).getByRole("button", { name: /^Mode: / });
/** Opens the Mode or Effort side list and returns its entry for `item`. */
function side(row: "Mode" | "Effort", item: string) {
  open();
  fireEvent.click(row === "Mode" ? modeRow() : effortRow());
  return screen.queryByRole("menuitemradio", { name: new RegExp(`^${item}`) });
}

describe("model picker", () => {
  it("names the chip by mode, agent, model and effort", () => {
    render(picker());
    expect(chip().getAttribute("aria-label")).toBe("Ask mode, Claude Code, Sonnet, Medium effort");
    expect(chip().getAttribute("aria-haspopup")).toBe("dialog");
  });

  it("hides what the agent does not offer", () => {
    render(picker({ options: { ...claudeOptions("sonnet", "normal", ["normal", "plan"]), effort: null } }));
    expect(chip().getAttribute("aria-label")).toBe("Ask mode, Claude Code, Sonnet");
    open();
    expect(effortRow().getAttribute("aria-label")).toBe("Effort: not offered");
    expect(effortRow().getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(effortRow());
    expect(screen.queryByRole("menu", { name: "Effort" })).toBeNull();
    expect(side("Mode", "Auto")).toBeNull();
    expect(screen.getByRole("menuitemradio", { name: /^Plan/ })).toBeTruthy();
  });

  it("names an agent the list does not offer by its own name", () => {
    render(
      <ModelPicker
        agents={AGENTS.filter((a) => a.support === "supported")}
        agent="open_code"
        name="OpenCode"
        options={claudeOptions()}
        started
        disabled={false}
        onChange={() => {}}
      />,
    );
    expect(screen.getByRole("button", { name: /^Ask mode, OpenCode,/ })).toBeTruthy();
  });

  it("picks a model and sends only the model", () => {
    const onChange = vi.fn();
    render(picker({ onChange }));
    open();
    fireEvent.click(option(/^Opus/));
    expect(onChange).toHaveBeenCalledTimes(1);
    // A new session sends the effort the row shows too.
    expect(onChange).toHaveBeenCalledWith({ model: "opus", effort: "high" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(chip());
    expect(recentModels()[0]).toMatchObject({ agent: "claude_code", model: "opus" });
    // The current model is no change.
    open();
    fireEvent.click(option(/^Sonnet/));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the running effort on the model in effect, and remembered ones on the rest", () => {
    rememberModelEffort({ agent: "claude_code", model: "opus" }, "high");
    rememberModelEffort({ agent: "claude_code", model: "sonnet" }, "max");
    const options = { ...claudeOptions("opus"), effort: choice("medium", CLAUDE_EFFORT) };
    render(picker({ options, started: true }));
    open();
    expect(option(/^Opus/).textContent).toContain("Medium");
    expect(option(/^Opus/).textContent).not.toContain("High");
    expect(option(/^Sonnet/).textContent).toContain("Max");
  });

  it("sends only the model on a started session", () => {
    const onChange = vi.fn();
    render(picker({ onChange, started: true }));
    open();
    fireEvent.click(option(/^Opus/));
    expect(onChange).toHaveBeenCalledWith({ model: "opus" });
  });

  it("restores a model's remembered effort while the model offers it", () => {
    const onChange = vi.fn();
    rememberModelEffort({ agent: "claude_code", model: "opus" }, "max");
    render(picker({ onChange }));
    open();
    expect(option(/^Opus/).textContent).toContain("Max");
    fireEvent.click(option(/^Opus/));
    expect(onChange).toHaveBeenLastCalledWith({ model: "opus", effort: "max" });
    rememberModelEffort({ agent: "claude_code", model: "opus" }, "turbo");
    open();
    fireEvent.click(option(/^Opus/));
    expect(onChange).toHaveBeenLastCalledWith({ model: "opus", effort: "high" });
  });

  it("shows an unlisted current model", () => {
    render(picker({ options: { ...claudeOptions(), model: { ...claudeModels, current: "sonnet-old" } } }));
    expect(chip().getAttribute("aria-label")).toContain("sonnet-old");
    open();
    expect(within(group("Claude Code")).queryAllByRole("option", { selected: true })).toEqual([]);
  });

  it("closes when the options lock, and focus returns to the chip", () => {
    const onChange = vi.fn();
    const { rerender } = render(picker({ onChange }));
    side("Effort", "High");
    expect(screen.getByRole("menu", { name: "Effort" })).toBeTruthy();
    rerender(picker({ onChange, disabled: true }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(chip().getAttribute("aria-disabled")).toBe("true");
    expect(document.activeElement).toBe(chip());
    fireEvent.click(chip());
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("opens and closes from its handle, and not while the options are locked", () => {
    const handle = createRef<{ toggle(): void }>();
    const props = { agents: AGENTS, agent: "claude_code", options: claudeOptions(), started: false, onChange: () => {} };
    const { rerender } = render(<ModelPicker {...props} disabled={false} handle={handle} />);
    act(() => handle.current!.toggle());
    expect(dialog()).toBeTruthy();
    act(() => handle.current!.toggle());
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(chip());
    rerender(<ModelPicker {...props} disabled handle={handle} />);
    act(() => handle.current!.toggle());
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("Escape closes the side list first, then the panel, without changing anything", () => {
    const onChange = vi.fn();
    render(picker({ onChange }));
    expect(side("Effort", "Medium")?.getAttribute("aria-checked")).toBe("true");
    fireEvent.keyDown(screen.getByRole("menu", { name: "Effort" }), { key: "Escape" });
    expect(screen.queryByRole("menu", { name: "Effort" })).toBeNull();
    expect(dialog()).toBeTruthy();
    expect(document.activeElement).toBe(effortRow());
    fireEvent.keyDown(dialog(), { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(chip());
    expect(onChange).not.toHaveBeenCalled();
  });

  it("opens a side list with ArrowRight and closes it with ArrowLeft", () => {
    render(picker());
    open();
    const row = effortRow();
    row.focus();
    fireEvent.keyDown(row, { key: "ArrowRight" });
    const menu = screen.getByRole("menu", { name: "Effort" });
    expect(document.activeElement).toBe(within(menu).getByRole("menuitemradio", { name: /^Medium/ }));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(within(menu).getByRole("menuitemradio", { name: /^High/ }));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
    expect(screen.queryByRole("menu", { name: "Effort" })).toBeNull();
    expect(document.activeElement).toBe(row);
  });

  it("picks an effort and remembers it for the model", () => {
    const onChange = vi.fn();
    render(picker({ onChange }));
    fireEvent.click(side("Effort", "High")!);
    expect(onChange).toHaveBeenCalledWith({ effort: "high" });
    expect(screen.queryByRole("menu", { name: "Effort" })).toBeNull();
    // Once the session runs at High, the Sonnet row shows it; Opus keeps its default.
    cleanup();
    render(picker({ options: { ...claudeOptions(), effort: choice("high", SONNET_EFFORT) } }));
    open();
    expect(option(/^Sonnet/).textContent).toContain("High");
  });

  it("switches to Auto directly, without a confirmation", () => {
    const onChange = vi.fn();
    render(picker({ onChange }));
    fireEvent.click(side("Mode", "Auto")!);
    expect(onChange).toHaveBeenCalledWith({ mode: "unattended" });
    expect(screen.queryByRole("menu", { name: "Mode" })).toBeNull();
  });

  it("makes the Auto state obvious", () => {
    render(picker({ options: claudeOptions("sonnet", "unattended") }));
    expect(chip().getAttribute("aria-label")).toMatch(/^Auto mode, /);
  });

  it("switches plan on and off directly", () => {
    const onChange = vi.fn();
    const { rerender } = render(picker({ onChange }));
    fireEvent.click(side("Mode", "Plan")!);
    expect(onChange).toHaveBeenLastCalledWith({ mode: "plan" });
    rerender(picker({ onChange, options: claudeOptions("sonnet", "plan") }));
    fireEvent.click(side("Mode", "Ask")!);
    expect(onChange).toHaveBeenLastCalledWith({ mode: "normal" });
  });

  it("starts a new session's agent from another agent's model", () => {
    const onChange = vi.fn();
    const onAgent = vi.fn();
    render(picker({ onChange, onAgent }));
    open();
    fireEvent.click(within(dialog()).getByRole("tab", { name: /^Codex/ }));
    fireEvent.click(option(/^GPT-5\.5,/));
    expect(onAgent).toHaveBeenCalledWith("codex", { model: "gpt-5.5", effort: "medium" });
    expect(onChange).not.toHaveBeenCalled();
    expect(recentModels()[0]).toMatchObject({ agent: "codex", model: "gpt-5.5", effort: "medium" });
  });

  it("keeps a started session's agent: other agents' models are disabled", () => {
    const onAgent = vi.fn();
    render(picker({ started: true, onAgent }));
    open();
    fireEvent.click(within(dialog()).getByRole("tab", { name: /^Codex/ }));
    const row = option(/^GPT-5\.5,/);
    expect(row.getAttribute("aria-disabled")).toBe("true");
    expect(row.getAttribute("aria-description")).toBe("Start a new session to use Codex");
    fireEvent.click(row);
    expect(onAgent).not.toHaveBeenCalled();
  });

  it("stars models into the quick section and picks them with Control and a number", () => {
    const onChange = vi.fn();
    render(picker({ onChange }));
    open();
    expect(within(group("Recent & favorites")).getByText(/Star a model to keep it here/)).toBeTruthy();
    expect(option(/^Opus/).getAttribute("aria-description")).toBe("Press Command D to add to favorites");
    // The current model is highlighted on opening; ArrowUp moves to Opus.
    fireEvent.keyDown(search(), { key: "ArrowUp" });
    expect(search().getAttribute("aria-activedescendant")).toBe(option(/^Opus/).id);
    fireEvent.keyDown(search(), { key: "d", metaKey: true });
    expect(favoriteModels()).toEqual([{ agent: "claude_code", model: "opus" }]);
    expect(within(group("Recent & favorites")).queryByText(/Star a model/)).toBeNull();
    expect(within(group("Recent & favorites")).getByRole("option", { name: /^Opus/ })).toBeTruthy();
    expect(within(group("Claude Code")).getByRole("option", { name: /^Opus/ }).getAttribute("aria-description")).toBe(
      "Favorite. Press Command D to remove from favorites",
    );
    fireEvent.keyDown(search(), { key: "1", ctrlKey: true });
    expect(onChange).toHaveBeenCalledWith({ model: "opus", effort: "high" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keeps the highlight on the same model when a star reorders the quick rows", () => {
    toggleFavoriteModel({ agent: "claude_code", model: "opus" });
    toggleFavoriteModel({ agent: "claude_code", model: "haiku" });
    render(picker());
    open();
    const quick = within(group("Recent & favorites")).getAllByRole("option");
    expect(quick[0]!.getAttribute("aria-label")).toMatch(/^Opus/);
    fireEvent.pointerMove(quick[0]!);
    fireEvent.keyDown(search(), { key: "d", metaKey: true });
    expect(favoriteModels()).toEqual([{ agent: "claude_code", model: "haiku" }]);
    fireEvent.keyDown(search(), { key: "d", metaKey: true });
    expect(favoriteModels()).toHaveLength(2);
    expect(favoriteModels().some((f) => f.model === "opus")).toBe(true);
  });

  it("a click on a hovered row's star toggles the favorite and does not pick the model", () => {
    const onChange = vi.fn();
    render(picker({ onChange }));
    open();
    const row = option(/^Opus/);
    fireEvent.pointerMove(row);
    expect(option(/^Opus/)).toBe(row);
    fireEvent.click(within(row).getByTitle("Add to favorites (⌘D)"));
    expect(favoriteModels()).toEqual([{ agent: "claude_code", model: "opus" }]);
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Agent options" })).toBeTruthy();
  });

  it("closes on a pointer press outside the panel", () => {
    render(picker());
    open();
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("moves the search to the first agent with a match and counts each agent's matches", () => {
    render(picker());
    open();
    fireEvent.change(search(), { target: { value: "gpt" } });
    const codex = within(dialog()).getByRole("tab", { name: /^Codex/ });
    expect(codex.getAttribute("aria-selected")).toBe("true");
    expect(codex.textContent).toContain("2");
    expect(within(dialog()).getByRole("tab", { name: /^OpenCode/ }).textContent).toContain("1");
    expect(within(group("Codex")).getAllByRole("option")).toHaveLength(2);
    expect(screen.queryByRole("group", { name: "Recent & favorites" })).toBeNull();
  });

  it("walks the panel with Tab and switches agents from the strip or with Command ]", () => {
    render(picker());
    open();
    expect(document.activeElement).toBe(search());
    const claude = within(dialog()).getByRole("tab", { name: "Claude Code, in use" });
    fireEvent.keyDown(search(), { key: "Tab" });
    expect(document.activeElement).toBe(claude);
    fireEvent.keyDown(claude, { key: "Tab" });
    expect(document.activeElement).toBe(effortRow());
    fireEvent.keyDown(effortRow(), { key: "Tab" });
    expect(document.activeElement).toBe(modeRow());
    fireEvent.keyDown(modeRow(), { key: "Tab" });
    expect(document.activeElement).toBe(search());
    fireEvent.keyDown(search(), { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(modeRow());

    claude.focus();
    fireEvent.keyDown(claude, { key: "ArrowDown" });
    const codex = within(dialog()).getByRole("tab", { name: /^Codex/ });
    expect(codex.getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(codex);
    search().focus();
    fireEvent.keyDown(search(), { key: "]", metaKey: true });
    expect(within(dialog()).getByRole("tab", { name: /^OpenCode/ }).getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(search(), { key: "[", metaKey: true });
    expect(codex.getAttribute("aria-selected")).toBe("true");
  });

  it("shows an unavailable agent's reason and no models", () => {
    render(picker());
    open();
    const gemini = within(dialog()).getByRole("tab", { name: "Gemini CLI, not installed" });
    fireEvent.click(gemini);
    expect(gemini.getAttribute("aria-selected")).toBe("true");
    expect(within(screen.getByRole("listbox", { name: "Models" })).getByText(/not installed/)).toBeTruthy();
    expect(within(screen.getByRole("listbox", { name: "Models" })).queryAllByRole("option")).toEqual([]);
  });

  it("says an agent is still learning its models", () => {
    render(picker({ agents: [...AGENTS, agent("amp", "Amp")], onAgent: vi.fn() }));
    open();
    fireEvent.click(within(dialog()).getByRole("tab", { name: "Amp" }));
    expect(within(screen.getByRole("listbox", { name: "Models" })).getByText("Learning its models…")).toBeTruthy();
  });

  it("collapses a long model list behind Show all", () => {
    render(picker());
    open();
    fireEvent.click(within(dialog()).getByRole("tab", { name: /^OpenCode/ }));
    expect(within(group("OpenCode")).getAllByRole("option")).toHaveLength(6);
    fireEvent.click(option(/^Show all 9 models/));
    expect(within(group("OpenCode")).getAllByRole("option")).toHaveLength(9);
  });

  it("lists exactly the efforts the model offers", () => {
    const { rerender } = render(picker({ options: claudeOptions("opus") }));
    side("Effort", "High");
    expect(within(screen.getByRole("menu", { name: "Effort" })).getAllByRole("menuitemradio")).toHaveLength(6);
    fireEvent.keyDown(screen.getByRole("menu", { name: "Effort" }), { key: "Escape" });
    rerender(picker({ options: claudeOptions("haiku") }));
    expect(effortRow().getAttribute("aria-label")).toBe("Effort: not offered");
    expect(effortRow().getAttribute("aria-disabled")).toBe("true");
  });
});
