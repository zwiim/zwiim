import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type HostUpdateStatus } from "@zwiim/sdk";
import type { DesktopUpdateStatus, DesktopUpdates, NativeBridge } from "@/lib/bridge";
import { HostProvider } from "@/lib/host-context";
import { AboutUpdates, HostLoginToggle } from "@/panels/About";
import { UpdateCard, useUpdates } from "@/panels/UpdateCard";
import { FakeTransport, hostHello, tick } from "./fakes";

afterEach(cleanup);

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
      {!remote && <HostLoginToggle bridge={b} />}
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
      hostStatus({ staged: { version: "0.3.0", source: "bundle" }, restart_required: true });
    t.handlers["host.update.apply"] = () => ({
      version: "0.3.0",
      state: "restart_pending",
      blockers: { live_turns: 1, pending_requests: 0, terminals: 2, session_ids: ["s1"] },
    });
    const updates = desktopUpdates({
      version: "0.3.0",
      supported: true,
      unsupported_reason: null,
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
    expect(screen.getByTestId("host-update-pending").textContent).toContain("1 running turn, 2 open terminals");
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
    t.handlers["host.update.status"] = () => hostStatus({ supported: false, unsupported_reason: "unsigned_build" });
    const updates = desktopUpdates({
      version: "0.1.0",
      supported: false,
      unsupported_reason: "unsigned_build",
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
    t.handlers["host.update.status"] = () => hostStatus();
    const updates = desktopUpdates({
      version: "0.2.0",
      supported: true,
      unsupported_reason: null,
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

/** The sidebar's update card, fed by the same hook the app uses. */
function Card({ b, onSeeChanges = () => {} }: { b: NativeBridge; onSeeChanges?: () => void }) {
  const updates = useUpdates(b);
  return <UpdateCard bridge={b} updates={updates} remote={false} onSeeChanges={onSeeChanges} />;
}

const applies = (t: FakeTransport) => t.calls.filter((x) => x.op === "host.update.apply").map((x) => x.params);

describe("update card", () => {
  it("offers to restart an older host on this computer, when idle or now", async () => {
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
    fireEvent.click(screen.getByText("Restart when idle"));
    await tick();
    expect(card.textContent).toContain("The host restarts when 1 running turn finishes.");
    fireEvent.click(screen.getByText("Restart now"));
    await tick();
    expect(applies(t)).toEqual([{ mode: "when_idle" }, { mode: "now" }]);
    controller.dispose();
  });

  it("shows a host update ready to use, its changes, and can be dismissed", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () =>
      hostStatus({ staged: { version: "0.3.0", source: "download" }, restart_required: true });
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
    fireEvent.click(screen.getByText("Restart"));
    await tick();
    expect(applies(t)).toEqual([{ mode: "now" }]);
    expect(card.textContent).toContain("Restarting the host…");
    fireEvent.click(screen.getByLabelText("Dismiss"));
    expect(screen.queryByTestId("update-card")).toBeNull();
    controller.dispose();
  });

  it("installs this app's update, then restarts into it", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus();
    const controller = await connect(t);
    const u = desktopUpdates({
      version: "0.2.0",
      supported: true,
      unsupported_reason: null,
      activity: { state: "available", version: "0.3.0", notes: null },
    });
    const b = bridge(u);
    const setDesktop = vi.fn();
    const view = (desktop: DesktopUpdateStatus) => (
      <HostProvider value={controller}>
        <UpdateCard
          bridge={b}
          updates={{ desktop, host: null, setDesktop, hint: true, checks: 0, check: async () => true }}
          remote={false}
          onSeeChanges={() => {}}
        />
      </HostProvider>
    );
    const { rerender } = render(view(u.current));
    expect(screen.getByTestId("update-card").textContent).toContain("Zwiim Desktop 0.3.0");
    fireEvent.click(screen.getByText("Update"));
    await tick();
    expect(u.install).toHaveBeenCalledTimes(1);
    expect(setDesktop).toHaveBeenCalledWith(u.current);
    rerender(view({ ...u.current, activity: { state: "ready", version: "0.3.0" } }));
    fireEvent.click(screen.getByText("Restart"));
    await tick();
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
    const updates = desktopUpdates({ version: "0.2.0", supported: true, unsupported_reason: null, activity: { state: "up_to_date" } });
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
