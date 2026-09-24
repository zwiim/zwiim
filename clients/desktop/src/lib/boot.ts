// Finding (or starting) the local host, then handing a connected controller
// to the UI. Desktop state is disposable: nothing here is persisted.

import {
  HostClient,
  HostController,
  LocalTransport,
  ZwiimClientError,
  type Transport,
} from "@zwiim/sdk";
import type { DiscoveredHost, NativeBridge } from "./bridge";

export type BootPhase =
  | { phase: "discovering" }
  | { phase: "starting" }
  | { phase: "ready"; controller: HostController }
  | { phase: "failed"; message: string };

export interface BootOptions {
  startTimeoutMs?: number;
  pollMs?: number;
  makeTransport?: (host: DiscoveredHost) => Transport;
}

/** Connection error code: no local host to connect to (it can be started). */
export const HOST_NOT_RUNNING = "host_not_running";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/**
 * Read discovery. An unreadable or corrupt file is reported, not thrown:
 * `null` host plus the problem, so boot can still start a host that rewrites it.
 */
async function discover(bridge: NativeBridge): Promise<{ host: DiscoveredHost | null; problem: string | null }> {
  try {
    return { host: await bridge.discover(), problem: null };
  } catch (e) {
    return { host: null, problem: message(e) };
  }
}

const defaultTransport = (h: DiscoveredHost) => new LocalTransport({ url: h.url, token: h.token });

/** A running host that answers at its advertised URL (or rejects our token). */
async function reachable(host: DiscoveredHost | null, make: BootOptions["makeTransport"]) {
  if (!host || !host.running) return false;
  try {
    await (make ?? defaultTransport)(host).request("host.hello", {});
    return true;
  } catch (e) {
    // A wrong token is still a live host; the client reports it.
    return e instanceof ZwiimClientError && e.code === "unauthenticated";
  }
}

export async function bootHost(
  bridge: NativeBridge,
  onPhase: (p: BootPhase) => void,
  options: BootOptions = {},
): Promise<HostController | null> {
  const { startTimeoutMs = 20_000, pollMs = 250, makeTransport } = options;
  onPhase({ phase: "discovering" });
  let { host } = await discover(bridge);
  if (!(await reachable(host, makeTransport))) {
    onPhase({ phase: "starting" });
    try {
      await bridge.startHost();
    } catch (e) {
      onPhase({ phase: "failed", message: message(e) });
      return null;
    }
    const deadline = Date.now() + startTimeoutMs;
    const stalePid = host?.pid;
    for (;;) {
      await sleep(pollMs);
      const found = await discover(bridge);
      host = found.host;
      if (host && host.pid !== stalePid && (await reachable(host, makeTransport))) break;
      if (Date.now() > deadline) {
        // A discovery file the new host never replaced is the likelier cause.
        onPhase({
          phase: "failed",
          message: found.problem
            ? `The host did not start in time. ${found.problem}`
            : "The host did not start in time",
        });
        return null;
      }
    }
  }

  const client = new HostClient({
    expectedHostId: host!.host_id || undefined,
    // Re-read discovery before every (re)connect: a restarted host may
    // come back on a new port.
    transport: async () => {
      const { host: current, problem } = await discover(bridge);
      // Either way no host can be reached here; starting one rewrites discovery.
      if (problem) throw new ZwiimClientError(HOST_NOT_RUNNING, problem);
      if (!current || !current.running) {
        throw new ZwiimClientError(HOST_NOT_RUNNING, "The host is not running");
      }
      return (makeTransport ?? defaultTransport)(current);
    },
  });
  const controller = new HostController(client);
  client.start();
  onPhase({ phase: "ready", controller });
  return controller;
}
