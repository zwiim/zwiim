import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CLIENT_CAPABILITIES, HostClient, HostController, type HostUpdateStatus } from "@zwiim/sdk";
import type { DesktopUpdateStatus, DesktopUpdates, NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { AboutUpdates, blockersText } from "@/panels/About";
import { HostLoginToggle } from "@/settings/general";
import { SETTINGS } from "@/settings/registry";
import { UpdateCard, UpdateChoices, useUpdates } from "@/panels/UpdateCard";
import { flowTiming, resetRestartFlow, type Blockers } from "@/panels/update-flow";
import { FakeTransport, hostHello, tick } from "./fakes";

const watchMs = flowTiming.watchMs;

afterEach(() => {
  cleanup();
  resetRestartFlow();
  flowTiming.watchMs = watchMs;
});

function hostStatus(overrides: Partial<HostUpdateStatus> = {}): HostUpdateStatus {
  return {
    version: "0.2.0",
    channel: "stable",
    supported: true,
    unsupported_reason: null,
    activity: { state: "idle" },
    available: null,
    staged: null,
    restart_required: false,
    trial: null,
    previous: null,
    last_result: null,
    last_checked: null,
    platform_signature: "enforced",
    bundled_with_desktop: true,
    ...overrides,
  };
}

function desktopUpdates(initial: DesktopUpdateStatus): DesktopUpdates & { current: DesktopUpdateStatus } {
  const u = {
    current: initial,
    status: vi.fn(async () => u.current),
    check: vi.fn(async () => u.current),
    install: vi.fn(async () => {
      u.current = {
        ...u.current,
        activity: { state: "downloading", version: "0.3.0", downloaded: 2_000_000, total: 8_000_000 },
      };
      return u.current;
    }),
    restart: vi.fn(async () => {}),
    setChannel: vi.fn(async (channel: DesktopUpdateStatus["channel"]) => {
      u.current = { ...u.current, channel };
      return u.current;
    }),
  };
  return u;
}

function bridge(updates?: DesktopUpdates): NativeBridge {
  return {
    discover: vi.fn(async () => null),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: false, account: null })),
    signIn: vi.fn(async () => ({ user_id: "u1", email: null })),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {}),
    updates,
  };
}

async function mount(t: FakeTransport, b: NativeBridge, remote = false) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  render(
    <HostProvider value={controller}>
      <AboutUpdates bridge={b} remote={remote} />
      {!remote && <HostLoginToggle bridge={b} setting={SETTINGS.find((x) => x.id === "host-login")!} />}
    </HostProvider>,
  );
  await tick();
  return controller;
}

describe("About & updates", () => {
  it("shows both versions and restarts the host into a staged update, when idle first", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () => hostHello({ version: "0.2.0" });
    t.handlers["host.update.status"] = () =>
      hostStatus({ staged: { version: "0.3.0", source: "bundle" }, restart_required: true, bundled_with_desktop: false });
    t.handlers["host.update.apply"] = () => ({
      version: "0.3.0",
      state: "restart_pending",
      blockers: { live_turns: 1, pending_requests: 0, terminals: 2, session_ids: ["s1"] },
    });
    const updates = desktopUpdates({
      version: "0.3.0",
      supported: true,
      unsupported_reason: null,
      channel: "stable",
      activity: { state: "up_to_date" },
    });
    const c = await mount(t, bridge(updates));
    expect(screen.getByTestId("about-desktop").textContent).toContain("0.3.0");
    expect(screen.getByTestId("about-host").textContent).toContain("0.2.0");
    expect(screen.getByTestId("about-compat").textContent).toBe("Compatible");
    expect(screen.getByTestId("host-update-state").textContent).toMatch(/Version 0\.3\.0 is ready/);

    fireEvent.click(screen.getByText("Restart host"));
    await tick();
    expect(t.calls.filter((x) => x.op === "host.update.apply").map((x) => x.params)).toEqual([{ mode: "when_idle" }]);
    expect(screen.getByTestId("host-update-pending").textContent).toContain("The host restarts when 1 running turn finishes.");
    fireEvent.click(screen.getByText("Restart now"));
    await tick();
    expect(t.calls.filter((x) => x.op === "host.update.apply").at(-1)!.params).toEqual({ mode: "now" });
    c.dispose();
  });

  it("says a failed host update was rolled back", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({
        previous: "0.3.0",
        last_result: {
          outcome: "rolled_back",
          version: "0.3.0",
          from: "0.2.0",
          reason: "The new version did not pass its health check.",
          at: "now",
        },
      });
    const c = await mount(t, bridge());
    expect(screen.getByTestId("host-update-rolled-back").textContent).toBe(
      "The update to 0.3.0 failed, so the host went back to 0.2.0. The new version did not pass its health check.",
    );
    c.dispose();
  });

  it("a development build of either side says it does not update itself", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({ supported: false, unsupported_reason: "unsigned_build", bundled_with_desktop: false });
    const updates = desktopUpdates({
      version: "0.1.0",
      supported: false,
      unsupported_reason: "unsigned_build",
      channel: "stable",
      activity: { state: "idle" },
    });
    const c = await mount(t, bridge(updates));
    expect(screen.getByTestId("about-desktop").textContent).toMatch(/does not update itself/);
    expect(screen.getByTestId("host-update-unsupported").textContent).toMatch(/not built by the release pipeline/);
    expect(screen.queryByText("Check for updates")).toBeNull();
    c.dispose();
  });

  it("downloads a Desktop update only when asked, and shows its progress", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ bundled_with_desktop: false });
    const updates = desktopUpdates({
      version: "0.2.0",
      supported: true,
      unsupported_reason: null,
      channel: "stable",
      activity: { state: "available", version: "0.3.0", notes: "### Added\n- Updates" },
    });
    const c = await mount(t, bridge(updates));
    expect(updates.install).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Download and install"));
    await tick();
    expect(updates.install).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("about-desktop").textContent).toContain("Downloading 0.3.0… 2.0 MB of 8.0 MB");
    c.dispose();
  });
});

