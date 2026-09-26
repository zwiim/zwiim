// The direct LAN path end to end, against the real binaries, beside the
// Cloud relay:
//
//   SDK (ConnectionManager) ─ mDNS discovery ─→ TCP → Noise tunnel → zwiim-host
//                           └─ zwiim-cloud (relay) → Noise tunnel ─┘
//
// The host is found the way another device on the network finds it: over
// real mDNS (the macOS `dns-sd` tool), dialing the non-loopback address it
// advertises. Build the binaries first (`cargo build -p zwiim-host -p
// zwiim-cloud`), then:
//
//   ZWIIM_LAN_IT=1 npx vitest run test/lan-integration.test.ts
//
// Skipped when ZWIIM_LAN_IT is unset. macOS only (`dns-sd`).

import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient, type HostFeedUpdate } from "../src/client/host-client";
import { CloudClient } from "../src/remote/cloud";
import { generateDeviceKeys } from "../src/remote/keys";
import { pairThroughCloud, parsePairingInvitation } from "../src/remote/pairing";
import type { ByteChannel } from "../src/remote/tunnel";
import type { SessionFrame } from "../src/state/session";
import type { TerminalOutput } from "../src/state/terminal";
import { ConnectionManager, type ConnectionMode } from "../src/transport/connection-manager";
import { LanTransport, type LanDial, type LanDiscover } from "../src/transport/lan";
import { LocalTransport } from "../src/transport/local";
import { RelayTransport } from "../src/transport/relay";
import type { KeyPairingInvitation, OpName, OpParams, OpResult } from "../src/types/protocol";
import { b64url, jwt, MOCK_AGENT, node, TARGET, until } from "./it-support";

// The SDK is typed for browsers; only this Node-run harness uses Node.
declare const process: { env: Record<string, string | undefined>; platform: string };

const enabled = !!process.env.ZWIIM_LAN_IT && process.platform === "darwin";
const TOKEN = "lan-it-token";
const USER = "lan-it-user";

interface Proc {
  kill(signal?: string): void;
  once(ev: "exit", fn: () => void): void;
  exitCode: number | null;
}

