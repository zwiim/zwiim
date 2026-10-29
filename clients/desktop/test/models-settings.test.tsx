import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController } from "@zwiim/sdk";
import { Workbench } from "@/App";
import type { NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { rememberWelcomed } from "@/lib/prefs";
import { FakeTransport, hostHello, tick } from "./fakes";
import { openMenu, openSettings } from "./ui";

afterEach(cleanup);
beforeEach(() => {
  window.localStorage.clear();
  rememberWelcomed();
});

function bridge(): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: true, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u", email: null })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
  };
}

const choice = (current: string, values: string[]) => ({
  current,
  values: values.map((v) => ({ value: v, label: v.toUpperCase(), description: null })),
});

const claude = {
  agent: "claude_code",
  name: "Claude Code",
  available: true,
  reason: null,
  support: "supported",
  adapter: null,
  options: { model: choice("opus", ["opus", "haiku"]), effort: choice("medium", ["low", "medium"]), mode: null },
  effort_by_model: { opus: choice("medium", ["low", "medium"]), haiku: null },
  commands: null,
};
const codex = { ...claude, agent: "codex", name: "Codex", available: false, reason: "not_installed", options: null, effort_by_model: {} };

/** A Host keeping `values`; `settings.update` merges into them as the Host would. */
function host(values: Record<string, string | null> = {}) {
  const t = new FakeTransport();
  const kept = { ...values };
  t.handlers["agent.list"] = () => [claude, codex];
  t.handlers["settings.get"] = () => ({ values: { ...kept } });
  t.handlers["settings.update"] = (p: { values: Record<string, string | null> }) => {
    Object.assign(kept, p.values);
    return { values: { ...kept } };
  };
  return t;
}

async function mount(t: FakeTransport) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <Workbench bridge={bridge()} />
    </HostProvider>,
  );
  await tick();
  return controller;
}

const updates = (t: FakeTransport) => t.calls.filter((c) => c.op === "settings.update").map((c) => c.params.values);

describe("models settings", () => {
  it("shows the host's defaults and saves a change there", async () => {
    const t = host({ "defaults.claude_code.model": "opus", "defaults.claude_code.effort": "low" });
    const c = await mount(t);
    const dialog = openSettings("Models");
    await tick();
    expect((dialog.getByLabelText("Claude Code model") as HTMLSelectElement).value).toBe("opus");
    expect((dialog.getByLabelText("Claude Code effort") as HTMLSelectElement).value).toBe("low");
    // Codex cannot run on this host: nothing to choose from.
    expect(dialog.getByText("Set up Codex to choose")).toBeTruthy();

    fireEvent.change(dialog.getByLabelText("Default mode"), { target: { value: "plan" } });
    await tick();
    expect(updates(t)).toEqual([{ "defaults.mode": "plan" }]);
    expect((dialog.getByLabelText("Default mode") as HTMLSelectElement).value).toBe("plan");
    // New chats read the defaults from the agent list, so it is read again.
    expect(t.calls.filter((x) => x.op === "agent.list").length).toBeGreaterThan(1);
    c.dispose();
  });

  it("clears the effort when the chosen model does not offer it", async () => {
    const t = host({ "defaults.claude_code.effort": "low" });
    const c = await mount(t);
    const dialog = openSettings("Models");
    await tick();
    fireEvent.change(dialog.getByLabelText("Claude Code model"), { target: { value: "haiku" } });
    await tick();
    expect(updates(t)).toEqual([{ "defaults.claude_code.model": "haiku", "defaults.claude_code.effort": null }]);
    expect(dialog.queryByLabelText("Claude Code effort")).toBeNull();
    c.dispose();
  });

  it("asks again while the host reads an agent's models, before its first chat", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const t = host();
      let learned = false;
      const codexNew = { ...codex, available: true };
      t.handlers["agent.list"] = () => [claude, learned ? { ...codexNew, options: claude.options } : codexNew];
      const c = await mount(t);
      const dialog = openSettings("Models");
      await tick();
      expect(dialog.getByText("Reading Codex's models…")).toBeTruthy();
      learned = true;
      await vi.advanceTimersByTimeAsync(2000);
      await tick();
      expect(dialog.getByLabelText("Codex model")).toBeTruthy();
      const lists = t.calls.filter((x) => x.op === "agent.list").length;
      await vi.advanceTimersByTimeAsync(6000);
      expect(t.calls.filter((x) => x.op === "agent.list").length).toBe(lists);
      c.dispose();
    } finally {
      vi.useRealTimers();
    }
  });

  it("is not offered by a host without shared settings", async () => {
    const t = host();
    t.handlers["host.hello"] = () => hostHello({ capabilities: CLIENT_CAPABILITIES.filter((x) => x !== "settings") });
    const c = await mount(t);
    fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
    const nav = within(screen.getByRole("navigation", { name: "Settings sections" }));
    expect(nav.queryByText("Models")).toBeNull();
    c.dispose();
  });
});