async function connect(t: FakeTransport) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  return controller;
}

/**
 * The sidebar's update card, fed by the same hook the app uses. This app's
 * update is read at launch, as the first check would.
 */
function Card({ b, onSeeChanges = () => {} }: { b: NativeBridge; onSeeChanges?: () => void }) {
  const updates = useUpdates(b, false);
  const setDesktop = updates.setDesktop;
  useEffect(() => {
    void b.updates?.status().then(setDesktop);
  }, [b, setDesktop]);
  return <UpdateCard bridge={b} updates={updates} remote={false} onSeeChanges={onSeeChanges} />;
}

const applies = (t: FakeTransport) => t.calls.filter((x) => x.op === "host.update.apply").map((x) => x.params);
const idle = { live_turns: 0, pending_requests: 0, terminals: 0, session_ids: [] };
const staged = { version: "0.3.0", source: "bundle" as const };
const wait = (ms: number) => act(async () => void (await new Promise((r) => setTimeout(r, ms))));

function installed(version: string, activity: DesktopUpdateStatus["activity"] = { state: "ready", version: "0.3.0" }) {
  return desktopUpdates({ version, supported: true, unsupported_reason: null, channel: "stable", activity });
}

describe("update card", () => {
  it("lists what keeps a restart waiting, and not open terminals", () => {
    expect(blockersText({ ...idle, live_turns: 1, pending_requests: 2, terminals: 3, starting: 2 })).toBe(
      "1 running turn, 2 waiting requests, 2 starting agents",
    );
    expect(blockersText({ ...idle, terminals: 1 })).toBe("");
  });

  it("counts an agent waiting on a request once in what Restart now stops", () => {
    const stops = (b: Blockers) => {
      const { unmount } = render(
        <HostProvider value={{} as HostController}>
          <UpdateChoices target={{ host: true, desktop: null, version: "0.3.0" }} blockers={b} />
        </HostProvider>,
      );
      const title = screen.getByText("Restart now").getAttribute("title");
      unmount();
      return title;
    };
    expect(stops({ ...idle, live_turns: 1, pending_requests: 1, session_ids: ["s1"] })).toBe(
      "Stops 1 running agent and closes open terminals.",
    );
    expect(stops({ ...idle, live_turns: 1, pending_requests: 1 })).toBe("Stops 2 running agents and closes open terminals.");
  });

  it("updates an older host on this computer once idle, or now while it waits", async () => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = () => hostHello({ revision: 0 });
    t.handlers["host.update.apply"] = () => ({
      version: "0.3.0",
      state: "restart_pending",
      blockers: { live_turns: 1, pending_requests: 0, terminals: 0, session_ids: ["s1"] },
    });
    const controller = await connect(t);
    render(
      <HostProvider value={controller}>
        <Card b={bridge()} />
      </HostProvider>,
    );
    const card = screen.getByTestId("update-card");
    expect(card.textContent).toMatch(/older than this app/);
    expect(screen.queryByLabelText("Dismiss")).toBeNull();
    fireEvent.click(screen.getByText("Update"));
    await tick();
    expect(card.textContent).toContain("The host restarts when 1 running turn finishes.");
    expect(screen.queryByText("Update")).toBeNull();
    fireEvent.click(screen.getByText("Restart now"));
    await tick();
    expect(applies(t)).toEqual([{ mode: "when_idle" }, { mode: "now" }]);
    controller.dispose();
  });

  it("shows a host update ready to use, its changes, and can be dismissed", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({ staged: { version: "0.3.0", source: "download" }, restart_required: true, bundled_with_desktop: false });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: {} });
    const controller = await connect(t);
    const seen = vi.fn();
    render(
      <HostProvider value={controller}>
        <Card b={bridge()} onSeeChanges={seen} />
      </HostProvider>,
    );
    await tick();
    const card = screen.getByTestId("update-card");
    expect(card.textContent).toContain("New update available");
    expect(card.textContent).toContain("Zwiim Host 0.3.0");
    fireEvent.click(screen.getByText("See changes"));
    expect(seen).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByText("Update"));
    await tick();
    expect(applies(t)).toEqual([{ mode: "when_idle" }]);
    expect(card.textContent).toContain("Restarting the host…");
    fireEvent.click(screen.getByLabelText("Dismiss"));
    expect(screen.queryByTestId("update-card")).toBeNull();
    controller.dispose();
  });

  it("shows a host update another window already asked for as waiting", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({
        staged: { version: "0.3.0", source: "bundle" },
        restart_required: true,
        restart_pending: true,
        bundled_with_desktop: false,
      });
    const controller = await connect(t);
    render(
      <HostProvider value={controller}>
        <Card b={bridge()} />
      </HostProvider>,
    );
    await tick();
    expect(screen.getByTestId("update-card").textContent).toContain("The host restarts when its running work finishes.");
    expect(screen.queryByText("Update")).toBeNull();
    expect(screen.getByText("Restart now")).toBeTruthy();
    controller.dispose();
  });

  it("updates this app and its host with one click", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ blockers: idle });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: idle });
    const controller = await connect(t);
    const u = desktopUpdates({
      version: "0.2.0",
      supported: true,
      unsupported_reason: null,
      channel: "stable",
      activity: { state: "available", version: "0.3.0", notes: null },
    });
    const b = bridge(u);
    const setDesktop = vi.fn();
    const view = (desktop: DesktopUpdateStatus, host: HostUpdateStatus) => (
      <HostProvider value={controller}>
        <UpdateCard
          bridge={b}
          updates={{ desktop, host, unified: true, setDesktop, hint: true, checks: 0, check: async () => true }}
          remote={false}
          onSeeChanges={() => {}}
        />
      </HostProvider>
    );
    const { rerender } = render(view(u.current, hostStatus({ blockers: idle })));
    expect(screen.getByTestId("update-card").textContent).toContain("Zwiim 0.3.0 for this app and its host");
    fireEvent.click(screen.getByText("Update"));
    await tick();
    expect(u.install).toHaveBeenCalledTimes(1);
    expect(setDesktop).toHaveBeenCalledWith(u.current);
    rerender(view(u.current, hostStatus({ blockers: idle })));
    expect(screen.getByTestId("update-progress")).toBeTruthy();
    // Installing staged the Host it brings. Nothing restarts by itself.
    rerender(view({ ...u.current, activity: { state: "ready", version: "0.3.0" } }, hostStatus({ staged, blockers: idle })));
    await tick();
    const card = screen.getByTestId("update-card");
    expect(card.getAttribute("aria-label")).toBe("Update ready");
    expect(screen.getByTestId("update-detail").textContent).toBe(
      "Zwiim 0.3.0 is downloaded for this app and its host. Sessions and workspaces are kept.",
    );
    expect(applies(t)).toEqual([]);
    expect(u.restart).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Restart when idle"));
    await tick();
    expect(applies(t)).toEqual([{ mode: "now" }]);
    expect(u.restart).toHaveBeenCalledTimes(1);
    controller.dispose();
  });

  it("waits for running agents, then asks before restarting", async () => {
    flowTiming.watchMs = 10;
    const t = new FakeTransport();
    let blockers = { ...idle, live_turns: 1 };
    t.handlers["host.update.status"] = () => hostStatus({ staged, restart_required: true, blockers });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: idle });
    const controller = await connect(t);
    const u = installed("0.2.0");
    render(
      <HostProvider value={controller}>
        <Card b={bridge(u)} />
      </HostProvider>,
    );
    await tick();
    fireEvent.click(screen.getByText("Restart when idle"));
    await tick();
    expect(screen.getByTestId("update-detail").textContent).toBe("Waiting for 1 running turn to finish.");
    expect(screen.getByText("Restart now").getAttribute("title")).toBe("Stops 1 running agent and closes open terminals.");
    expect(screen.queryByText("Restart when idle")).toBeNull();
    blockers = idle;
    await wait(100);
    const card = screen.getByTestId("update-card");
    expect(card.getAttribute("aria-label")).toBe("Ready to restart");
    expect(screen.getByTestId("update-detail").textContent).toBe("Agents are done. Restart to finish updating to 0.3.0.");
    expect(applies(t)).toEqual([]);
    expect(u.restart).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Restart"));
    await tick();
    expect(applies(t)).toEqual([{ mode: "now" }]);
    expect(u.restart).toHaveBeenCalledTimes(1);
    controller.dispose();
  });

  async function readyToRestart(t: FakeTransport) {
    flowTiming.watchMs = 10;
    let blockers = { ...idle, live_turns: 1 };
    t.handlers["host.update.status"] = () => hostStatus({ staged, restart_required: true, blockers });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: idle });
    const controller = await connect(t);
    const u = installed("0.2.0");
    render(
      <HostProvider value={controller}>
        <Card b={bridge(u)} />
      </HostProvider>,
    );
    await tick();
    fireEvent.click(screen.getByText("Restart when idle"));
    await tick();
    blockers = idle;
    await wait(100);
    expect(screen.getByTestId("update-card").getAttribute("aria-label")).toBe("Ready to restart");
    return { controller, u, work: (b: typeof idle) => void (blockers = b) };
  }

  it("goes back to waiting when an agent starts while it asks to restart", async () => {
    const t = new FakeTransport();
    const { controller, work } = await readyToRestart(t);
    work({ ...idle, live_turns: 1 });
    await wait(60);
    expect(screen.getByTestId("update-detail").textContent).toBe("Waiting for 1 running turn to finish.");
    controller.dispose();
  });

  it("does not restart on Restart when an agent has started since", async () => {
    const t = new FakeTransport();
    const { controller, u, work } = await readyToRestart(t);
    work({ ...idle, live_turns: 1 });
    fireEvent.click(screen.getByText("Restart"));
    await tick();
    expect(applies(t)).toEqual([]);
    expect(u.restart).not.toHaveBeenCalled();
    expect(screen.getByTestId("update-detail").textContent).toBe("Waiting for 1 running turn to finish.");
    controller.dispose();
  });

  it("keeps asking to restart through a brief host blip", async () => {
    const t = new FakeTransport();
    const { controller } = await readyToRestart(t);
    t.handlers["host.update.status"] = () => {
      throw new Error("host is down");
    };
    await wait(60);
    t.handlers["host.update.status"] = () => hostStatus({ staged, restart_required: true, blockers: idle });
    await wait(60);
    expect(screen.getByTestId("update-card").getAttribute("aria-label")).toBe("Ready to restart");
    controller.dispose();
  });

  it("checks only this app when unified", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ blockers: idle });
    const controller = await connect(t);
    const u = installed("0.2.0", { state: "up_to_date" });
    function Check() {
      const updates = useUpdates(bridge(u), false);
      return <button onClick={() => void updates.check()}>Check</button>;
    }
    render(
      <HostProvider value={controller}>
        <Check />
      </HostProvider>,
    );
    await tick();
    fireEvent.click(screen.getByText("Check"));
    await tick();
    expect(u.check).toHaveBeenCalledTimes(1);
    expect(t.calls.filter((x) => x.op === "host.update.check")).toEqual([]);
    expect(t.calls.filter((x) => x.op === "host.update.status").length).toBeGreaterThan(1);
    controller.dispose();
  });

  it("says the host couldn't be prepared and restarts only this app", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ staged: null, restart_required: false, blockers: idle });
    const controller = await connect(t);
    const u = installed("0.2.0");
    render(
      <HostProvider value={controller}>
        <Card b={bridge(u)} />
      </HostProvider>,
    );
    await tick();
    expect(screen.getByTestId("update-card").getAttribute("aria-label")).toBe("Update ready");
    expect(screen.getByTestId("update-detail").textContent).toBe(
      "Zwiim 0.3.0 is downloaded for this app, but its host couldn't be prepared (it stays on 0.2.0). Restart to update this app.",
    );
    expect(screen.queryByText("Restart when idle")).toBeNull();
    fireEvent.click(screen.getByText("Restart Zwiim"));
    await tick();
    expect(u.restart).toHaveBeenCalledTimes(1);
    expect(applies(t)).toEqual([]);
    controller.dispose();
  });

  it.each([
    ["live", () => hostHello()],
    ["update required", () => hostHello({ revision: 0 })],
  ])("finishes an unfinished update when the host is %s", async (_, hello) => {
    const t = new FakeTransport();
    t.handlers["host.hello"] = hello;
    t.handlers["host.update.status"] = () => hostStatus({ staged, restart_required: true, blockers: idle });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: idle });
    const controller = await connect(t);
    const u = installed("0.3.0", { state: "up_to_date" });
    render(
      <HostProvider value={controller}>
        <Card b={bridge(u)} />
      </HostProvider>,
    );
    await tick();
    expect(screen.getByTestId("update-card").getAttribute("aria-label")).toBe("Finish update");
    expect(screen.getByTestId("update-detail").textContent).toBe("This app is on 0.3.0. Its host is still on 0.2.0.");
    fireEvent.click(screen.getByText("Restart now"));
    await tick();
    expect(applies(t)).toEqual([{ mode: "now" }]);
    expect(u.restart).not.toHaveBeenCalled();
    controller.dispose();
  });

  it("goes back to normal once the host returns on the new version", async () => {
    flowTiming.watchMs = 10;
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ staged, restart_required: true, blockers: idle });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: idle });
    const controller = await connect(t);
    const u = installed("0.3.0", { state: "up_to_date" });
    render(
      <HostProvider value={controller}>
        <Card b={bridge(u)} />
      </HostProvider>,
    );
    await tick();
    fireEvent.click(screen.getByText("Restart now"));
    await tick();
    expect(applies(t)).toEqual([{ mode: "now" }]);
    // The host is down for its restart: nothing can be pressed twice.
    expect((screen.getByText("Restart now") as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByText("Restart when idle") as HTMLButtonElement).disabled).toBe(true);
    t.handlers["host.update.status"] = () => hostStatus({ version: "0.3.0", staged: null, restart_required: false, blockers: idle });
    await wait(60);
    expect(screen.queryByTestId("update-card")).toBeNull();
    controller.dispose();
  });

  it("frees the card when the host comes back on its old version", async () => {
    flowTiming.watchMs = 10;
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ staged, restart_required: true, blockers: idle });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: idle });
    const controller = await connect(t);
    const u = installed("0.3.0", { state: "up_to_date" });
    render(
      <HostProvider value={controller}>
        <Card b={bridge(u)} />
      </HostProvider>,
    );
    await tick();
    fireEvent.click(screen.getByText("Restart now"));
    await tick();
    // Still up on 0.2.0, as it may not have restarted yet: nothing is freed.
    await wait(60);
    expect((screen.getByText("Restart now") as HTMLButtonElement).disabled).toBe(true);
    t.handlers["host.update.status"] = () => {
      throw new Error("host is down");
    };
    await wait(60);
    expect((screen.getByText("Restart now") as HTMLButtonElement).disabled).toBe(true);
    // Its trial failed and it rolled back.
    t.handlers["host.update.status"] = () => hostStatus({ staged, restart_required: true, blockers: idle });
    await wait(60);
    expect((screen.getByText("Restart now") as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByText("Restart when idle") as HTMLButtonElement).disabled).toBe(false);
    controller.dispose();
  });

  it("restarts once even when pressed twice", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ staged, restart_required: true, blockers: idle });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: idle });
    const controller = await connect(t);
    const u = installed("0.2.0");
    render(
      <HostProvider value={controller}>
        <Card b={bridge(u)} />
      </HostProvider>,
    );
    await tick();
    const now = screen.getByText("Restart now");
    fireEvent.click(now);
    fireEvent.click(now);
    await tick();
    expect(applies(t)).toEqual([{ mode: "now" }]);
    expect(u.restart).toHaveBeenCalledTimes(1);
    expect((now as HTMLButtonElement).disabled).toBe(true);
    controller.dispose();
  });

  it("leaves a remote host alone when this app updates", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({ staged: { version: "0.3.0", source: "download" }, restart_required: true, bundled_with_desktop: false });
    const controller = await connect(t);
    const u = desktopUpdates({
      version: "0.2.0",
      supported: true,
      unsupported_reason: null,
      channel: "stable",
      activity: { state: "ready", version: "0.3.0" },
    });
    render(
      <HostProvider value={controller}>
        <UpdateCard
          bridge={bridge(u)}
          updates={{
            desktop: u.current,
            host: null,
            unified: false,
            setDesktop: () => {},
            hint: true,
            checks: 0,
            check: async () => true,
          }}
          remote
          onSeeChanges={() => {}}
        />
      </HostProvider>,
    );
    expect(screen.getByTestId("update-card").textContent).toContain("Zwiim Desktop 0.3.0.");
    fireEvent.click(screen.getByText("Update"));
    await tick();
    expect(applies(t)).toEqual([]);
    expect(u.restart).toHaveBeenCalledTimes(1);
    controller.dispose();
  });

  it("stays away when nothing wants an update", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus();
    const controller = await connect(t);
    render(
      <HostProvider value={controller}>
        <Card b={bridge()} />
      </HostProvider>,
    );
    await tick();
    expect(screen.queryByTestId("update-card")).toBeNull();
    controller.dispose();
  });
});

