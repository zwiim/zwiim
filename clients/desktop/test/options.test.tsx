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

/** Opens the mode menu and returns its entry for `name`, if the agent offers it. */
function modeItem(name: string) {
  if (!screen.queryByRole("menu")) fireEvent.click(screen.getByRole("button", { name: /^Mode:/ }));
  return screen.queryByRole("menuitemradio", { name: new RegExp(`^${name}`) });
}

describe("options bar", () => {
  it("hides what the agent does not offer", () => {
    render(
      <OptionsBar
        options={{ ...opts("normal", ["normal", "plan"]), effort: null }}
        disabled={false}
        onChange={() => {}}
      />,
    );
    expect(screen.queryByLabelText("Effort")).toBeNull();
    expect(screen.getByLabelText("Model")).toBeTruthy();
    expect(modeItem("Auto")).toBeNull();
    expect(modeItem("Plan")).toBeTruthy();
  });

  it("switches to Auto directly, without a confirmation", () => {
    const onChange = vi.fn();
    render(<OptionsBar options={opts()} disabled={false} onChange={onChange} />);
    fireEvent.click(modeItem("Auto")!);
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
    expect(screen.getByRole("button", { name: "Mode: Auto" })).toBeTruthy();
    fireEvent.click(screen.getByText("Ask first again"));
    expect(onChange).toHaveBeenCalledWith({ mode: "normal" });
  });

  it("closes the mode menu with Escape without changing anything", () => {
    const onChange = vi.fn();
    render(<OptionsBar options={opts()} disabled={false} onChange={onChange} />);
    expect(modeItem("Ask")?.getAttribute("aria-checked")).toBe("true");
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("switches plan on and off directly", () => {
    const onChange = vi.fn();
    const { rerender } = render(<OptionsBar options={opts()} disabled={false} onChange={onChange} />);
    fireEvent.click(modeItem("Plan")!);
    expect(onChange).toHaveBeenLastCalledWith({ mode: "plan" });
    rerender(<OptionsBar options={opts("plan")} disabled={false} onChange={onChange} />);
    fireEvent.click(modeItem("Ask")!);
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
    expect((screen.getByLabelText("Model") as HTMLSelectElement).value).toBe("opus");
    fireEvent.click(modeItem("Plan")!);
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
      expect(screen.queryByLabelText("Model")).toBeNull();
      learned = true;
      await act(async () => vi.advanceTimersByTime(2000));
      await tick();
      expect((screen.getByLabelText("Model") as HTMLSelectElement).value).toBe("sonnet");
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
    expect((within(bar).getByLabelText("Model") as HTMLSelectElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /^Mode:/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(bar.textContent).toContain("between turns");
    await frame({ type: "event", event: { session_id: "s1", seq: 2, type: "agent.turn_complete", payload: { stop_reason: "end_turn" } } });
    await frame({ type: "event", event: { session_id: "s1", seq: 3, type: "agent.status", payload: { status: "connected" } } });
    const model = within(screen.getByTestId("options-bar")).getByLabelText("Model") as HTMLSelectElement;
    expect(model.disabled).toBe(false);
    fireEvent.change(model, { target: { value: "opus" } });
    await tick();
    expect(t.calls.find((c) => c.op === "session.set_options")!.params).toEqual({ session_id: "s1", model: "opus" });
    expect(model.value).toBe("opus");
    controller.dispose();
  });

  it("reports an unavailable option and keeps the agent's value", async () => {
    const t = new FakeTransport();
    t.handlers["session.set_options"] = () => {
      throw new ZwiimClientError("option_unavailable", "not offered", 422, { option: "model", value: "opus" });
    };
    const { controller, frame } = await open(t);
    await frame({ type: "snapshot", epoch: "e", event_seq: 1, snapshot: snapshot(opts()) });
    const model = screen.getByLabelText("Model") as HTMLSelectElement;
    fireEvent.change(model, { target: { value: "opus" } });
    await tick();
    expect(screen.getByTestId("options-error").textContent).toContain("does not offer opus");
    expect(model.value).toBe("sonnet");
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
