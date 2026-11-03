import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostClient, HostController, type HostUpdateStatus } from "@zwiim/sdk";
import type { DesktopUpdates } from "@/lib/bridge";
import {
  compareVersions,
  flowTiming,
  hostPreparedFor,
  resetRestartFlow,
  restartNow,
  restartWhenIdle,
  useRestartPhase,
  useRestartWatch,
  type Blockers,
  type RestartTarget,
} from "@/panels/update-flow";
import { FakeTransport, tick } from "./fakes";

const idle: Blockers = { live_turns: 0, pending_requests: 0, terminals: 0, session_ids: [] };

function hostStatus(overrides: Partial<HostUpdateStatus> = {}): HostUpdateStatus {
  return {
    version: "0.1.0-rc.13",
    channel: "stable",
    supported: true,
    unsupported_reason: null,
    activity: { state: "idle" },
    available: null,
    staged: { version: "0.1.0-rc.14", source: "bundle" },
    restart_required: true,
    trial: null,
    previous: null,
    last_result: null,
    last_checked: null,
    platform_signature: "enforced",
    bundled_with_desktop: true,
    blockers: idle,
    ...overrides,
  };
}

async function connect(t: FakeTransport) {
  const client = new HostClient({ transport: t, random: () => 0.5 });
  const controller = new HostController(client);
  client.start();
  await tick();
  return controller;
}

const desktop = () => ({ restart: vi.fn(async () => {}) }) as unknown as DesktopUpdates & { restart: ReturnType<typeof vi.fn> };
const sent = (t: FakeTransport, op: string) => t.calls.filter((c) => c.op === op).map((c) => c.params);
const wait = (ms: number) => act(async () => void (await new Promise((r) => setTimeout(r, ms))));

/** The Host and the shared phase, as the app reads them. */
function watch(controller: HostController) {
  return renderHook(() => {
    useRestartWatch(controller);
    return useRestartPhase();
  });
}

beforeEach(() => {
  flowTiming.watchMs = 10;
  resetRestartFlow();
});
afterEach(cleanup);

describe("versions", () => {
  it("compares versions with prereleases", () => {
    expect(compareVersions("0.1.0-rc.13", "0.1.0-rc.14")).toBeLessThan(0);
    expect(compareVersions("0.1.0-rc.14", "0.1.0")).toBeLessThan(0);
    expect(compareVersions("0.1.0", "0.1.1")).toBeLessThan(0);
    expect(compareVersions("0.1.0", "0.1.0")).toBe(0);
    expect(compareVersions("0.1.0-rc.9", "0.1.0-rc.10")).toBeLessThan(0);
    expect(compareVersions("0.1.1", "0.1.0-rc.14")).toBeGreaterThan(0);
  });

  it("says a host is prepared when current or staged at the version", () => {
    expect(hostPreparedFor(hostStatus({ version: "0.1.0-rc.14", staged: null }), "0.1.0-rc.14")).toBe(true);
    expect(hostPreparedFor(hostStatus(), "0.1.0-rc.14")).toBe(true);
    expect(hostPreparedFor(hostStatus({ supported: false }), "0.1.0-rc.14")).toBe(false);
    expect(hostPreparedFor(hostStatus({ staged: { version: "0.1.0-rc.12", source: "bundle" } }), "0.1.0-rc.14")).toBe(false);
  });
});