describe("diagnostics", () => {
  it("shows the host's report on request and copies it with this app's version", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus();
    t.handlers["host.diagnostics"] = () => ({
      version: "0.2.0",
      target: "aarch64-apple-darwin",
      channel: "stable",
      protocol: { major: 1, revision: 1, min_client_revision: 1, capabilities: [] },
      release_signing: { release_key: true, platform_signature: "enforced" },
      adapters: {
        claude: { version: "0.81.2", locked_version: "0.81.2", installed: true, healthy: true, authenticated: false, setup: { state: "idle" }, source: "managed" },
        codex: { version: null, locked_version: "1.12.0", installed: false, healthy: null, authenticated: false, setup: { state: "installing" }, source: null },
      },
      update: {
        supported: true, unsupported_reason: null, activity: { state: "idle" }, available: null, staged: null, trial: null,
        previous: "0.1.0", last_result: { outcome: "applied", version: "0.2.0", from: "0.1.0", reason: null, at: "now" },
        last_checked: null, bundled_with_desktop: true,
      },
      database: { schema: { state: "current" }, migrations_known: 52, applied_at_start: 2, snapshot_at_start: true, snapshots: 1 },
    });
    const writeText = vi.fn(async () => {});
    Object.assign(navigator, { clipboard: { writeText } });
    const updates = desktopUpdates({ version: "0.2.0", supported: true, unsupported_reason: null, channel: "stable", activity: { state: "up_to_date" } });
    const c = await mount(t, bridge(updates));
    expect(t.calls.some((x) => x.op === "host.diagnostics")).toBe(false);
    fireEvent.click(screen.getByText("Show"));
    await tick();
    expect(screen.getByTestId("diagnostics-adapter-claude").textContent).toBe("0.81.2 · healthy · not signed in");
    expect(screen.getByTestId("diagnostics-adapter-codex").textContent).toBe("setting up");
    expect(screen.getByTestId("diagnostics-database").textContent).toBe(
      "up to date · 52 migrations · 2 applied at start · 1 snapshot",
    );
    fireEvent.click(screen.getByText("Copy"));
    await tick();
    const report = JSON.parse((writeText.mock.calls[0] as unknown as [string])[0]);
    expect(report.desktop.version).toBe("0.2.0");
    expect(report.host.update.last_result.outcome).toBe("applied");
    expect(screen.getByText("Copied")).toBeTruthy();
    c.dispose();
  });
});

