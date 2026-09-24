// Runs against a real host through the real protocol:
//   ZWIIM_IT_URL=http://127.0.0.1:38180 ZWIIM_IT_TOKEN=... ZWIIM_IT_DIR=/abs/dir npx vitest run test/integration.test.ts
// Skipped when ZWIIM_IT_URL is unset.

import { describe, expect, it } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient } from "../src/client/host-client";
import type { TerminalOutput } from "../src/state/terminal";
import { LocalTransport } from "../src/transport/local";

// The SDK is typed for browsers; only this Node-run test reads the environment.
declare const process: { env: Record<string, string | undefined> };

const url = process.env.ZWIIM_IT_URL;
const token = process.env.ZWIIM_IT_TOKEN ?? "";
const dir = process.env.ZWIIM_IT_DIR ?? "";
/** An existing idle session to exercise options on (optional). */
const sessionId = process.env.ZWIIM_IT_SESSION;

async function until(cond: () => boolean, ms = 10_000): Promise<void> {
  const end = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 25));
  }
}

describe.skipIf(!url)("integration: real host", () => {
  it("rejects a bad token as unauthenticated and stops", async () => {
    const client = new HostClient({ transport: new LocalTransport({ url: url!, token: "wrong" }) });
    client.start();
    await until(() => client.state.status === "unauthenticated");
    client.stop();
  });

  it("connects, lists, adds a workspace, reads files and git, and streams a terminal", async () => {
    const client = new HostClient({ transport: () => new LocalTransport({ url: url!, token }) });
    const c = new HostController(client);
    client.start();
    await until(() => c.store.getState().connection.status === "live");
    await until(() => c.store.getState().workspaces !== null);

    const ws = await c.addWorkspace(dir);
    const again = await c.addWorkspace(`${dir}/`);
    expect(again.workspace_id).toBe(ws.workspace_id);
    expect(JSON.stringify(ws)).not.toContain(dir);

    const tree = await client.request("workspace.tree", { workspace_id: ws.workspace_id, max_depth: 1 });
    expect(Array.isArray(tree)).toBe(true);
    await c.refreshSessions(ws.workspace_id);
    expect(c.store.getState().sessionsByWorkspace[ws.workspace_id]).toBeDefined();

    const outputs: TerminalOutput[] = [];
    c.onTerminalOutput((_, o) => outputs.push(o));
    const tid = await c.spawnTerminal(ws.workspace_id);
    expect(c.store.getState().terminals[tid]?.workspace_id).toBe(ws.workspace_id);
    const listed = await client.request("terminal.list", {});
    expect(listed.find((x) => x.terminal_id === tid)?.workspace_id).toBe(ws.workspace_id);
    c.attachTerminal(tid);
    await until(() => outputs.some((o) => o.kind === "reset"));
    await client.request("terminal.resize", { terminal_id: tid, cols: 100, rows: 30 });
    await client.request("terminal.write", { terminal_id: tid, data: "echo zwiim-$((40+2))\r" });
    const text = () =>
      outputs.map((o) => (o.kind === "append" || o.kind === "reset" ? o.data : "")).join("");
    await until(() => text().includes("zwiim-42"));
    await client.request("terminal.kill", { terminal_id: tid });
    await until(() => outputs.some((o) => o.kind === "exit"));
    expect(c.store.getState().terminals[tid]?.alive).toBe(false);

    c.dispose();
  }, 30_000);

  it("loads agents and the host owner", async () => {
    const client = new HostClient({ transport: () => new LocalTransport({ url: url!, token }) });
    const c = new HostController(client);
    client.start();
    await until(() => c.store.getState().agents !== null);
    const agents = c.store.getState().agents!;
    expect(agents.length).toBeGreaterThan(0);
    for (const a of agents) expect(a.available || a.reason !== null).toBe(true);
    expect(c.store.getState().host).toHaveProperty("owner");
    c.dispose();
  });

  it.skipIf(!sessionId)("reads and changes session options, kept across reconnect", async () => {
    const client = new HostClient({ transport: () => new LocalTransport({ url: url!, token }) });
    const c = new HostController(client);
    client.start();
    await until(() => c.store.getState().connection.status === "live");
    c.openSession(sessionId!);
    await until(() => c.store.getState().sessions[sessionId!]?.options != null, 60_000);
    const before = c.store.getState().sessions[sessionId!]!.options!;
    const effort = before.effort;
    if (effort) {
      const other = effort.values.find((v) => v.value !== effort.current)!.value;
      await c.setSessionOptions(sessionId!, { effort: other });
      expect(c.store.getState().sessions[sessionId!]!.options!.effort!.current).toBe(other);
      client.stop();
      client.start();
      await until(() => c.store.getState().connection.status === "live");
      await until(() => c.store.getState().sessions[sessionId!]?.options?.effort?.current === other);
      await c.setSessionOptions(sessionId!, { effort: effort.current });
    }
    const bad = await c.setSessionOptions(sessionId!, { model: "no-such-model" }).catch((e) => e);
    expect(bad.code).toBe("option_unavailable");
    c.dispose();
  }, 90_000);
});