describe("restart flow", () => {
  it("restarts at once when no agent works, even with terminals open", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ blockers: { ...idle, terminals: 2 } });
    t.handlers["host.update.apply"] = () => ({ version: "0.1.0-rc.14", state: "restarting", blockers: idle });
    const c = await connect(t);
    const d = desktop();
    await restartWhenIdle(c, { host: true, desktop: d, version: "0.1.0-rc.14" });
    expect(sent(t, "host.update.apply")).toEqual([{ mode: "now" }]);
    expect(d.restart).toHaveBeenCalledTimes(1);
    c.dispose();
  });

  it("waits while agents work, then asks once", async () => {
    const t = new FakeTransport();
    let blockers = { ...idle, live_turns: 1 };
    t.handlers["host.update.status"] = () => hostStatus({ blockers });
    const c = await connect(t);
    const d = desktop();
    const target: RestartTarget = { host: true, desktop: d, version: "0.1.0-rc.14" };
    const view = watch(c);
    await act(() => restartWhenIdle(c, target));
    expect(view.result.current.kind).toBe("waiting");
    expect(sent(t, "host.update.apply")).toEqual([]);
    await wait(60);
    expect(view.result.current.kind).toBe("waiting");
    blockers = idle;
    await wait(80);
    expect(view.result.current.kind).toBe("ready");
    expect(sent(t, "host.update.apply")).toEqual([]);
    expect(d.restart).not.toHaveBeenCalled();
    c.dispose();
  });

  it("keeps watching through a read that fails while the host restarts", async () => {
    const t = new FakeTransport();
    let down = false;
    t.handlers["host.update.status"] = () => {
      if (down) throw new Error("host unavailable");
      return hostStatus({ blockers: { ...idle, live_turns: 1 } });
    };
    const c = await connect(t);
    const view = watch(c);
    await act(() => restartWhenIdle(c, { host: true, desktop: desktop(), version: "0.1.0-rc.14" }));
    down = true;
    await wait(50);
    expect(view.result.current.kind).toBe("waiting");
    t.handlers["host.update.status"] = () => hostStatus({ blockers: idle });
    await wait(80);
    expect(view.result.current.kind).toBe("ready");
    c.dispose();
  });

  it("goes back to waiting when an agent starts before Restart", async () => {
    const t = new FakeTransport();
    let blockers = idle;
    t.handlers["host.update.status"] = () => hostStatus({ blockers });
    const c = await connect(t);
    const d = desktop();
    const target: RestartTarget = { host: true, desktop: d, version: "0.1.0-rc.14" };
    const view = watch(c);
    blockers = { ...idle, starting: 1 };
    await act(() => restartWhenIdle(c, target));
    expect(view.result.current).toMatchObject({ kind: "waiting", blockers: { starting: 1 } });
    blockers = idle;
    await wait(80);
    expect(view.result.current.kind).toBe("ready");
    blockers = { ...idle, live_turns: 1 };
    await act(() => restartWhenIdle(c, target));
    expect(view.result.current.kind).toBe("waiting");
    expect(sent(t, "host.update.apply")).toEqual([]);
    expect(d.restart).not.toHaveBeenCalled();
    c.dispose();
  });

  it("an old host held only by terminals restarts now", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ blockers: undefined });
    const modes: string[] = [];
    t.handlers["host.update.apply"] = (p) => {
      modes.push(p.mode);
      return p.mode === "now"
        ? { version: "0.1.0-rc.14", state: "restarting", blockers: idle }
        : { version: "0.1.0-rc.14", state: "restart_pending", blockers: { ...idle, terminals: 1 } };
    };
    const c = await connect(t);
    const d = desktop();
    await restartWhenIdle(c, { host: true, desktop: d, version: "0.1.0-rc.14" });
    expect(modes).toEqual(["when_idle", "now"]);
    expect(d.restart).toHaveBeenCalledTimes(1);
    c.dispose();
  });

  it("an old host that comes back on the new version relaunches this app", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus({ blockers: undefined });
    t.handlers["host.update.apply"] = () => ({
      version: "0.1.0-rc.14",
      state: "restart_pending",
      blockers: { ...idle, live_turns: 1 },
    });
    const c = await connect(t);
    const d = desktop();
    const view = watch(c);
    await act(() => restartWhenIdle(c, { host: true, desktop: d, version: "0.1.0-rc.14" }));
    expect(view.result.current.kind).toBe("waiting");
    t.handlers["host.update.status"] = () => hostStatus({ version: "0.1.0-rc.14", staged: null, blockers: undefined });
    await wait(60);
    expect(d.restart).toHaveBeenCalledTimes(1);
    expect(view.result.current.kind).toBe("restarting");
    c.dispose();
  });

  it("does not relaunch when the host refuses to restart", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.status"] = () => hostStatus();
    t.handlers["host.update.apply"] = () => {
      throw new Error("update_unavailable");
    };
    const c = await connect(t);
    const d = desktop();
    const view = watch(c);
    await expect(act(() => restartNow(c, { host: true, desktop: d, version: "0.1.0-rc.14" }))).rejects.toThrow();
    expect(d.restart).not.toHaveBeenCalled();
    expect(view.result.current.kind).toBe("idle");
    c.dispose();
  });

  it("host-only target never relaunches this app", async () => {
    const t = new FakeTransport();
    t.handlers["host.update.apply"] = () => ({ version: "0.1.0-rc.14", state: "restarting", blockers: idle });
    const c = await connect(t);
    await restartNow(c, { host: true, desktop: null, version: "0.1.0-rc.14" });
    expect(sent(t, "host.update.apply")).toEqual([{ mode: "now" }]);
    c.dispose();
  });

  it("desktop-only target never touches the host", async () => {
    const t = new FakeTransport();
    const c = await connect(t);
    const d = desktop();
    await restartNow(c, { host: false, desktop: d, version: "0.1.0-rc.14" });
    await restartWhenIdle(c, { host: false, desktop: d, version: "0.1.0-rc.14" });
    expect(d.restart).toHaveBeenCalledTimes(2);
    expect(t.calls.filter((x) => x.op.startsWith("host.update"))).toEqual([]);
    c.dispose();
  });
});