describe("starting the host at login", () => {
  it("is shown for an installed app and changes only the choice", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus();
    const set = vi.fn(async (enabled: boolean) => ({ supported: true, enabled }));
    const b = { ...bridge(), hostLogin: { status: vi.fn(async () => ({ supported: true, enabled: true })), set } };
    const c = await mount(t, b);
    const box = screen.getByTestId("host-login") as HTMLInputElement;
    expect(box.checked).toBe(true);
    fireEvent.click(box);
    await tick();
    expect(set).toHaveBeenCalledWith(false);
    expect((screen.getByTestId("host-login") as HTMLInputElement).checked).toBe(false);
    expect(t.calls.some((x) => x.op.startsWith("host.restart"))).toBe(false);
    c.dispose();
  });

  it("is hidden in a development build and for a remote host", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus();
    const dev = { ...bridge(), hostLogin: { status: vi.fn(async () => ({ supported: false, enabled: false })), set: vi.fn() } };
    const c = await mount(t, dev);
    expect(screen.queryByTestId("host-login")).toBeNull();
    c.dispose();
    cleanup();
    const installed = { ...bridge(), hostLogin: { status: vi.fn(async () => ({ supported: true, enabled: true })), set: vi.fn() } };
    const r = await mount(new FakeTransport(), installed, true);
    expect(screen.queryByTestId("host-login")).toBeNull();
    r.dispose();
  });
});

