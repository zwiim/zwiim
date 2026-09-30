// The remote path end to end, against the real binaries:
//
//   SDK (RelayTransport) → zwiim-cloud (relay) → Noise tunnel → zwiim-host
//
// Starts its own Cloud (plain HTTP on loopback) and host (fresh data dir,
// file key store), with a scripted ACP agent standing in for Claude Code so
// sessions are deterministic. Build the binaries first (`cargo build -p
// zwiim-host -p zwiim-cloud`), then:
//
//   ZWIIM_RELAY_IT=1 npx vitest run test/relay-integration.test.ts
//
// Skipped when ZWIIM_RELAY_IT is unset.

import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient, type HostFeedUpdate } from "../src/client/host-client";
import { CloudClient } from "../src/remote/cloud";
import { generateDeviceKeys } from "../src/remote/keys";
import { pairThroughCloud, parsePairingInvitation } from "../src/remote/pairing";
import type { SessionFrame } from "../src/state/session";
import type { TerminalOutput } from "../src/state/terminal";
import { LocalTransport } from "../src/transport/local";
import { RelayTransport } from "../src/transport/relay";
import type { KeyPairingInvitation, OpName, OpParams, OpResult } from "../src/types/protocol";
import { b64url, jwt, MOCK_AGENT, node, TARGET, until } from "./it-support";

// The SDK is typed for browsers; only this Node-run harness uses Node.
declare const process: { env: Record<string, string | undefined> };

const enabled = !!process.env.ZWIIM_RELAY_IT;
const TOKEN = "relay-it-token";
const USER = "relay-it-user";

interface Proc {
  kill(signal?: string): void;
  once(ev: "exit", fn: () => void): void;
  exitCode: number | null;
}

