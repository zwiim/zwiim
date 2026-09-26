// DEV ONLY. The real-network checks this slice is accepted on, run in order
// against a paired host: each mode connects, reads the host, counts events,
// disconnects and reconnects. Results carry no secrets.

import type { ConnectionMode } from "@zwiim/sdk";
import { errorInfo, type HostConnection, type HostConnectionView } from "../host/connection";

export interface CheckResult {
  step: string;
  ok: boolean;
  detail: string;
}

/** The view once `test` holds; rejects after `ms` with the last view. */
export function waitForView(
  connection: HostConnection,
  test: (view: HostConnectionView) => boolean,
  ms = 20_000,
): Promise<HostConnectionView> {
  return new Promise((resolve, reject) => {
    if (test(connection.view)) return resolve(connection.view);
    const timer = setTimeout(() => {
      off();
      const v = connection.view;
      reject(new Error(`timed out: ${v.status}/${v.active}${v.error ? ` (${v.error.code})` : ""}`));
    }, ms);
    const off = connection.subscribe((view) => {
      if (!test(view)) return;
      clearTimeout(timer);
      off();
      resolve(view);
    });
  });
}

const live = (v: HostConnectionView) => v.status === "live";

/**
 * For each mode: connect, check the path, `host.hello`, `workspace.list`,
 * events, disconnect, reconnect, disconnect. `mark` is told when each mode
 * starts and ends (the dev stack counts Cloud contacts in between).
 */
export async function runChecks(
  open: (mode: ConnectionMode) => Promise<HostConnection>,
  report: (result: CheckResult) => void,
  mark: (event: string) => Promise<void> = async () => {},
  connectTimeoutMs = 20_000,
): Promise<CheckResult[]> {
  const results: CheckResult[] = [];
  const check = async (step: string, run: () => Promise<string>) => {
    let result: CheckResult;
    try {
      result = { step, ok: true, detail: await run() };
    } catch (e) {
      const { code, message } = errorInfo(e);
      result = { step, ok: false, detail: `${code}: ${message}` };
    }
    results.push(result);
    report(result);
    return result.ok;
  };
  const plan: [ConnectionMode, "lan" | "relay"][] = [
    ["auto", "lan"],
    ["cloud", "relay"],
    ["local", "lan"],
  ];
  for (const [mode, path] of plan) {
    await mark(`${mode}:start`);
    const connection = await open(mode);
    try {
      connection.connect();
      const ok = await check(`${mode}: connect`, async () => {
        const v = await waitForView(connection, live, connectTimeoutMs);
        if (v.active !== path) throw new Error(`took ${v.active}, expected ${path}`);
        return `live over ${v.active}, host ${v.hostId}`;
      });
      if (!ok) continue;
      await check(`${mode}: host.hello`, async () => {
        const hello = await connection.request("host.hello", {});
        return `${hello.host_id} ${hello.protocol} ${hello.version}`;
      });
      await check(`${mode}: workspace.list`, async () => {
        const list = await connection.request("workspace.list", {});
        return `${list.length} workspace(s)`;
      });
      await check(`${mode}: events`, async () => {
        const v = await waitForView(connection, (v) => v.events > 0, 5_000);
        return `${v.events} feed update(s)`;
      });
      await check(`${mode}: disconnect`, async () => {
        connection.disconnect();
        const v = await waitForView(connection, (v) => v.status === "disconnected", 5_000);
        return `${v.status}/${v.active}`;
      });
      await check(`${mode}: reconnect`, async () => {
        connection.connect();
        const v = await waitForView(connection, live, connectTimeoutMs);
        if (v.active !== path) throw new Error(`took ${v.active}, expected ${path}`);
        const hello = await connection.request("host.hello", {});
        return `live over ${v.active}, hello ${hello.host_id}`;
      });
    } finally {
      connection.close();
      await mark(`${mode}:end`);
    }
  }
  return results;
}
