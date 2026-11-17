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
import { UnattendedBanner } from "@/panels/OptionsBar";
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

const codexOptions = (): SessionOptions => ({
  model: {
    current: "gpt-5.5",
    values: [
      { value: "gpt-5.5", label: "GPT-5.5", description: null },
      { value: "gpt-5.5-mini", label: "GPT-5.5 mini", description: null },
    ],
  },
  effort: { current: "medium", values: [{ value: "medium", label: "Medium", description: null }, { value: "high", label: "High", description: null }] },
  mode: { current: "normal", label: "", available: ["normal", "unattended"] },
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

const chip = () => screen.getByRole("button", { name: /^(Ask|Auto|Plan) mode, / }) as HTMLButtonElement;
const composerBox = () => screen.getByTestId("composer").querySelector("textarea")!;
const dialog = () => screen.getByRole("dialog", { name: "Agent options" });

/** Opens the picker and returns the side-list entry for `item` under `row`. */
function pick(row: "Mode" | "Effort", item: string) {
  if (!screen.queryByRole("dialog", { name: "Agent options" })) fireEvent.click(chip());
  fireEvent.click(within(dialog()).getByRole("button", { name: new RegExp(`^${row}: `) }));
  return screen.queryByRole("menuitemradio", { name: new RegExp(`^${item}$`) });
}

/** Opens the picker and returns the model row whose name starts with `name`. */
function model(name: string) {
  if (!screen.queryByRole("dialog", { name: "Agent options" })) fireEvent.click(chip());
  return within(screen.getByRole("listbox", { name: "Models" })).getAllByRole("option", { name: new RegExp(`^${name}`) }).at(-1)!;
}

describe("unattended banner", () => {
  it("makes the Auto state obvious and easy to leave", () => {
    const onChange = vi.fn();
    render(<UnattendedBanner options={opts("unattended")} disabled={false} onChange={onChange} />);
    expect(screen.getByTestId("unattended-banner").textContent).toContain("without asking");
    fireEvent.click(screen.getByText("Ask first again"));
    expect(onChange).toHaveBeenCalledWith({ mode: "normal" });
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

  it("starts fast mode as picked, else as last chosen on the model, and sends none where the model offers none", () => {
    const fastOn = (model: string | null) => (model === "opus" ? null : false);
    const sonnet = newSessionOptions(opts(), { model: null, effort: null }, {}, undefined, undefined, fastOn);
    expect(sonnet.shown.fast).toBe(false);
    expect(sonnet.request.fast).toBe(false);
    expect(newSessionOptions(opts(), { model: null, effort: null }, { fast: true }, undefined, undefined, fastOn).request.fast).toBe(true);
    const opus = newSessionOptions(opts(), { model: "opus", effort: null }, { fast: true }, undefined, undefined, fastOn);
    expect(opus.shown.fast).toBeNull();
    expect("fast" in opus.request).toBe(false);
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
    expect(chip().getAttribute("aria-label")).toBe("Plan mode, Claude Code, Opus, Low effort");
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
      expect(screen.queryByRole("button", { name: /mode, / })).toBeNull();
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

  const codex = (options: SessionOptions | null = codexOptions()) => ({
    agent: "codex",
    name: "Codex",
    available: true,
    reason: null,
    support: "supported",
    options,
  });
  const claude = () => ({ agent: "claude_code", name: "Claude Code", available: true, reason: null, support: "supported", options: opts() });

  async function newSession(agents: object[]) {
    const t = new FakeTransport();
    t.handlers["agent.list"] = () => agents;
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
    return { t, controller };
  }

  async function send(t: FakeTransport) {
    const box = screen.getByPlaceholderText(/^Describe the task/);
    fireEvent.change(box, { target: { value: "go" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await tick();
    return t.calls.find((c) => c.op === "session.start")!.params;
  }

  it("starts another agent with the model and effort picked in the picker", async () => {
    // Stale per-agent choices must not win over the pick.
    rememberAgentChoices("codex", { model: "gpt-5.5-mini", effort: "low" });
    const { t, controller } = await newSession([claude(), codex()]);
    fireEvent.click(chip());
    fireEvent.click(within(dialog()).getByRole("tab", { name: /^Codex/ }));
    fireEvent.click(model("GPT-5\\.5,"));
    expect(chip().getAttribute("aria-label")).toBe("Auto mode, Codex, GPT-5.5, Medium effort");
    const params = await send(t);
    expect(params.agent).toBe("codex");
    expect(params.options).toEqual({ model: "gpt-5.5", effort: "medium", mode: "unattended" });
    controller.dispose();
  });

  it("starts a model with the effort its row shows, not an earlier effort pick", async () => {
    const efforts = (current: string) => ({
      current,
      values: ["low", "medium", "high", "max"].map((value) => ({ value, label: value[0].toUpperCase() + value.slice(1), description: null })),
    });
    const agent = {
      ...claude(),
      options: { ...opts(), effort: efforts("medium") },
      effort_by_model: { sonnet: efforts("medium"), opus: efforts("high") },
    };
    const { t, controller } = await newSession([agent]);
    fireEvent.click(pick("Effort", "Max")!);
    expect(chip().getAttribute("aria-label")).toContain("Max effort");
    fireEvent.click(model("Opus"));
    expect(chip().getAttribute("aria-label")).toBe("Auto mode, Claude Code, Opus, High effort");
    const params = await send(t);
    expect(params.options).toEqual({ model: "opus", effort: "high", mode: "unattended" });
    controller.dispose();
  });

  it("starts an agent the host has not learned yet with the host's defaults", async () => {
    const { t, controller } = await newSession([claude(), codex(null)]);
    fireEvent.click(chip());
    fireEvent.click(within(dialog()).getByRole("tab", { name: /^Codex/ }));
    expect(within(screen.getByRole("listbox", { name: "Models" })).getByText("Learning its models…")).toBeTruthy();
    fireEvent.click(within(screen.getByRole("listbox", { name: "Models" })).getByRole("option"));
    const params = await send(t);
    expect(params.agent).toBe("codex");
    expect(params.options).toBeUndefined();
    controller.dispose();
  });

  it("never selects an unavailable agent", async () => {
    const gone = { ...codex(null), available: false, reason: "not_installed" };
    const { t, controller } = await newSession([claude(), gone]);
    fireEvent.click(chip());
    fireEvent.click(within(dialog()).getByRole("tab", { name: /^Codex/ }));
    const list = within(screen.getByRole("listbox", { name: "Models" }));
    expect(list.getByText(/not installed/)).toBeTruthy();
    expect(list.queryAllByRole("option")).toEqual([]);
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Search models" }), { key: "Enter" });
    fireEvent.keyDown(dialog(), { key: "Escape" });
    expect((await send(t)).agent).toBe("claude_code");
    controller.dispose();
  });

  it("opens the picker from the composer with Command .", async () => {
    const { controller } = await newSession([claude()]);
    const box = screen.getByPlaceholderText(/^Describe the task/);
    fireEvent.keyDown(box, { key: ".", ctrlKey: true });
    expect(dialog()).toBeTruthy();
    fireEvent.keyDown(within(dialog()).getByRole("combobox", { name: "Search models" }), { key: ".", ctrlKey: true });
    expect(screen.queryByRole("dialog", { name: "Agent options" })).toBeNull();
    expect(document.activeElement).toBe(chip());
    controller.dispose();
  });

  it("cycles effort and mode from the composer in a new session", async () => {
    const { controller } = await newSession([claude()]);
    const box = screen.getByPlaceholderText(/^Describe the task/);
    fireEvent.keyDown(box, { key: "?", code: "Slash", ctrlKey: true, shiftKey: true });
    fireEvent.keyDown(box, { key: "Tab", shiftKey: true });
    expect(chip().getAttribute("aria-label")).toBe("Ask mode, Claude Code, Sonnet, High effort");
    controller.dispose();
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
    // The agent's options live in the composer, next to send/stop.
    expect(screen.getByTestId("composer").contains(chip())).toBe(true);
    expect(within(screen.getByTestId("composer")).getByLabelText("Stop agent")).toBeTruthy();
    expect(chip().getAttribute("aria-disabled")).toBe("true");
    expect(screen.getByTestId("composer").textContent).toContain("between turns");
    fireEvent.click(chip());
    expect(screen.queryByRole("dialog", { name: "Agent options" })).toBeNull();
    await frame({ type: "event", event: { session_id: "s1", seq: 2, type: "agent.turn_complete", payload: { stop_reason: "end_turn" } } });
    await frame({ type: "event", event: { session_id: "s1", seq: 3, type: "agent.status", payload: { status: "connected" } } });
    expect(chip().getAttribute("aria-disabled")).toBeNull();
    fireEvent.click(model("Opus"));
    await tick();
    expect(t.calls.find((c) => c.op === "session.set_options")!.params).toEqual({ session_id: "s1", model: "opus" });
    expect(chip().getAttribute("aria-label")).toContain("Opus");
    controller.dispose();
  });

  it("opens the picker with Command . and cycles effort and mode with Command Shift / and Shift Tab", async () => {
    const t = new FakeTransport();
    t.handlers["session.set_options"] = (p) => ({
      session_id: "s1",
      options: { ...opts(p.mode ?? "normal"), effort: { ...opts().effort!, current: p.effort ?? "low" } },
    });
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot(opts()) });
    const box = composerBox();
    fireEvent.keyDown(box, { key: ".", ctrlKey: true });
    expect(dialog()).toBeTruthy();
    fireEvent.keyDown(dialog(), { key: "Escape" });
    fireEvent.keyDown(box, { key: "?", code: "Slash", ctrlKey: true, shiftKey: true });
    await tick();
    expect(t.calls.filter((c) => c.op === "session.set_options").at(-1)!.params).toEqual({ session_id: "s1", effort: "high" });
    fireEvent.keyDown(box, { key: "Tab", shiftKey: true });
    await tick();
    expect(t.calls.filter((c) => c.op === "session.set_options").at(-1)!.params).toEqual({ session_id: "s1", mode: "plan" });
    controller.dispose();
  });

  it("refuses the cycling keys mid-turn", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot(opts(), "prompting") });
    const box = composerBox();
    fireEvent.keyDown(box, { key: "?", code: "Slash", ctrlKey: true, shiftKey: true });
    expect(screen.getByTestId("options-error").textContent).toBe("Options change between turns");
    // The message goes when the turn ends.
    await frame({ type: "event", event: { session_id: "s1", seq: 2, type: "agent.turn_complete", payload: { stop_reason: "end_turn" } } });
    await frame({ type: "event", event: { session_id: "s1", seq: 3, type: "agent.status", payload: { status: "connected" } } });
    expect(screen.queryByTestId("options-error")).toBeNull();
    await frame({ type: "snapshot", epoch: "e", event_seq: 4, snapshot: snapshot(opts(), "prompting") });
    fireEvent.keyDown(box, { key: "Tab", shiftKey: true });
    fireEvent.keyDown(box, { key: ".", ctrlKey: true });
    expect(screen.queryByRole("dialog", { name: "Agent options" })).toBeNull();
    expect(t.calls.some((c) => c.op === "session.set_options")).toBe(false);
    controller.dispose();
  });

  it("closes the open picker when the host locks options, and sends nothing", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot(opts()) });
    fireEvent.click(chip());
    expect(dialog()).toBeTruthy();
    await frame({ type: "snapshot", epoch: "e", event_seq: 2, snapshot: snapshot(opts(), "prompting") });
    expect(screen.queryByRole("dialog", { name: "Agent options" })).toBeNull();
    expect(document.activeElement).toBe(chip());
    expect(t.calls.some((c) => c.op === "session.set_options")).toBe(false);
    controller.dispose();
  });

  it("reports an unavailable option and keeps the agent's value", async () => {
    const t = new FakeTransport();
    t.handlers["session.set_options"] = () => {
      throw new ZwiimClientError("option_unavailable", "not offered", 422, { option: "model", value: "opus" });
    };
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot(opts()) });
    fireEvent.click(model("Opus"));
    await tick();
    expect(screen.getByTestId("options-error").textContent).toContain("does not offer opus");
    expect(chip().getAttribute("aria-label")).toContain("Sonnet");
    controller.dispose();
  });

  it("shows no options while the agent has not reported any", async () => {
    const t = new FakeTransport();
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: { ...snapshot(opts()), options: null } });
    expect(screen.queryByRole("button", { name: /mode, / })).toBeNull();
    controller.dispose();
  });
});