describe.skipIf(!enabled)("integration: SDK → Cloud relay → Noise → real host", () => {
  let fs: any, path: any, os: any, net: any, cp: any;
  let root = "";
  let cloudPort = 0;
  let hostPort = 0;
  let cloudProc: Proc | null = null;
  let hostProc: Proc | null = null;
  let jwtSecret = "";
  let repo = "";

  // A TCP proxy between the device and the Cloud: cut to drop the relay,
  // and a record of what the Cloud sent the device.
  let proxy: any = null;
  let proxyPort = 0;
  const proxied: Uint8Array[] = [];
  const sockets = new Set<any>();

  async function freePort(): Promise<number> {
    return new Promise((resolve) => {
      const srv = net.createServer();
      srv.listen(0, "127.0.0.1", () => {
        const { port } = srv.address();
        srv.close(() => resolve(port));
      });
    });
  }

  function spawn(bin: string, env: Record<string, string>, log: string): Proc {
    const out = fs.openSync(path.join(root, log), "a");
    return cp.spawn(path.join(TARGET, bin), [], { env, stdio: ["ignore", out, out] });
  }

  async function stop(p: Proc | null): Promise<void> {
    if (!p || p.exitCode !== null) return;
    await new Promise<void>((resolve) => {
      p.once("exit", () => resolve());
      p.kill("SIGTERM");
    });
  }

  function startCloud(): void {
    cloudProc = spawn(
      "zwiim-cloud",
      {
        PATH: process.env.PATH ?? "",
        ZWIIM_CLOUD_ADDR: `127.0.0.1:${cloudPort}`,
        ZWIIM_CLOUD_PUBLIC_URL: `http://127.0.0.1:${cloudPort}`,
        ZWIIM_CLOUD_DATA_DIR: path.join(root, "cloud"),
        ZWIIM_CLOUD_JWT_SECRET: jwtSecret,
        RUST_LOG: "info",
      },
      "cloud.log",
    );
  }

  function startHost(): void {
    hostProc = spawn(
      "zwiim-host",
      {
        PATH: `${path.join(root, "bin")}:${process.env.PATH ?? ""}`,
        HOME: path.join(root, "home"),
        ZWIIM_DATA_DIR: path.join(root, "host"),
        ZWIIM_PORT: String(hostPort),
        ZWIIM_HOST: "127.0.0.1",
        ZWIIM_TOKEN: TOKEN,
        ZWIIM_HOST_KEY_STORE: "file",
        // The mock agent is on PATH, not a managed install.
        ZWIIM_ADAPTER_SOURCE: "path",
        RUST_LOG: "info",
      },
      "host.log",
    );
  }

  const hostUrl = () => `http://127.0.0.1:${hostPort}`;
  const local = new (class {
    request<O extends OpName>(op: O, params: OpParams<O>): Promise<OpResult<O>> {
      return new LocalTransport({ url: hostUrl(), token: TOKEN }).request(op, params) as Promise<OpResult<O>>;
    }
  })();

  async function hostUp(): Promise<void> {
    await until(async () => {
      try {
        await local.request("host.hello", {});
        return true;
      } catch {
        return false;
      }
    });
  }

  function cloud(): CloudClient {
    return new CloudClient({ url: `http://127.0.0.1:${proxyPort}`, accessToken: jwt(jwtSecret, USER) });
  }

  const cloudOnline = () =>
    until(async () => {
      try {
        return (await cloud().hosts()).some((h) => h.online);
      } catch {
        return false;
      }
    }, 60_000);

  // State shared by the steps below, which run in order.
  const device = generateDeviceKeys();
  let invitation: KeyPairingInvitation;
  let workspaceId = "";
  let remote: HostClient;
  let controller: HostController;
  const feed: HostFeedUpdate[] = [];
  const sessionFrames: [string, SessionFrame][] = [];

  beforeAll(async () => {
    [fs, path, os, net, cp] = await Promise.all(
      ["node:fs", "node:path", "node:os", "node:net", "node:child_process"].map(node),
    );
    root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-relay-it-")));
    for (const d of ["bin", "home", "host", "cloud"]) fs.mkdirSync(path.join(root, d));
    fs.writeFileSync(path.join(root, "bin", "claude-agent-acp"), MOCK_AGENT, { mode: 0o755 });

    repo = path.join(root, "repo");
    fs.mkdirSync(repo);
    const git = (...args: string[]) => cp.execFileSync("git", ["-C", repo, ...args], { stdio: "ignore" });
    git("init", "-q", "-b", "main");
    fs.writeFileSync(path.join(repo, "README.md"), "relay integration\n");
    git("add", ".");
    git("-c", "user.name=it", "-c", "user.email=it@zwiim.test", "commit", "-q", "-m", "init");
    fs.writeFileSync(path.join(repo, "notes.txt"), "untracked\n");

    jwtSecret = b64url(String(Math.random()) + Date.now());
    [cloudPort, hostPort, proxyPort] = [await freePort(), await freePort(), await freePort()];
    proxy = net.createServer((client: any) => {
      const upstream = net.connect(cloudPort, "127.0.0.1");
      sockets.add(client);
      sockets.add(upstream);
      upstream.on("data", (b: Uint8Array) => proxied.push(new Uint8Array(b)));
      client.pipe(upstream).pipe(client);
      const drop = () => {
        client.destroy();
        upstream.destroy();
        sockets.delete(client);
        sockets.delete(upstream);
      };
      client.on("error", drop).on("close", drop);
      upstream.on("error", drop).on("close", drop);
    });
    await new Promise<void>((r) => proxy.listen(proxyPort, "127.0.0.1", r));

    startCloud();
    startHost();
    await hostUp();
    // The Cloud comes up alongside; wait until it answers.
    await until(async () => {
      try {
        await cloud().hosts();
        return true;
      } catch {
        return false;
      }
    });

    // Local host authority: enroll into the account, pair this device, add a repo.
    const { enrollment_token } = await cloud().enrollmentToken();
    await local.request("cloud.enroll", {
      cloud_url: `http://127.0.0.1:${cloudPort}`,
      enrollment_token,
      name: "relay-it",
    });
    workspaceId = (await local.request("workspace.add", { path: repo })).workspace_id;
    await cloudOnline();
    // The QR shown on the host, redeemed remotely: pairing crosses the relay.
    invitation = await local.request("device.pair_invite", {});
    await pairThroughCloud({
      cloud: cloud(),
      invitation: parsePairingInvitation(JSON.stringify(invitation)),
      deviceSecret: device.secret,
      deviceId: "relay-it-device",
      deviceName: "Relay IT",
    });
  }, 120_000);

  afterAll(async () => {
    controller?.dispose();
    remote?.stop();
    for (const s of sockets) s.destroy();
    proxy?.close();
    await stop(hostProc);
    await stop(cloudProc);
    if (root && !process.env.ZWIIM_RELAY_IT_KEEP) fs.rmSync(root, { recursive: true, force: true });
  }, 60_000);

  it("paired the device through the relay, once", async () => {
    const { devices } = await local.request("device.list", {});
    expect(devices).toEqual([
      expect.objectContaining({
        device_id: "relay-it-device",
        device_name: "Relay IT",
        status: "trusted",
        device_pubkey: device.publicKey,
      }),
    ]);
    // The code is spent: replaying it from another device is refused.
    const other = generateDeviceKeys();
    await expect(
      pairThroughCloud({ cloud: cloud(), invitation, deviceSecret: other.secret, deviceId: "relay-it-other" }),
    ).rejects.toMatchObject({ code: "pairing_rejected" });
    expect((await local.request("device.list", {})).devices).toHaveLength(1);
  });

  it("discovers the host through the Cloud, keyed as pairing pinned it", async () => {
    const hosts = await cloud().hosts();
    expect(hosts).toHaveLength(1);
    expect(hosts[0]).toMatchObject({ host_id: invitation.host_id, online: true, name: "relay-it" });
    expect(hosts[0]!.static_pubkey).toBe(invitation.host_static_pubkey);

    // Another account sees nothing and cannot get a ticket.
    const stranger = new CloudClient({ url: `http://127.0.0.1:${proxyPort}`, accessToken: jwt(jwtSecret, "someone-else") });
    expect(await stranger.hosts()).toEqual([]);
    await expect(stranger.ticket(invitation.host_id)).rejects.toMatchObject({ code: "host_unknown" });
  });

  it("goes live and gives the same results as the local transport", async () => {
    // Host selection: the SDK only needs the host id, and the pinned key.
    const [host] = await cloud().hosts();
    const transport = new RelayTransport({
      cloud: cloud(),
      hostId: host!.host_id,
      hostStaticPubkey: invitation.host_static_pubkey,
      deviceSecret: device.secret,
    });
    remote = new HostClient({ transport, backoff: { baseMs: 100, maxMs: 1000 } });
    controller = new HostController(remote);
    remote.subscribe({ feed: (u) => feed.push(u), session: (id, f) => sessionFrames.push([id, f]) });
    remote.start();
    await until(() => controller.store.getState().connection.status === "live");
    await until(() => controller.store.getState().workspaces?.some((w) => w.workspace_id === workspaceId) ?? false);

    const same = async <O extends OpName>(op: O, params: OpParams<O>) => {
      const [viaRelay, viaLocal] = [await remote.request(op, params), await local.request(op, params)];
      expect(viaRelay).toEqual(viaLocal);
      // No host-private path or credential in what the remote client sees.
      const json = JSON.stringify(viaRelay);
      expect(json).not.toContain(root);
      expect(json).not.toContain(TOKEN);
      return viaRelay;
    };
    const hello = await same("host.hello", {});
    expect(hello.host_id).toBe(invitation.host_id);
    const list = await same("workspace.list", {});
    expect(list.map((w) => w.workspace_id)).toContain(workspaceId);
    const tree = await same("workspace.tree", { workspace_id: workspaceId, max_depth: 1 } as OpParams<"workspace.tree">);
    expect(JSON.stringify(tree)).toContain("README.md");
    const file = await same("files.read", { workspace_id: workspaceId, path: "README.md" });
    expect(JSON.stringify(file)).toContain("relay integration");
    const status = await same("git.status", { workspace_id: workspaceId });
    expect(JSON.stringify(status)).toContain("notes.txt");

    // Errors come back the same way too.
    const missing = { workspace_id: workspaceId, path: "nope.txt" };
    const [re, le] = [
      await remote.request("files.read", missing).catch((e) => e),
      await local.request("files.read", missing).catch((e) => e),
    ];
    expect([re.code, re.status]).toEqual([le.code, le.status]);
  }, 60_000);

  it("runs a terminal remotely", async () => {
    const outputs: TerminalOutput[] = [];
    const off = controller.onTerminalOutput((_, o) => outputs.push(o));
    const tid = await controller.spawnTerminal(workspaceId);
    controller.attachTerminal(tid);
    await until(() => outputs.some((o) => o.kind === "reset"));
    await remote.request("terminal.write", { terminal_id: tid, data: "echo relay-$((40+2))\r" });
    const shown = () => outputs.map((o) => (o.kind === "append" || o.kind === "reset" ? o.data : "")).join("");
    await until(() => shown().includes("relay-42"));
    await remote.request("terminal.kill", { terminal_id: tid });
    await until(() => outputs.some((o) => o.kind === "exit"));
    off();
  }, 30_000);

  let sessionId = "";
  const deltas = () =>
    sessionFrames.flatMap(([id, f]) =>
      id !== sessionId ? [] : f.type === "event" ? [f.event] : f.type === "replay" ? f.events : [],
    );
  const said = () =>
    deltas()
      .filter((e) => e.type === "agent.message_delta")
      .map((e) => (e.payload as { text: string }).text)
      .join("");

  it("starts a session, streams its events, sends again, and reads history", async () => {
    sessionId = await controller.startSession({
      workspace_id: workspaceId,
      agent: "claude_code",
      blocks: [{ type: "text", text: "hello over the relay" }],
    });
    expect(sessionId).toMatch(/^[0-9a-f-]{36}$/);
    await until(() => said().includes("echo: hello over the relay"));
    await until(() => deltas().some((e) => e.type === "agent.turn_complete"));

    await controller.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "second" }] });
    await until(() => said().includes("echo: second"));
    await until(() => deltas().filter((e) => e.type === "agent.turn_complete").length === 2);

    const history = await remote.request("session.history", { session_id: sessionId });
    const json = JSON.stringify(history);
    expect(json).toContain("hello over the relay");
    expect(json).toContain("echo: second");
    expect(history).toEqual(await local.request("session.history", { session_id: sessionId }));
    expect(json).not.toContain(root);
    // The agent's own session id (its transcript name) stays on the host.
    const [transcript] = fs.readdirSync(path.join(root, "home", ".claude", "projects", "it"));
    const acpId = String(transcript).replace(/\.jsonl$/, "");
    expect(acpId).not.toBe(sessionId);
    expect(json).not.toContain(acpId);
    expect(JSON.stringify(sessionFrames)).not.toContain(acpId);
  }, 60_000);

  const seqs = () => deltas().map((e) => e.seq);
  const noDuplicates = () => {
    const s = seqs();
    expect(new Set(s).size).toBe(s.length);
    expect([...s].sort((a, b) => a - b)).toEqual(s);
  };

  it("recovers a relay cut without duplicate events", async () => {
    const tunnelsBefore = proxied.length;
    for (const s of sockets) s.destroy();
    await until(() => controller.store.getState().connection.status !== "live");
    await until(() => controller.store.getState().connection.status === "live", 30_000);
    expect(proxied.length).toBeGreaterThan(tunnelsBefore);

    await controller.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "after the cut" }] });
    await until(() => said().includes("echo: after the cut"));
    noDuplicates();
  }, 60_000);

  it("rides out a Cloud outage", async () => {
    const workspacesBefore = controller.store.getState().workspaces;
    await stop(cloudProc);
    await until(() => controller.store.getState().connection.status === "reconnecting", 30_000);
    // Local work on the host is unaffected, and the remote state is kept.
    expect((await local.request("workspace.list", {})).length).toBeGreaterThan(0);
    expect(controller.store.getState().workspaces).toEqual(workspacesBefore);
    startCloud();
    await cloudOnline();
    await until(() => controller.store.getState().connection.status === "live", 60_000);
    await controller.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "after the outage" }] });
    await until(() => said().includes("echo: after the outage"));
    noDuplicates();
  }, 120_000);

  it("recovers a host restart through a new epoch", async () => {
    const epochs = () => feed.flatMap((u) => (u.type === "snapshot" ? [u.snapshot.epoch] : []));
    const before = epochs()[epochs().length - 1];
    await stop(hostProc);
    startHost();
    await hostUp();
    await cloudOnline();
    await until(() => controller.store.getState().connection.status === "live" && epochs().at(-1) !== before, 60_000);
    expect(controller.store.getState().workspaces?.some((w) => w.workspace_id === workspaceId)).toBe(true);
    // The session is reachable again, from a fresh baseline.
    const history = await remote.request("session.history", { session_id: sessionId });
    expect(JSON.stringify(history)).toContain("echo: after the outage");
  }, 120_000);

  it("keeps Zwiim payloads out of the Cloud's reach", () => {
    // Everything the Cloud relayed to the device, raw: no protocol content.
    let bytes = "";
    for (const b of proxied) bytes += new TextDecoder("latin1").decode(b);
    expect(bytes.length).toBeGreaterThan(10_000);
    for (const secret of ["echo: hello over the relay", "relay-42", "relay integration", workspaceId, sessionId]) {
      expect(bytes).not.toContain(secret);
    }
  });

  it("stops for good once the host revokes the device", async () => {
    await local.request("device.revoke", { device_id: "relay-it-device" });
    await until(() => controller.store.getState().connection.status === "unauthenticated", 30_000);
    expect(remote.state.error?.code).toBe("device_refused");
  }, 60_000);
});