describe("beta updates", () => {
  const upToDate = (): DesktopUpdateStatus => ({
    version: "0.2.0",
    supported: true,
    unsupported_reason: null,
    channel: "stable",
    activity: { state: "up_to_date" },
  });
  /** A host that offers the channel op and keeps its own channel. */
  function channelHost(initial: HostUpdateStatus["channel"] = "stable") {
    const t = new FakeTransport();
    let channel = initial;
    t.handlers["host.hello"] = () => hostHello({ capabilities: [...CLIENT_CAPABILITIES, "host.update.channel"] });
    t.handlers["host.update.status"] = () => hostStatus({ channel });
    t.handlers["host.update.set_channel"] = (p) => {
      channel = (p as { channel: HostUpdateStatus["channel"] }).channel;
      return hostStatus({ channel });
    };
    return t;
  }
  const sets = (t: FakeTransport) => t.calls.filter((x) => x.op === "host.update.set_channel").map((x) => x.params);

  it("moves this app and the host on this computer together, then checks both", async () => {
    const t = channelHost();
    const u = desktopUpdates(upToDate());
    const c = await mount(t, bridge(u));
    const box = screen.getByTestId("beta-updates") as HTMLInputElement;
    expect(box.checked).toBe(false);
    fireEvent.click(box);
    await tick();
    expect(sets(t)).toEqual([{ channel: "beta" }]);
    expect(u.setChannel).toHaveBeenCalledWith("beta");
    expect(u.check).toHaveBeenCalled();
    expect(t.calls.some((x) => x.op === "host.update.check")).toBe(true);
    expect((screen.getByTestId("beta-updates") as HTMLInputElement).checked).toBe(true);
    fireEvent.click(screen.getByTestId("beta-updates"));
    await tick();
    expect(sets(t)).toEqual([{ channel: "beta" }, { channel: "stable" }]);
    expect((screen.getByTestId("beta-updates") as HTMLInputElement).checked).toBe(false);
    c.dispose();
  });

  it("follows the channel of the host on this computer", async () => {
    const t = channelHost("beta");
    const u = desktopUpdates(upToDate());
    const c = await mount(t, bridge(u));
    await tick();
    expect(u.setChannel).toHaveBeenCalledWith("beta");
    expect((screen.getByTestId("beta-updates") as HTMLInputElement).checked).toBe(true);
    expect(sets(t)).toEqual([]);
    c.dispose();
  });

  it("changes only this app for a remote host or one without channels", async () => {
    const remote = channelHost();
    const u = desktopUpdates(upToDate());
    const r = await mount(remote, bridge(u), true);
    fireEvent.click(screen.getByTestId("beta-updates"));
    await tick();
    expect(u.setChannel).toHaveBeenCalledWith("beta");
    expect(sets(remote)).toEqual([]);
    expect(screen.getByText(/the host keeps its own setting/)).toBeTruthy();
    r.dispose();
    cleanup();

    const old = new FakeTransport();
    old.handlers["host.update.status"] = () => hostStatus();
    const v = desktopUpdates(upToDate());
    const o = await mount(old, bridge(v));
    fireEvent.click(screen.getByTestId("beta-updates"));
    await tick();
    expect(v.setChannel).toHaveBeenCalledWith("beta");
    expect(old.calls.some((x) => x.op === "host.update.set_channel")).toBe(false);
    o.dispose();
  });

  it("is not shown without a Desktop updater", async () => {
    const t = channelHost();
    const c = await mount(t, bridge());
    expect(screen.queryByTestId("beta-updates")).toBeNull();
    c.dispose();
  });
});

