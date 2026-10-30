import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  HostClient,
  HostController,
  ZwiimClientError,
  type SessionMode,
  type SessionOptions,
  type SessionSnapshot,
} from "@zwiim/sdk";
import { HostProvider } from "@/lib/host-context";
import { rememberAgentChoices } from "@/lib/prefs";
import { OptionsBar, UnattendedBanner } from "@/panels/OptionsBar";
import { newSessionOptions, SessionPanel } from "@/panels/SessionPanel";
import { FakeTransport, tick } from "./fakes";

afterEach(cleanup);

const opts = (
  mode: SessionMode = "normal",
  available: SessionMode[] = ["normal", "plan", "unattended"],
  model = "sonnet",
): SessionOptions => ({
  model: {
    current: model,
    values: [
      { value: "sonnet", label: "Sonnet", description: null },
      { value: "opus", label: "Opus", description: null },
    ],
  },
  effort: { current: "low", values: [{ value: "low", label: "Low", description: null }, { value: "high", label: "High", description: null }] },
  mode: { current: mode, label: mode === "normal" ? "Manual" : "", available },
});

const snapshot = (options: SessionOptions, status = "connected"): SessionSnapshot => ({
  status: status as SessionSnapshot["status"],
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options,
  commands: [],
});

/** Opens the options menu, then `row`'s submenu, and returns its entry for `item`. */
function pick(row: "Mode" | "Model" | "Effort", item: string) {
  if (!screen.queryByRole("menu", { name: "Session options" })) fireEvent.click(screen.getByRole("button", { name: /^Session options:/ }));
  fireEvent.click(screen.getByRole("menuitem", { name: new RegExp(`^${row} `) }));
  return screen.queryByRole("menuitemradio", { name: new RegExp(`^${item}$`) });
}
const chip = () => screen.getByRole("button", { name: /^Session options:/ }) as HTMLButtonElement;

const models = (n: number): SessionOptions["model"] => ({
  current: "sonnet",
  values: [
    { value: "sonnet", label: "Sonnet", description: null },
    { value: "opus", label: "Opus", description: "Most capable" },
    { value: "opus-1m", label: "Opus 1M", description: null },
    ...Array.from({ length: n - 3 }, (_, i) => ({ value: `m${i}`, label: `Model ${i}`, description: null })),
  ],
});