describe.skipIf(!enabled)("integration: SDK → LAN (mDNS + Noise) and Cloud relay → real host", () => {
  let fs: any, path: any, os: any, net: any, cp: any;
  let root = "";
  let cloudPort = 0;
  let hostPort = 0;
  let lanPort = 0;
  let cloudProc: Proc | null = null;
  let hostProc: Proc | null = null;
  let jwtSecret = "";
  let repo = "";

  // A TCP proxy in front of the Cloud: every Cloud contact (HTTP or relay)
  // is a connection through it, and cutting its sockets drops the relay.
  let proxy: any = null;
  let proxyPort = 0;
  let cloudContacts = 0;
  const cloudSockets = new Set<any>();

  // Every LAN connection this device opened.
  const lanSockets = new Set<any>();
  let lanDials: string[] = [];
  let discoveries = 0;

  async function freePort(host = "127.0.0.1"): Promise<number> {
    return new Promise((resolve) => {
      const srv = net.createServer();
      srv.listen(0, host, () => {
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
        ZWIIM_LAN_PORT: String(lanPort),
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

  // ─── the platform's LAN discovery and dialer, as a device would have ───

  /** Run `dns-sd` until `done(output)` or `ms`, then stop it. */
  function dnsSd(args: string[], done: (out: string) => boolean, ms: number): Promise<string> {
    return new Promise((resolve) => {
      const child = cp.spawn("dns-sd", args, { stdio: ["ignore", "pipe", "ignore"] });
      let out = "";
      const finish = () => {
        clearTimeout(timer);
        child.kill();
        resolve(out);
      };
      const timer = setTimeout(finish, ms);
      child.stdout.on("data", (b: Uint8Array) => {
        out += new TextDecoder().decode(b);
        if (done(out)) finish();
      });
    });
  }

  /** DNS-SD lookup of `<host_id>._zwiim._tcp.local.`, then its A records. */
  const discover: LanDiscover = async (hostId, { timeoutMs }) => {
    discoveries++;
    const reached = /can be reached at (\S+):(\d+)[^\n]*\n([^\n]*)\n/;
    const lookup = await dnsSd(["-L", hostId, "_zwiim._tcp", "local."], (o) => reached.test(o), timeoutMs);
    const m = reached.exec(lookup);
    if (!m) return [];
    const txt = Object.fromEntries(
      m[3]!
        .trim()
        .split(/\s+/)
        .map((kv) => [kv.slice(0, kv.indexOf("=")), kv.slice(kv.indexOf("=") + 1)]),
    );
    const v4 = /\s(\d+\.\d+\.\d+\.\d+)\s+\d+\s*$/gm;
    const found = await dnsSd(
      ["-G", "v4", m[1]!],
      (o) => [...o.matchAll(v4)].some((a) => !a[1]!.startsWith("127.")),
      timeoutMs,
    );
    const addresses = [...found.matchAll(v4)].map((a) => a[1]!);
    return [{ instance: hostId, port: Number(m[2]), addresses, txt }];
  };

  /** A TCP connection, as a byte channel. */
  function tcp(address: string, port: number, timeoutMs: number): Promise<ByteChannel> {
    return new Promise((resolve, reject) => {
      const socket = net.connect({ host: address, port });
      const timer = setTimeout(() => {
        socket.destroy();
        reject(new Error("connect timeout"));
      }, timeoutMs);
      let handlers: Parameters<ByteChannel["listen"]>[0] | null = null;
      const queue: Uint8Array[] = [];
      socket.on("data", (b: Uint8Array) => {
        const bytes = new Uint8Array(b);
        if (handlers) handlers.onData(bytes);
        else queue.push(bytes);
      });
      socket.on("error", () => {});
      socket.on("close", () => {
        clearTimeout(timer);
        lanSockets.delete(socket);
        handlers?.onClose({ code: 1006, reason: "" });
        reject(new Error("connect ECONNREFUSED"));
      });
      socket.on("connect", () => {
        clearTimeout(timer);
        socket.setNoDelay(true);
        lanSockets.add(socket);
        resolve({
          send: (bytes) => socket.write(bytes),
          close: () => socket.destroy(),
          listen: (h) => {
            handlers = h;
            for (const b of queue.splice(0)) h.onData(b);
          },
        });
      });
    });
  }

  /**
   * Dial the host's LAN address as another device on the network would:
   * never the loopback address it also advertises to this machine.
   */
  const dial: LanDial = async ({ address, port }, { timeoutMs }) => {
    lanDials.push(`${address}:${port}`);
    if (address.startsWith("127.")) throw new Error("loopback: not a LAN address");
    return tcp(address, port, timeoutMs);
  };

  // State shared by the steps below, which run in order.
  const device = generateDeviceKeys();
  let invitation: KeyPairingInvitation;
  let hostId = "";
  let workspaceId = "";

  interface Remote {
    manager: ConnectionManager;
    client: HostClient;
    controller: HostController;
    feed: HostFeedUpdate[];
    frames: [string, SessionFrame][];
    statuses: string[];
  }
  const remotes: Remote[] = [];

  function connect(mode: ConnectionMode, opts: { cloud?: boolean; pinned?: string } = {}): Remote {
    const manager = new ConnectionManager({
      mode,
      hostId,
      hostStaticPubkey: opts.pinned ?? invitation.host_static_pubkey,
      deviceSecret: device.secret,
      lan: { discover, dial },
      cloud: opts.cloud === false ? null : cloud(),
      lanDiscoverTimeoutMs: 3000,
      lanRetry: { baseMs: 200, maxMs: 1000 },
    });
    const client = new HostClient({ transport: manager, expectedHostId: hostId, backoff: { baseMs: 100, maxMs: 1000 } });
    const controller = new HostController(client);
    const r: Remote = { manager, client, controller, feed: [], frames: [], statuses: [] };
    client.subscribe({
      feed: (u) => r.feed.push(u),
      session: (id, f) => r.frames.push([id, f]),
      connection: (s) => {
        if (r.statuses[r.statuses.length - 1] !== s.status) r.statuses.push(s.status);
      },
    });
    remotes.push(r);
    client.start();
    return r;
  }

  function disconnect(r: Remote): void {
    r.controller.dispose();
    r.client.stop();
  }

  const isLive = (r: Remote) => r.client.state.status === "live";
  const epochs = (r: Remote) => r.feed.flatMap((u) => (u.type === "snapshot" ? [u.snapshot.epoch] : []));

  function relayTransport(secret = device.secret): RelayTransport {
    return new RelayTransport({
      cloud: cloud(),
      hostId,
      hostStaticPubkey: invitation.host_static_pubkey,
      deviceSecret: secret,
    });
  }

  function lanTransport(
    opts: { secret?: Uint8Array; pinned?: string; discover?: LanDiscover; dial?: LanDial } = {},
  ): LanTransport {
    return new LanTransport({
      hostId,
      hostStaticPubkey: opts.pinned ?? invitation.host_static_pubkey,
      deviceSecret: opts.secret ?? device.secret,
      discover: opts.discover ?? discover,
      dial: opts.dial ?? dial,
      discoverTimeoutMs: 3000,
    });
  }

  beforeAll(async () => {
    [fs, path, os, net, cp] = await Promise.all(
      ["node:fs", "node:path", "node:os", "node:net", "node:child_process"].map(node),
    );
    root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-lan-it-")));
    for (const d of ["bin", "home", "host", "cloud"]) fs.mkdirSync(path.join(root, d));
    fs.writeFileSync(path.join(root, "bin", "claude-agent-acp"), MOCK_AGENT, { mode: 0o755 });

    repo = path.join(root, "repo");
    fs.mkdirSync(repo);
    const git = (...args: string[]) => cp.execFileSync("git", ["-C", repo, ...args], { stdio: "ignore" });
    git("init", "-q", "-b", "main");
    fs.writeFileSync(path.join(repo, "README.md"), "lan integration\n");
    git("add", ".");
    git("-c", "user.name=it", "-c", "user.email=it@zwiim.test", "commit", "-q", "-m", "init");
    fs.writeFileSync(path.join(repo, "notes.txt"), "untracked\n");

    jwtSecret = b64url(String(Math.random()) + Date.now());
    [cloudPort, hostPort, proxyPort, lanPort] = [
      await freePort(),
      await freePort(),
      await freePort(),
      await freePort("0.0.0.0"),
    ];
    proxy = net.createServer((client: any) => {
      cloudContacts++;
      const upstream = net.connect(cloudPort, "127.0.0.1");
      cloudSockets.add(client);
      cloudSockets.add(upstream);
      client.pipe(upstream).pipe(client);
      const drop = () => {
        client.destroy();
        upstream.destroy();
        cloudSockets.delete(client);
        cloudSockets.delete(upstream);
      };
      client.on("error", drop).on("close", drop);
      upstream.on("error", drop).on("close", drop);
    });
    await new Promise<void>((r) => proxy.listen(proxyPort, "127.0.0.1", r));

    startCloud();
    startHost();
    await hostUp();
    await until(async () => {
      try {
        await cloud().hosts();
        return true;
      } catch {
        return false;
      }
    });

    // Local host authority: enroll, add a repo, pair this device (through
    // the Cloud, as R3 does), and turn LAN access on.
    const { enrollment_token } = await cloud().enrollmentToken();
    await local.request("cloud.enroll", {
      cloud_url: `http://127.0.0.1:${cloudPort}`,
      enrollment_token,
      name: "lan-it",
    });
    workspaceId = (await local.request("workspace.add", { path: repo })).workspace_id;
    await cloudOnline();
    invitation = await local.request("device.pair_invite", {});
    hostId = invitation.host_id;
    await pairThroughCloud({
      cloud: cloud(),
      invitation: parsePairingInvitation(JSON.stringify(invitation)),
      deviceSecret: device.secret,
      deviceId: "lan-it-device",
      deviceName: "LAN IT",
    });
    expect(await local.request("lan.status", {})).toMatchObject({ enabled: false, listening: false });
    const status = await local.request("lan.set", { enabled: true });
    expect(status).toMatchObject({ enabled: true, listening: true, advertised: true, port: lanPort });
  }, 120_000);

  afterAll(async () => {
    for (const r of remotes) disconnect(r);
    for (const s of [...cloudSockets, ...lanSockets]) s.destroy();
    proxy?.close();
    await stop(hostProc);
    await stop(cloudProc);
    if (root && !process.env.ZWIIM_LAN_IT_KEEP) fs.rmSync(root, { recursive: true, force: true });
  }, 60_000);

  it("advertises the host id, port and key only, over mDNS", async () => {
    const [ad] = (await discover(hostId, { timeoutMs: 5000 })) as any[];
    expect(ad).toBeDefined();
    expect(ad.port).toBe(lanPort);
    expect(ad.txt).toEqual({ v: "1", id: hostId, pk: invitation.host_static_pubkey });
    expect(ad.addresses.some((a: string) => !a.startsWith("127."))).toBe(true);
  }, 30_000);

  it("1. reaches the host over the loopback direct path", async () => {
    const loopback = lanTransport({
      discover: async () => [
        { instance: hostId, port: lanPort, addresses: ["127.0.0.1"], txt: { v: "1", id: hostId, pk: invitation.host_static_pubkey } },
      ],
      dial: ({ address, port }, { timeoutMs }) => tcp(address, port, timeoutMs),
    });
    expect(await loopback.request("host.hello", {})).toEqual(await local.request("host.hello", {}));
    loopback.close();
  });

  it("2. reaches the host at its advertised LAN address", async () => {
    const before = lanDials.length;
    const lan = lanTransport();
    expect(await lan.request("host.hello", {})).toEqual(await local.request("host.hello", {}));
    expect(await lan.request("workspace.list", {})).toEqual(await local.request("workspace.list", {}));
    const used = lanDials.slice(before).filter((d) => !d.startsWith("127."));
    expect(used).toHaveLength(1);
    expect(used[0]).toMatch(new RegExp(`:${lanPort}$`));
    lan.close();
  });

  it("3. the relay path is unchanged", async () => {
    const contacts = cloudContacts;
    const relay = relayTransport();
    expect(await relay.request("host.hello", {})).toEqual(await local.request("host.hello", {}));
    expect(cloudContacts).toBeGreaterThan(contacts);
    relay.close();
  });

  let auto: Remote;

  it("4. Automatic chooses the LAN, without contacting the Cloud", async () => {
    const contacts = cloudContacts;
    auto = connect("auto");
    await until(() => isLive(auto) && epochs(auto).length > 0);
    expect(auto.manager.path).toBe("lan");
    await until(() => auto.controller.store.getState().workspaces?.some((w) => w.workspace_id === workspaceId) ?? false);
    expect(cloudContacts).toBe(contacts);
  }, 30_000);

  const sessionEvents = (r: Remote, id: string) =>
    r.frames.flatMap(([sid, f]) =>
      sid !== id ? [] : f.type === "event" ? [f.event] : f.type === "replay" ? f.events : [],
    );
  const said = (r: Remote, id: string) =>
    sessionEvents(r, id)
      .filter((e) => e.type === "agent.message_delta")
      .map((e) => (e.payload as { text: string }).text)
      .join("");
  const noDuplicates = (r: Remote, id: string) => {
    const s = sessionEvents(r, id).map((e) => e.seq);
    expect(new Set(s).size).toBe(s.length);
    expect([...s].sort((a, b) => a - b)).toEqual(s);
  };
  let sessionId = "";

  it("10. gives the same workspace, files, Git, terminal and session results over the LAN and the relay", async () => {
    const contacts = cloudContacts;
    const relay = relayTransport();
    const lan = auto.client;
    const same = async <O extends OpName>(op: O, params: OpParams<O>) => {
      const viaLan = await lan.request(op, params);
      expect(viaLan).toEqual(await relay.request(op, params));
      expect(viaLan).toEqual(await local.request(op, params));
      const json = JSON.stringify(viaLan);
      expect(json).not.toContain(root);
      expect(json).not.toContain(TOKEN);
      return viaLan;
    };
    await same("host.hello", {});
    expect((await same("workspace.list", {})).map((w) => w.workspace_id)).toContain(workspaceId);
    const tree = await same("workspace.tree", { workspace_id: workspaceId, max_depth: 1 } as OpParams<"workspace.tree">);
    expect(JSON.stringify(tree)).toContain("README.md");
    expect(JSON.stringify(await same("files.read", { workspace_id: workspaceId, path: "README.md" }))).toContain(
      "lan integration",
    );
    expect(JSON.stringify(await same("git.status", { workspace_id: workspaceId }))).toContain("notes.txt");
    const missing = { workspace_id: workspaceId, path: "nope.txt" };
    const [le, re]: any[] = [
      await lan.request("files.read", missing).catch((e) => e),
      await relay.request("files.read", missing).catch((e) => e),
    ];
    expect([le.code, le.status]).toEqual([re.code, re.status]);
    relay.close();
    const relayContacts = cloudContacts - contacts;

    // A terminal over the LAN.
    const outputs: TerminalOutput[] = [];
    const off = auto.controller.onTerminalOutput((_, o) => outputs.push(o));
    const tid = await auto.controller.spawnTerminal(workspaceId);
    auto.controller.attachTerminal(tid);
    await until(() => outputs.some((o) => o.kind === "reset"));
    await auto.client.request("terminal.write", { terminal_id: tid, data: "echo lan-$((40+2))\r" });
    const shown = () => outputs.map((o) => (o.kind === "append" || o.kind === "reset" ? o.data : "")).join("");
    await until(() => shown().includes("lan-42"));
    await auto.client.request("terminal.kill", { terminal_id: tid });
    await until(() => outputs.some((o) => o.kind === "exit"));
    off();

    // A session over the LAN; its history is the same through the relay.
    sessionId = await auto.controller.startSession({
      workspace_id: workspaceId,
      agent: "claude_code",
      blocks: [{ type: "text", text: "hello over the lan" }],
    });
    await until(() => said(auto, sessionId).includes("echo: hello over the lan"));
    await until(() => sessionEvents(auto, sessionId).some((e) => e.type === "agent.turn_complete"));
    const history = await auto.client.request("session.history", { session_id: sessionId });
    expect(JSON.stringify(history)).toContain("echo: hello over the lan");
    const relay2 = relayTransport();
    expect(history).toEqual(await relay2.request("session.history", { session_id: sessionId }));
    relay2.close();
    // Only the explicit relay comparisons contacted the Cloud.
    expect(cloudContacts - contacts).toBeGreaterThanOrEqual(relayContacts);
    expect(auto.manager.path).toBe("lan");
  }, 60_000);

  it("reconnects over the LAN after a LAN drop, without duplicates", async () => {
    const contacts = cloudContacts;
    const snapshots = epochs(auto).length;
    for (const s of lanSockets) s.destroy();
    await until(() => !isLive(auto));
    await until(() => isLive(auto) && epochs(auto).length > snapshots, 30_000);
    expect(auto.manager.path).toBe("lan");
    expect(cloudContacts).toBe(contacts);
    await auto.controller.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "after the drop" }] });
    await until(() => said(auto, sessionId).includes("echo: after the drop"));
    noDuplicates(auto, sessionId);
  }, 60_000);

  it("5. Automatic falls back to the relay when the LAN goes away, without looking offline", async () => {
    auto.statuses.length = 0;
    await local.request("lan.set", { enabled: false });
    await until(() => isLive(auto) && auto.manager.path === "relay", 30_000);
    expect(auto.statuses).not.toContain("error");
    expect(auto.statuses).not.toContain("unauthenticated");
    expect(auto.statuses).not.toContain("disconnected");
    expect(lanSockets.size).toBe(0);
    await auto.controller.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "over the relay" }] });
    await until(() => said(auto, sessionId).includes("echo: over the relay"));
    noDuplicates(auto, sessionId);
  }, 60_000);

  it("stays on a healthy relay when the LAN returns, and takes the LAN at the next reconnect", async () => {
    await local.request("lan.set", { enabled: true });
    await until(async () => (await discover(hostId, { timeoutMs: 3000 })).length > 0, 30_000);
    await new Promise((r) => setTimeout(r, 1500));
    expect(auto.manager.path).toBe("relay");
    expect(isLive(auto)).toBe(true);
    // The relay drops (as on a network change): the LAN is preferred again.
    for (const s of cloudSockets) s.destroy();
    await until(() => isLive(auto) && auto.manager.path === "lan", 30_000);
    await auto.controller.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "back on the lan" }] });
    await until(() => said(auto, sessionId).includes("echo: back on the lan"));
    noDuplicates(auto, sessionId);
  }, 60_000);

  it("7. Cloud only never looks on the LAN", async () => {
    const [d, n] = [discoveries, lanDials.length];
    const r = connect("cloud");
    await until(() => isLive(r) && epochs(r).length > 0, 30_000);
    expect(r.manager.path).toBe("relay");
    expect(await r.client.request("workspace.list", {})).toEqual(await local.request("workspace.list", {}));
    expect([discoveries, lanDials.length]).toEqual([d, n]);
    disconnect(r);
  }, 30_000);

  let localOnly: Remote;

  it("6. Local only fails without the Cloud when the host is not reachable locally", async () => {
    await local.request("lan.set", { enabled: false });
    await until(() => auto.manager.path === "relay" && isLive(auto), 30_000);
    const contacts = cloudContacts;
    localOnly = connect("local", { cloud: false });
    await until(() => localOnly.client.state.error?.code === "lan_unavailable", 30_000);
    expect(localOnly.client.state.status).toBe("reconnecting");
    await local.request("lan.set", { enabled: true });
    await until(() => isLive(localOnly) && epochs(localOnly).length > 0, 30_000);
    expect(localOnly.manager.path).toBe("lan");
    expect(cloudContacts).toBe(contacts);
  }, 60_000);

  it("recovers a host restart over the LAN through a new epoch", async () => {
    const contacts = cloudContacts;
    const before = epochs(localOnly).at(-1);
    await stop(hostProc);
    startHost();
    await hostUp();
    // LAN access stays on across the restart.
    expect(await local.request("lan.status", {})).toMatchObject({ enabled: true });
    await until(() => isLive(localOnly) && epochs(localOnly).at(-1) !== before, 60_000);
    expect(localOnly.manager.path).toBe("lan");
    expect(localOnly.controller.store.getState().workspaces?.some((w) => w.workspace_id === workspaceId)).toBe(true);
    const history = await localOnly.client.request("session.history", { session_id: sessionId });
    expect(JSON.stringify(history)).toContain("echo: back on the lan");
    expect(cloudContacts).toBe(contacts);
    await cloudOnline();
    await until(() => isLive(auto), 60_000);
  }, 120_000);

  it("9. refuses a host whose key is not the pinned one", async () => {
    const wrong = generateDeviceKeys().publicKey;
    const [n, contacts] = [lanDials.length, cloudContacts];
    // Discovery shows the real host, whose key is not the one pinned.
    expect(await lanTransport({ pinned: wrong }).request("host.hello", {}).catch((e) => e)).toMatchObject({
      code: "host_identity_mismatch",
    });
    expect(lanDials.length).toBe(n);
    // An advertisement forged to match the wrong pin: the host there cannot
    // complete that handshake, and nothing is exchanged.
    const forged = lanTransport({
      pinned: wrong,
      discover: async (id, o) =>
        ((await discover(id, o)) as any[]).map((a) => ({ ...a, txt: { ...a.txt, pk: wrong } })),
    });
    expect(await forged.request("host.hello", {}).catch((e) => e)).toMatchObject({ code: "lan_unavailable" });
    // Automatic does not hide the mismatch behind the relay.
    const r = connect("auto", { pinned: wrong });
    await until(() => r.client.state.status === "error", 30_000);
    expect(r.client.state.error?.code).toBe("host_identity_mismatch");
    expect(cloudContacts).toBe(contacts);
    disconnect(r);
  }, 60_000);

  it("8. refuses unknown and revoked devices on both paths", async () => {
    const stranger = generateDeviceKeys();
    expect(await lanTransport({ secret: stranger.secret }).request("host.hello", {}).catch((e) => e)).toMatchObject({
      code: "device_refused",
    });
    expect(await relayTransport(stranger.secret).request("host.hello", {}).catch((e) => e)).toMatchObject({
      code: "device_refused",
    });

    const viaRelay = connect("cloud");
    await until(() => isLive(viaRelay) && isLive(auto) && isLive(localOnly), 30_000);
    expect(auto.manager.path).toBe("lan");
    await local.request("device.revoke", { device_id: "lan-it-device" });
    for (const r of [auto, localOnly, viaRelay]) {
      await until(() => r.client.state.status === "unauthenticated", 30_000);
      expect(r.client.state.error?.code).toBe("device_refused");
    }
    expect(await lanTransport().request("host.hello", {}).catch((e) => e)).toMatchObject({ code: "device_refused" });
    expect(await relayTransport().request("host.hello", {}).catch((e) => e)).toMatchObject({ code: "device_refused" });
  }, 60_000);
});