describe("Settings: one Zwiim box", () => {
  const calls = (t: FakeTransport, op: string) => t.calls.filter((x) => x.op === op).map((x) => x.params);
  const readyUpdates = (version = "0.2.0") => installed(version, { state: "ready", version: "0.3.0" });

  it("shows one Zwiim box with one check when unified", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ blockers: idle });
    const updates = installed("0.2.0", { state: "up_to_date" });
    const c = await mount(t, bridge(updates));
    expect(screen.getAllByTestId("about-zwiim")).toHaveLength(1);
    expect(screen.queryByTestId("about-host")).toBeNull();
    expect(screen.queryByTestId("about-desktop")).toBeNull();
    expect(screen.queryByText("Restart host")).toBeNull();

    fireEvent.click(screen.getByText("Check for updates"));
    await tick();
    expect(updates.check).toHaveBeenCalledTimes(1);
    expect(calls(t, "host.update.check")).toEqual([]);

    updates.current = { ...updates.current, activity: { state: "available", version: "0.3.0", notes: null } };
    await wait(1100);
    fireEvent.click(screen.getByText("Update"));
    await tick();
    expect(updates.install).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("Download and install")).toBeNull();
    c.dispose();
  });

  it("offers Restart when idle and Restart now once ready, and restarts both", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({ staged: { version: "0.3.0", source: "bundle" }, restart_required: true, blockers: idle });
    t.handlers["host.update.apply"] = () => ({ version: "0.3.0", state: "restarting", blockers: idle });
    const updates = readyUpdates();
    const c = await mount(t, bridge(updates));
    expect(screen.getByText("Restart when idle")).toBeTruthy();
    expect(screen.queryByText("Restart host")).toBeNull();
    expect(screen.getByTestId("about-zwiim").textContent).toContain(
      "Zwiim 0.3.0 is downloaded for this app and its host. Sessions and workspaces are kept.",
    );
    fireEvent.click(screen.getByText("Restart now"));
    await tick();
    expect(calls(t, "host.update.apply")).toEqual([{ mode: "now" }]);
    expect(updates.restart).toHaveBeenCalledTimes(1);
    c.dispose();
  });

  it("restarts only this app when its host could not be prepared", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ blockers: idle });
    const updates = readyUpdates();
    const c = await mount(t, bridge(updates));
    expect(screen.getByTestId("about-zwiim").textContent).toContain(
      "Zwiim 0.3.0 is downloaded for this app, but its host couldn't be prepared (it stays on 0.2.0). Restart to update this app.",
    );
    expect(screen.queryByText("Restart when idle")).toBeNull();
    fireEvent.click(screen.getByText("Restart Zwiim"));
    await tick();
    expect(calls(t, "host.update.apply")).toEqual([]);
    expect(updates.restart).toHaveBeenCalledTimes(1);
    c.dispose();
  });

  it("does not offer a restart of this app alone while the host's answer is stale", async () => {
    const t = new FakeTransport();
    let hostStaged: HostUpdateStatus["staged"] = null;
    t.handlers["host.update.status"] = () => hostStatus({ staged: hostStaged, blockers: idle });
    const updates = installed("0.2.0", { state: "up_to_date" });
    let aloneOffered = false;
    const watch = new MutationObserver(() => {
      if (screen.queryByText("Restart Zwiim")) aloneOffered = true;
    });
    watch.observe(document.body, { childList: true, subtree: true, characterData: true });
    const c = await mount(t, bridge(updates));
    // This app's update is installed, and with it the host's is staged.
    hostStaged = { version: "0.3.0", source: "bundle" };
    updates.current = { ...updates.current, activity: { state: "ready", version: "0.3.0" } };
    await wait(1100);
    watch.disconnect();
    expect(aloneOffered).toBe(false);
    expect(screen.queryByText("Restart Zwiim")).toBeNull();
    expect(screen.getByText("Restart when idle")).toBeTruthy();
    expect(screen.getByText("Restart now")).toBeTruthy();
    c.dispose();
  });

  it("says a development build of both sides does not update itself, with nothing to check", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ supported: false, unsupported_reason: "unsigned_build" });
    const updates = desktopUpdates({
      version: "0.2.0",
      supported: false,
      unsupported_reason: "unsigned_build",
      channel: "stable",
      activity: { state: "idle" },
    });
    const c = await mount(t, bridge(updates));
    const box = screen.getByTestId("about-zwiim");
    expect(screen.getByTestId("host-update-unsupported").textContent).toMatch(/not built by the release pipeline/);
    expect(box.textContent).toMatch(/does not update itself/);
    expect(screen.queryByText("Check for updates")).toBeNull();
    c.dispose();
  });

  it("shows both versions when they differ", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ version: "0.2.0" });
    const c = await mount(t, bridge(installed("0.3.0", { state: "up_to_date" })));
    expect(screen.getByTestId("about-versions").textContent).toBe("Desktop 0.3.0 · Host 0.2.0");
    c.dispose();
    cleanup();

    const t2 = new FakeTransport();
    t2.handlers["host.update.status"] = () => hostStatus({ version: "0.3.0" });
    const c2 = await mount(t2, bridge(installed("0.3.0", { state: "up_to_date" })));
    expect(screen.getByTestId("about-versions").textContent).toBe("0.3.0");
    c2.dispose();
  });

  it("keeps the host's facts as read-only lines", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({ trial: { version: "0.3.0", from: "0.2.0" } });
    const c = await mount(t, bridge(installed("0.2.0", { state: "up_to_date" })));
    expect(screen.getByTestId("about-compat").textContent).toBe("Compatible");
    expect(screen.getByTestId("host-update-trial").textContent).toContain("0.3.0");
    c.dispose();
  });

  it("explains a failed check", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus();
    const c = await mount(
      t,
      bridge(installed("0.2.0", { state: "failed", message: "error sending request" })),
    );
    const box = screen.getByTestId("about-zwiim");
    expect(box.textContent).toContain("Couldn't reach the update server. Check your connection and try again.");
    expect(box.textContent).toContain("error sending request");
    expect(screen.getByText("Check for updates")).toBeTruthy();
    c.dispose();
  });

  it("shows no failure from the host's own check", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({ activity: { state: "failed", reason: "verification", message: "bad signature" } });
    const c = await mount(t, bridge(installed("0.2.0", { state: "up_to_date" })));
    const box = screen.getByTestId("about-zwiim");
    expect(box.textContent).not.toContain("could not be verified");
    expect(box.textContent).not.toContain("bad signature");
    expect(box.textContent).not.toContain("The last update check failed.");
    c.dispose();
  });

  it("keeps the two boxes for a remote host", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus();
    const c = await mount(t, bridge(installed("0.2.0", { state: "up_to_date" })), true);
    expect(screen.queryByTestId("about-zwiim")).toBeNull();
    expect(screen.getByTestId("about-desktop")).toBeTruthy();
    expect(screen.getByTestId("about-host")).toBeTruthy();
    c.dispose();
  });
});