describe("options bar", () => {
  it("hides what the agent does not offer", () => {
    render(
      <OptionsBar
        options={{ ...opts("normal", ["normal", "plan"]), effort: null }}
        disabled={false}
        onChange={() => {}}
      />,
    );
    expect(chip().getAttribute("aria-label")).toBe("Session options: Ask · Sonnet");
    fireEvent.click(chip());
    expect(screen.queryByRole("menuitem", { name: /^Effort/ })).toBeNull();
    expect(screen.getByRole("menuitem", { name: /^Model/ })).toBeTruthy();
    expect(pick("Mode", "Auto")).toBeNull();
    expect(screen.getByRole("menuitemradio", { name: /^Plan/ })).toBeTruthy();
  });

  it("opens one menu with mode, model and effort", () => {
    render(<OptionsBar options={opts()} disabled={false} onChange={() => {}} />);
    expect(chip().getAttribute("aria-label")).toBe("Session options: Ask · Sonnet · Low");
    fireEvent.click(chip());
    const menu = screen.getByRole("menu", { name: "Session options" });
    expect(within(menu).getAllByRole("menuitem").map((r) => r.getAttribute("aria-label"))).toEqual([
      "Mode Ask",
      "Model Sonnet",
      "Effort Low",
    ]);
  });

  // The chip sits at the composer's right edge, beside Send: the menu and its
  // submenus grow leftward so they stay inside the window.
  it("opens the menu and its submenus leftward from the chip", () => {
    render(<OptionsBar options={opts()} disabled={false} onChange={() => {}} />);
    pick("Model", "Opus");
    const menu = screen.getByRole("menu", { name: "Session options" });
    expect(menu.className).toMatch(/\bright-0\b/);
    expect(menu.className).not.toMatch(/\bleft-0\b/);
    const sub = screen.getByRole("menu", { name: "Model" });
    expect(sub.className).toMatch(/\bright-full\b/);
    expect(sub.className).not.toMatch(/\bleft-full\b/);
  });

  it("picks a model from its submenu and sends only the model", () => {
    const onChange = vi.fn();
    render(<OptionsBar options={opts()} disabled={false} onChange={onChange} />);
    fireEvent.click(pick("Model", "Opus")!);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith({ model: "opus" });
    expect(screen.queryByRole("menu")).toBeNull();
    // The current model is no change.
    fireEvent.click(pick("Model", "Sonnet")!);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("searches models when the agent offers many", () => {
    const { rerender } = render(<OptionsBar options={{ ...opts(), model: models(12) }} disabled={false} onChange={() => {}} />);
    pick("Model", "Opus");
    const search = screen.getByRole("textbox", { name: "Search models" });
    expect(document.activeElement).toBe(search);
    fireEvent.change(search, { target: { value: "OPUS" } });
    const sub = screen.getByRole("menu", { name: "Model" });
    expect(within(sub).getAllByRole("menuitemradio").map((i) => i.getAttribute("aria-label"))).toEqual(["Opus", "Opus 1M"]);
    // Descriptions match too.
    fireEvent.change(search, { target: { value: "capable" } });
    expect(within(sub).getAllByRole("menuitemradio").map((i) => i.getAttribute("aria-label"))).toEqual(["Opus"]);
    fireEvent.change(search, { target: { value: "xyz" } });
    expect(within(sub).queryAllByRole("menuitemradio")).toEqual([]);
    expect(screen.getByText('No models match "xyz"')).toBeTruthy();
    rerender(<OptionsBar options={opts()} disabled={false} onChange={() => {}} />);
    fireEvent.keyDown(screen.getByRole("menu", { name: "Model" }), { key: "Escape" });
    pick("Model", "Opus");
    expect(screen.queryByRole("textbox", { name: "Search models" })).toBeNull();
  });

  it("starts each model search afresh", () => {
    render(<OptionsBar options={{ ...opts(), model: models(12) }} disabled={false} onChange={() => {}} />);
    pick("Model", "Opus");
    fireEvent.change(screen.getByRole("textbox", { name: "Search models" }), { target: { value: "xyz" } });
    fireEvent.keyDown(screen.getByRole("menu", { name: "Model" }), { key: "Escape" });
    pick("Model", "Opus");
    expect((screen.getByRole("textbox", { name: "Search models" }) as HTMLInputElement).value).toBe("");
  });

  it("shows an unlisted current model", () => {
    const options = opts();
    render(<OptionsBar options={{ ...options, model: { ...options.model!, current: "opus-old" } }} disabled={false} onChange={() => {}} />);
    expect(chip().getAttribute("aria-label")).toContain("opus-old");
    pick("Model", "Opus");
    const sub = screen.getByRole("menu", { name: "Model" });
    expect(within(sub).queryAllByRole("menuitemradio", { checked: true })).toEqual([]);
  });

  it("a single model does not open", () => {
    const options = opts();
    render(
      <OptionsBar
        options={{ ...options, model: { current: "sonnet", values: [options.model!.values[0]] } }}
        disabled={false}
        onChange={() => {}}
      />,
    );
    fireEvent.click(chip());
    const row = screen.getByRole("menuitem", { name: /^Model/ });
    expect(row.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(row);
    expect(screen.queryByRole("menu", { name: "Model" })).toBeNull();
  });

  it("closes when the options lock", () => {
    const { rerender } = render(<OptionsBar options={opts()} disabled={false} onChange={() => {}} />);
    pick("Model", "Opus");
    expect(screen.getByRole("menu", { name: "Model" })).toBeTruthy();
    rerender(<OptionsBar options={opts()} disabled onChange={() => {}} />);
    expect(screen.queryByRole("menu")).toBeNull();
    expect(chip().disabled).toBe(true);
  });

  it("Escape closes the submenu first, then the menu, without changing anything", () => {
    const onChange = vi.fn();
    render(<OptionsBar options={opts()} disabled={false} onChange={onChange} />);
    expect(pick("Model", "Sonnet")?.getAttribute("aria-checked")).toBe("true");
    fireEvent.keyDown(screen.getByRole("menu", { name: "Model" }), { key: "Escape" });
    expect(screen.queryByRole("menu", { name: "Model" })).toBeNull();
    expect(screen.getByRole("menu", { name: "Session options" })).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: /^Model/ }));
    fireEvent.keyDown(screen.getByRole("menu", { name: "Session options" }), { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(chip());
    expect(onChange).not.toHaveBeenCalled();
  });

  it("opens a submenu with ArrowRight and closes it with ArrowLeft", () => {
    render(<OptionsBar options={opts()} disabled={false} onChange={() => {}} />);
    fireEvent.click(chip());
    const row = screen.getByRole("menuitem", { name: /^Effort/ });
    fireEvent.keyDown(row, { key: "ArrowRight" });
    const sub = screen.getByRole("menu", { name: "Effort" });
    expect(document.activeElement).toBe(within(sub).getByRole("menuitemradio", { name: "Low" }));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(within(sub).getByRole("menuitemradio", { name: "High" }));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
    expect(screen.queryByRole("menu", { name: "Effort" })).toBeNull();
    expect(document.activeElement).toBe(row);
  });

  it("switches to Auto directly, without a confirmation", () => {
    const onChange = vi.fn();
    render(<OptionsBar options={opts()} disabled={false} onChange={onChange} />);
    fireEvent.click(pick("Mode", "Auto")!);
    expect(onChange).toHaveBeenCalledWith({ mode: "unattended" });
    // The menu closes (it plays its exit hidden from assistive tech).
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("makes the Auto state obvious and easy to leave", () => {
    const onChange = vi.fn();
    render(
      <>
        <UnattendedBanner options={opts("unattended")} disabled={false} onChange={onChange} />
        <OptionsBar options={opts("unattended")} disabled={false} onChange={onChange} />
      </>,
    );
    expect(screen.getByTestId("unattended-banner").textContent).toContain("without asking");
    expect(chip().getAttribute("aria-label")).toContain("Auto");
    fireEvent.click(screen.getByText("Ask first again"));
    expect(onChange).toHaveBeenCalledWith({ mode: "normal" });
  });

  it("switches plan on and off directly", () => {
    const onChange = vi.fn();
    const { rerender } = render(<OptionsBar options={opts()} disabled={false} onChange={onChange} />);
    fireEvent.click(pick("Mode", "Plan")!);
    expect(onChange).toHaveBeenLastCalledWith({ mode: "plan" });
    rerender(<OptionsBar options={opts("plan")} disabled={false} onChange={onChange} />);
    fireEvent.click(pick("Mode", "Ask")!);
    expect(onChange).toHaveBeenLastCalledWith({ mode: "normal" });
  });
});

describe("new session options", () => {
  it("starts from remembered model/effort when still offered, and Auto mode unless picked", () => {
    const a = newSessionOptions(opts(), { model: "opus", effort: "gone" }, {});
    expect(a.request).toEqual({ model: "opus", effort: "low", mode: "unattended" });
    expect(a.shown.mode?.current).toBe("unattended");
    // Without Auto on offer, the agent's own mode stays.
    const ask = newSessionOptions(opts("normal", ["normal", "plan"]), { model: null, effort: null }, {});
    expect(ask.request.mode).toBeUndefined();
    expect(ask.shown.mode?.current).toBe("normal");
    const b = newSessionOptions(opts(), { model: null, effort: null }, { mode: "plan", effort: "high" });
    expect(b.request).toEqual({ model: "sonnet", effort: "high", mode: "plan" });
    expect(b.shown.mode?.current).toBe("plan");
    // A mode the agent does not offer is never requested.
    expect(newSessionOptions(opts("normal", ["normal"]), { model: null, effort: null }, { mode: "unattended" }).request.mode).toBeUndefined();
  });

  it("offers the chosen model's own effort, and none on a model without one", () => {
    const high = { current: "high", values: [{ value: "high", label: "High", description: null }] };
    const effortOn = (model: string | null) => (model === "opus" ? null : high);
    const opus = newSessionOptions(opts(), { model: "opus", effort: "low" }, {}, effortOn);
    expect(opus.shown.effort).toBeNull();
    expect(opus.request).toEqual({ model: "opus", mode: "unattended" });
    const sonnet = newSessionOptions(opts(), { model: "opus", effort: "low" }, { model: "sonnet" }, effortOn);
    expect(sonnet.request).toEqual({ model: "sonnet", effort: "high", mode: "unattended" });
  });

  it("prefers picks, then the Host's defaults, then what this client remembered", () => {
    const remembered = { model: "sonnet", effort: "high" };
    const host = { model: "opus", effort: "low", mode: "plan" as const };
    expect(newSessionOptions(opts(), remembered, {}, undefined, host).request).toEqual({
      model: "opus",
      effort: "low",
      mode: "plan",
    });
    expect(newSessionOptions(opts(), remembered, { model: "sonnet", mode: "unattended" }, undefined, host).request).toEqual({
      model: "sonnet",
      effort: "low",
      mode: "unattended",
    });
    // A Host default the agent no longer offers falls through to the remembered choice.
    const gone = { model: "haiku", effort: null, mode: null };
    expect(newSessionOptions(opts(), { model: "opus", effort: null }, {}, undefined, gone).request.model).toBe("opus");
    // No Host mode: Auto where offered, as before.
    expect(newSessionOptions(opts(), remembered, {}, undefined, gone).request.mode).toBe("unattended");
  });

  it("sends the chosen options with the first message", async () => {
    rememberAgentChoices("claude_code", { model: "opus" });
    const t = new FakeTransport();
    t.handlers["agent.list"] = () => [
      { agent: "claude_code", name: "Claude Code", available: true, reason: null, support: "supported", options: opts() },
    ];
    t.handlers["session.start"] = () => ({ session_id: "s1" });
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <SessionPanel workspaceId="w1" sessionId={null} onSessionStarted={() => {}} />
      </HostProvider>,
    );
    await tick();
    expect(chip().getAttribute("aria-label")).toContain("Opus");
    fireEvent.click(pick("Mode", "Plan")!);
    const box = screen.getByPlaceholderText(/^Describe the task/);
    fireEvent.change(box, { target: { value: "go" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    expect(t.calls.find((c) => c.op === "session.start")!.params.options).toEqual({ model: "opus", effort: "low", mode: "plan" });
    controller.dispose();
  });

  it("shows an agent's options once the host has learned them", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      let learned = false;
      const t = new FakeTransport();
      t.handlers["agent.list"] = () => [
        {
          agent: "codex",
          name: "Codex",
          available: true,
          reason: null,
          support: "supported",
          options: learned ? opts() : null,
        },
      ];
      const client = new HostClient({ transport: t });
      const controller = new HostController(client);
      client.start();
      await tick();
      render(
        <HostProvider value={controller}>
          <SessionPanel workspaceId="w1" sessionId={null} onSessionStarted={() => {}} />
        </HostProvider>,
      );
      await tick();
      expect(screen.queryByRole("button", { name: /^Session options:/ })).toBeNull();
      learned = true;
      await act(async () => vi.advanceTimersByTime(2000));
      await tick();
      expect(chip().getAttribute("aria-label")).toContain("Sonnet");
      const polls = t.calls.filter((c) => c.op === "agent.list").length;
      await act(async () => vi.advanceTimersByTime(10_000));
      expect(t.calls.filter((c) => c.op === "agent.list").length).toBe(polls);
      controller.dispose();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("open session options", () => {
  async function open(t: FakeTransport) {
    const client = new HostClient({ transport: t });
    const controller = new HostController(client);
    client.start();
    await tick();
    render(
      <HostProvider value={controller}>
        <SessionPanel workspaceId="w1" sessionId="s1" onSessionStarted={() => {}} />
      </HostProvider>,
    );
    await tick();
    const frame = (f: object) => act(async () => t.frame({ subscription_id: "sub-1", session_id: "s1", ...f } as never));
    return { controller, frame };
  }

  it("changes options between turns only", async () => {
    const t = new FakeTransport();
    t.handlers["session.set_options"] = (p) => ({ session_id: "s1", options: opts("normal", undefined, p.model) });
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot(opts(), "prompting") });
    const bar = screen.getByTestId("options-bar");
    // The agent's options live in the composer, next to send/stop.
    expect(screen.getByTestId("composer").contains(bar)).toBe(true);
    expect(within(screen.getByTestId("composer")).getByLabelText("Stop agent")).toBeTruthy();
    expect(within(bar).getByRole("button", { name: /^Session options:/ }).hasAttribute("disabled")).toBe(true);
    expect(bar.textContent).toContain("between turns");
    await frame({ type: "event", event: { session_id: "s1", seq: 2, type: "agent.turn_complete", payload: { stop_reason: "end_turn" } } });
    await frame({ type: "event", event: { session_id: "s1", seq: 3, type: "agent.status", payload: { status: "connected" } } });
    expect(chip().disabled).toBe(false);
    fireEvent.click(pick("Model", "Opus")!);
    await tick();
    expect(t.calls.find((c) => c.op === "session.set_options")!.params).toEqual({ session_id: "s1", model: "opus" });
    expect(chip().getAttribute("aria-label")).toContain("Opus");
    controller.dispose();
  });

  it("reports an unavailable option and keeps the agent's value", async () => {
    const t = new FakeTransport();
    t.handlers["session.set_options"] = () => {
      throw new ZwiimClientError("option_unavailable", "not offered", 422, { option: "model", value: "opus" });
    };
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot(opts()) });
    fireEvent.click(pick("Model", "Opus")!);
    await tick();
    expect(screen.getByTestId("options-error").textContent).toContain("does not offer opus");
    expect(chip().getAttribute("aria-label")).toContain("Sonnet");
    controller.dispose();
  });

  it("shows no options while the agent has not reported any", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: { ...snapshot(opts()), options: null } });
    expect(screen.queryByTestId("options-bar")).toBeNull();
    controller.dispose();
  });
});
