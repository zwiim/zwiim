// @vitest-environment node
//
// The Desktop's remote path end to end, against the real binaries and, by
// default, the real Internet:
//
//   Desktop remote code (lib/remote) → HTTPS/WSS → Cloudflare quick tunnel
//     → zwiim-cloud (relay) → Noise tunnel → zwiim-host
//
// The Cloud and the tunnel are started with the development deployment
// scripts (deploy/dev/run-cloud.sh, deploy/dev/tunnel.sh), configured as a
// deployment is, and verify account tokens against a Supabase-style JWKS
// endpoint (a local stand-in signing ES256 tokens; no real project or
// credentials are involved). Both the device and the host reach the Cloud
// only through its public tunnel URL, as they would a hosted Cloud. The
// native shell's part (the account token on Cloud calls, the keychain) is
// stood in for by `shell` below, which does what `cloud.rs` / `device.rs` do.
// Needs `cloudflared` on PATH; `ZWIIM_REMOTE_E2E_TUNNEL=0` runs the same
// steps on loopback instead. `ZWIIM_REMOTE_E2E_REAL_AGENT=1` also runs one
// short turn with the real Claude Code agent (`claude-agent-acp` on PATH,
// signed in). Build the binaries first
// (`cargo build -p zwiim-host -p zwiim-cloud`), then:
//
//   ZWIIM_REMOTE_E2E=1 npx vitest run test/remote-e2e.test.ts
//
// Skipped when ZWIIM_REMOTE_E2E is unset.

import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  encodeKey,
  generateDeviceKeys,
  LocalTransport,
  type HostController,
  type KeyPairingInvitation,
  type OpName,
  type OpParams,
  type OpResult,
  type SessionFrame,
  type TerminalOutput,
} from "@zwiim/sdk";
import type { CloudReply, DeviceKey, PairedHost, RemoteBridge } from "@/lib/bridge";
import { connectRemote, listCloudHosts, pairRemote } from "@/lib/remote";

declare const process: { env: Record<string, string | undefined> };
const node = (m: string): Promise<any> => import(/* @vite-ignore */ m);

const enabled = !!process.env.ZWIIM_REMOTE_E2E;
const tunnelled = process.env.ZWIIM_REMOTE_E2E_TUNNEL !== "0";
const realAgent = !!process.env.ZWIIM_REMOTE_E2E_REAL_AGENT;
const TARGET = new URL("../../../target/debug/", import.meta.url).pathname;
const DEPLOY = new URL("../../../deploy/dev/", import.meta.url).pathname;
const TOKEN = "r5-e2e-token";
const USER = "r5-e2e-user";

async function until(cond: () => boolean | Promise<boolean>, ms = 30_000): Promise<void> {
  const end = Date.now() + ms;
  while (!(await cond())) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await new Promise((r) => setTimeout(r, 100));
  }
}

const text = (s: string) => new TextEncoder().encode(s);

/** Every account token issued, to look for later where none may be. */
const issued: string[] = [];

// The stand-in Supabase project: its signing key, whose public half it
// publishes as JWKS, and its URL.
const KID = "e2e-signing-key";
let signing: CryptoKeyPair;
let supabaseUrl = "";
let jwksFetches = 0;
const newSigningKey = () =>
  crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]) as Promise<CryptoKeyPair>;

/** An ES256 account token, as the Supabase project issues. */
async function jwt(sub: string, key: CryptoKey = signing.privateKey): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  const claims = { sub, iss: `${supabaseUrl}/auth/v1`, aud: "authenticated", role: "authenticated", iat: now, exp: now + 3600 };
  const body = `${encodeKey(text(JSON.stringify({ alg: "ES256", typ: "JWT", kid: KID })))}.${encodeKey(text(JSON.stringify(claims)))}`;
  const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, key, text(body));
  const token = `${body}.${encodeKey(new Uint8Array(sig))}`;
  issued.push(token);
  return token;
}

// A scripted ACP agent standing in for Claude Code: answers each prompt
// with "echo: <prompt>" and records the turn where the host reads history.
const MOCK_AGENT = String.raw`#!/usr/bin/env node
const fs = require("fs"), path = require("path"), readline = require("readline");
const dir = path.join(process.env.CLAUDE_CONFIG_DIR || path.join(process.env.HOME, ".claude"), "projects", "r5-e2e");
fs.mkdirSync(dir, { recursive: true });
const cwds = new Map();
const send = (m) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...m }) + "\n");
const line = (sid, o) => fs.appendFileSync(path.join(dir, sid + ".jsonl"), JSON.stringify(o) + "\n");
readline.createInterface({ input: process.stdin }).on("line", (raw) => {
  let msg;
  try { msg = JSON.parse(raw); } catch { return; }
  if (msg.id === undefined || !msg.method) return;
  const p = msg.params || {};
  switch (msg.method) {
    case "initialize":
      return send({ id: msg.id, result: { protocolVersion: 1, agentCapabilities: {}, authMethods: [] } });
    case "session/new": {
      const sessionId = crypto.randomUUID();
      cwds.set(sessionId, p.cwd);
      return send({ id: msg.id, result: { sessionId } });
    }
    case "session/prompt": {
      const prompt = (p.prompt || []).filter((b) => b.type === "text").map((b) => b.text).join("");
      const reply = "echo: " + prompt;
      const cwd = cwds.get(p.sessionId);
      line(p.sessionId, { type: "user", timestamp: new Date().toISOString(), uuid: crypto.randomUUID(), sessionId: p.sessionId, cwd,
        message: { role: "user", content: [{ type: "text", text: prompt }] } });
      line(p.sessionId, { type: "assistant", timestamp: new Date().toISOString(), uuid: crypto.randomUUID(), sessionId: p.sessionId, cwd,
        message: { id: "msg_" + Date.now(), role: "assistant", model: "mock", content: [{ type: "text", text: reply }] } });
      send({ method: "session/update", params: { sessionId: p.sessionId,
        update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: reply } } } });
      setTimeout(() => send({ id: msg.id, result: { stopReason: "end_turn" } }), 100);
      return;
    }
    default:
      return send({ id: msg.id, result: {} });
  }
});
`;

interface Proc {
  pid?: number;
  kill(signal?: string): void;
  once(ev: "exit", fn: () => void): void;
  exitCode: number | null;
}

describe.skipIf(!enabled)("e2e: Desktop → Internet → Cloud relay → real host", () => {
  let fs: any, path: any, os: any, net: any, cp: any;
  let root = "";
  let cloudPort = 0;
  let hostPort = 0;
  let proxyPort = 0;
  let cloudProc: Proc | null = null;
  let hostProc: Proc | null = null;
  let tunnelProc: Proc | null = null;
  let supabase: any = null;
  let repo = "";
  /** The Cloud's public origin: the tunnel's https URL, or loopback. */
  let cloudUrl = "";

  // Between the tunnel and the Cloud: everything the Cloud receives and
  // sends, raw, and a way to cut every relay link at once.
  let proxy: any = null;
  const seen: Uint8Array[] = [];
  const sockets = new Set<any>();

  // What the native shell holds: the account token (never given to the UI)
  // and the keychain's device identity and pins.
  let accountToken: string | null = null;
  const deviceKeys = generateDeviceKeys();
  const identity: DeviceKey = {
    device_id: "desktop-e2e0123456789ab",
    device_name: "E2E Desktop",
    secret: encodeKey(deviceKeys.secret),
  };
  let pins: PairedHost[] = [];
  const shell: RemoteBridge = {
    async cloudRequest(method, p): Promise<CloudReply> {
      if (!accountToken) {
        return { status: 401, body: { error: { code: "unauthenticated", message: "Sign in again" } } };
      }
      const res = await fetch(`${cloudUrl}${p}`, { method, headers: { authorization: `Bearer ${accountToken}` } });
      return { status: res.status, body: await res.json().catch(() => null) };
    },
    deviceIdentity: async () => identity,
    pairedHosts: async () => pins.map((h) => ({ ...h })),
    pinHost: async (h) => void (pins = [...pins.filter((x) => x.host_id !== h.host_id), h]),
    forgetHost: async (id) => void (pins = pins.filter((x) => x.host_id !== id)),
  };

  async function freePort(): Promise<number> {
    return new Promise((resolve) => {
      const srv = net.createServer();
      srv.listen(0, "127.0.0.1", () => {
        const { port } = srv.address();
        srv.close(() => resolve(port));
      });
    });
  }

  function spawn(bin: string, args: string[], env: Record<string, string>, log: string): Proc {
    const out = fs.openSync(path.join(root, log), "a");
    return cp.spawn(bin, args, { env, stdio: ["ignore", out, out] });
  }

  async function stop(p: Proc | null): Promise<void> {
    if (!p || p.exitCode !== null) return;
    await new Promise<void>((resolve) => {
      p.once("exit", () => resolve());
      p.kill("SIGTERM");
    });
  }

  /** The deployment's settings file, as deploy/dev/cloud.env.example has it. */
  function writeCloudEnv(): void {
    const lines = [
      "# e2e",
      `ZWIIM_CLOUD_ADDR=127.0.0.1:${cloudPort}`,
      `ZWIIM_CLOUD_DATA_DIR=${path.join(root, "cloud")}`,
      `ZWIIM_CLOUD_SUPABASE_URL=${supabaseUrl}`,
      "ZWIIM_CLOUD_LOG_FORMAT=json",
      // Through a tunnel, run-cloud.sh takes the URL tunnel.sh recorded.
      ...(tunnelled ? [] : [`ZWIIM_CLOUD_PUBLIC_URL=${cloudUrl}`]),
    ];
    fs.writeFileSync(path.join(root, "cloud.env"), lines.join("\n") + "\n");
  }

  const deployEnv = () => ({
    PATH: process.env.PATH ?? "",
    HOME: path.join(root, "home"),
    ZWIIM_CLOUD_ENV: path.join(root, "cloud.env"),
    ZWIIM_DEV_DIR: path.join(root, "dev"),
  });

  function startCloud(): void {
    cloudProc = spawn(
      path.join(DEPLOY, "run-cloud.sh"),
      [],
      { ...deployEnv(), ZWIIM_CLOUD_BIN: path.join(TARGET, "zwiim-cloud") },
      "cloud.log",
    );
  }

  /** The host; with `real`, the real Claude Code agent instead of the mock. */
  function startHost(real = false): void {
    hostProc = spawn(
      path.join(TARGET, "zwiim-host"),
      [],
      {
        PATH: real ? (process.env.PATH ?? "") : `${path.join(root, "bin")}:${process.env.PATH ?? ""}`,
        HOME: real ? (process.env.HOME ?? "") : path.join(root, "home"),
        // Claude Code finds its sign-in in the user's keychain entry.
        ...(real ? { USER: process.env.USER ?? "" } : {}),
        ZWIIM_DATA_DIR: path.join(root, "host"),
        ZWIIM_PORT: String(hostPort),
        ZWIIM_HOST: "127.0.0.1",
        ZWIIM_TOKEN: TOKEN,
        ZWIIM_HOST_KEY_STORE: "file",
        RUST_LOG: "info",
      },
      "host.log",
    );
  }

  /** The host's own computer: local authority, no Cloud in the path. */
  const local = new (class {
    request<O extends OpName>(op: O, params: OpParams<O>): Promise<OpResult<O>> {
      return new LocalTransport({ url: `http://127.0.0.1:${hostPort}`, token: TOKEN }).request(op, params) as Promise<
        OpResult<O>
      >;
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

  const hostOnline = () =>
    until(async () => {
      try {
        return (await listCloudHosts(shell, cloudUrl)).some((h) => h.online);
      } catch {
        return false;
      }
    }, 90_000);

  /** Start a public quick tunnel to the proxy with tunnel.sh; its https URL. */
  async function openTunnel(): Promise<string> {
    // The tunnel's origin is the proxy in front of the Cloud.
    tunnelProc = spawn(
      path.join(DEPLOY, "tunnel.sh"),
      [],
      { ...deployEnv(), ZWIIM_CLOUD_ADDR: `127.0.0.1:${proxyPort}` },
      "tunnel.log",
    );
    const recorded = path.join(root, "dev", "public-url");
    await until(() => fs.existsSync(recorded), 90_000);
    return fs.readFileSync(recorded, "utf8").trim();
  }

  /** The TCP addresses a process listens on. */
  function listening(pid: number): string[] {
    const out = cp.execFileSync("lsof", ["-nP", "-a", "-p", String(pid), "-iTCP", "-sTCP:LISTEN", "-Fn"], {
      encoding: "utf8",
    });
    return out.split("\n").filter((l: string) => l.startsWith("n")).map((l: string) => l.slice(1));
  }

  let invitation: KeyPairingInvitation;
  let qrText = "";
  let hostId = "";
  let workspaceId = "";
  let controller: HostController | null = null;
  const feedEpochs: string[] = [];
  const sessionFrames: [string, SessionFrame][] = [];
  const status = () => controller!.store.getState().connection.status;

  /** Connect as the Desktop does when the user picks the host. */
  async function select(): Promise<HostController> {
    controller?.dispose();
    const { controller: c } = await connectRemote(shell, cloudUrl, hostId, "cloud");
    c.client.subscribe({
      feed: (u) => void (u.type === "snapshot" && feedEpochs.push(String(u.snapshot.epoch))),
      session: (id, f) => sessionFrames.push([id, f]),
    });
    controller = c;
    return c;
  }

  beforeAll(async () => {
    [fs, path, os, net, cp] = await Promise.all(
      ["node:fs", "node:path", "node:os", "node:net", "node:child_process"].map(node),
    );
    root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "zwiim-r5-e2e-")));
    for (const d of ["bin", "home", "host", "cloud"]) fs.mkdirSync(path.join(root, d));
    fs.writeFileSync(path.join(root, "bin", "claude-agent-acp"), MOCK_AGENT, { mode: 0o755 });

    repo = path.join(root, "repo");
    fs.mkdirSync(repo);
    const git = (...args: string[]) => cp.execFileSync("git", ["-C", repo, ...args], { stdio: "ignore" });
    git("init", "-q", "-b", "main");
    fs.writeFileSync(path.join(repo, "README.md"), "r5 remote desktop\n");
    git("add", ".");
    git("-c", "user.name=e2e", "-c", "user.email=e2e@zwiim.test", "commit", "-q", "-m", "first commit over the relay");
    fs.writeFileSync(path.join(repo, "notes.txt"), "untracked\n");

    // The stand-in Supabase project, publishing its token signing key.
    signing = await newSigningKey();
    const jwk = { ...(await crypto.subtle.exportKey("jwk", signing.publicKey)), kid: KID, alg: "ES256", use: "sig" };
    const http = await node("node:http");
    supabase = http.createServer((req: any, res: any) => {
      if (req.url !== "/auth/v1/.well-known/jwks.json") return res.writeHead(404).end();
      jwksFetches++;
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ keys: [jwk] }));
    });
    await new Promise<void>((r) => supabase.listen(0, "127.0.0.1", r));
    supabaseUrl = `http://127.0.0.1:${supabase.address().port}`;

    [cloudPort, hostPort, proxyPort] = [await freePort(), await freePort(), await freePort()];
    proxy = net.createServer((client: any) => {
      const upstream = net.connect(cloudPort, "127.0.0.1");
      sockets.add(client);
      sockets.add(upstream);
      client.on("data", (b: Uint8Array) => seen.push(new Uint8Array(b)));
      upstream.on("data", (b: Uint8Array) => seen.push(new Uint8Array(b)));
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

    cloudUrl = tunnelled ? await openTunnel() : `http://127.0.0.1:${proxyPort}`;
    writeCloudEnv();
    startCloud();
    startHost();
    await hostUp();
    accountToken = await jwt(USER);
    // Reachable through the tunnel (its DNS name can take a moment).
    await until(async () => {
      try {
        await listCloudHosts(shell, cloudUrl);
        return true;
      } catch {
        return false;
      }
    }, 120_000);

    // On the host's computer: Devices → Remote access → Enable, with a
    // single-use token the signed-in Desktop fetched.
    const reply = await shell.cloudRequest("POST", "/v1/enrollments");
    const { enrollment_token } = reply.body as { enrollment_token: string };
    await local.request("cloud.enroll", { cloud_url: cloudUrl, enrollment_token, name: "e2e-host" });
    workspaceId = (await local.request("workspace.add", { path: repo })).workspace_id;
    hostId = (await local.request("host.hello", {})).host_id;
    await hostOnline();
  }, 240_000);

  afterAll(async () => {
    controller?.dispose();
    for (const s of sockets) s.destroy();
    proxy?.close();
    await stop(hostProc);
    await stop(cloudProc);
    await stop(tunnelProc);
    supabase?.close();
    if (root && !process.env.ZWIIM_REMOTE_E2E_KEEP) fs.rmSync(root, { recursive: true, force: true });
    else if (root) console.log(`kept ${root}`);
  }, 60_000);

  it("lists the account's host, online, and nothing to another account", async () => {
    console.log(`Cloud reached at ${cloudUrl}`);
    const hosts = await listCloudHosts(shell, cloudUrl);
    expect(hosts).toEqual([expect.objectContaining({ host_id: hostId, name: "e2e-host", online: true })]);
    const mine = accountToken;
    accountToken = await jwt("someone-else");
    expect(await listCloudHosts(shell, cloudUrl)).toEqual([]);
    accountToken = mine;
  });

  it("verifies account tokens against the project's published keys only", async () => {
    expect(jwksFetches).toBeGreaterThan(0);
    const forged = await jwt(USER, (await newSigningKey()).privateKey);
    const res = await fetch(`${cloudUrl}/v1/hosts`, { headers: { authorization: `Bearer ${forged}` } });
    expect(res.status).toBe(401);
  });

  it("answers health through the public URL, and has no legacy API", async () => {
    const health = await fetch(`${cloudUrl}/healthz`);
    expect([health.status, await health.text()]).toEqual([200, "ok"]);
    for (const p of ["/api/list_folders", "/api/terminal_spawn", "/api/health", "/ws/events"]) {
      const res = await fetch(`${cloudUrl}${p}`, { method: "POST", headers: { authorization: `Bearer ${accountToken}` } });
      expect(res.status, p).toBe(404);
    }
  });

  it("listens on loopback only: no inbound port on the host, the Cloud behind the tunnel", () => {
    const cloudPid = Number(cp.execFileSync("pgrep", ["-P", String(cloudProc!.pid)], { encoding: "utf8" }).trim());
    for (const pid of [hostProc!.pid!, cloudPid]) {
      const addrs = listening(pid);
      expect(addrs.length).toBeGreaterThan(0);
      for (const a of addrs) expect(a, `pid ${pid}`).toMatch(/^(127\.0\.0\.1|\[::1\]):\d+$/);
    }
  });

  it("refuses bad deployment settings before listening, naming the setting", () => {
    const check = (env: Record<string, string>) =>
      cp.spawnSync(path.join(TARGET, "zwiim-cloud"), ["--check"], { env: { PATH: "", ...env }, encoding: "utf8" });
    const good = { ZWIIM_CLOUD_PUBLIC_URL: "https://cloud.example.com", ZWIIM_CLOUD_DATA_DIR: root, ZWIIM_CLOUD_SUPABASE_URL: supabaseUrl };
    expect(check(good).status).toBe(0);
    for (const [bad, name] of [
      [{ ZWIIM_CLOUD_PUBLIC_URL: "http://cloud.example.com" }, "ZWIIM_CLOUD_PUBLIC_URL"],
      [{ ZWIIM_CLOUD_ADDR: "0.0.0.0:8443" }, "ZWIIM_CLOUD_ADDR"],
      [{ ZWIIM_CLOUD_SUPABASE_URL: "" }, "ZWIIM_CLOUD_SUPABASE_URL"],
    ] as const) {
      const r = check({ ...good, ...bad });
      expect(r.status, name).toBe(2);
      expect(r.stderr, name).toContain(name);
    }
  });

  it("shows a QR with the host's identity and a one-time code, and no address or secret", async () => {
    invitation = await local.request("device.pair_invite", {});
    qrText = JSON.stringify(invitation);
    expect(Object.keys(invitation).sort()).toEqual(["code", "expires_at", "host_id", "host_static_pubkey", "v"]);
    expect(invitation.host_id).toBe(hostId);
    for (const address of ["http", "ws", "127.0.0.1", "localhost", "trycloudflare", String(hostPort), TOKEN]) {
      expect(qrText).not.toContain(address);
    }
    const keys = JSON.parse(fs.readFileSync(path.join(root, "host", "host-keys.json"), "utf8"));
    expect(qrText).not.toContain(keys.static_secret);
    expect(qrText).not.toContain(keys.sign_secret);
  });

  it("refuses expired, forged, mismatched and wrong codes, and pins nothing", async () => {
    const tamper = (o: Partial<KeyPairingInvitation>) => JSON.stringify({ ...invitation, ...o });
    const other = encodeKey(generateDeviceKeys().secret);
    await expect(
      pairRemote(shell, cloudUrl, tamper({ expires_at: new Date(Date.now() - 1000).toISOString() })),
    ).rejects.toMatchObject({ code: "pairing_expired" });
    await expect(pairRemote(shell, cloudUrl, tamper({ host_static_pubkey: other }))).rejects.toMatchObject({
      code: "pairing_invalid",
    });
    await expect(pairRemote(shell, cloudUrl, qrText, { expectedHostId: "some-other-host" })).rejects.toMatchObject({
      code: "pairing_invalid",
    });
    await expect(pairRemote(shell, cloudUrl, tamper({ code: "not-the-code" }))).rejects.toMatchObject({
      code: "pairing_rejected",
    });
    await expect(pairRemote(shell, cloudUrl, `${qrText.slice(0, -1)},"url":"https://x"}`)).rejects.toMatchObject({
      code: "pairing_invalid",
    });
    expect(pins).toEqual([]);
    expect((await local.request("device.list", {})).devices).toEqual([]);
  }, 60_000);

  it("pairs through the Cloud, pins the host key, and refuses the code's reuse", async () => {
    const pinned = await pairRemote(shell, cloudUrl, qrText, { expectedHostId: hostId, name: "e2e-host" });
    expect(pinned).toMatchObject({ host_id: hostId, host_static_pubkey: invitation.host_static_pubkey });
    expect(pins).toEqual([pinned]);
    expect((await local.request("device.list", {})).devices).toEqual([
      expect.objectContaining({
        device_id: identity.device_id,
        device_name: "E2E Desktop",
        device_pubkey: deviceKeys.publicKey,
        status: "trusted",
      }),
    ]);
    await expect(pairRemote(shell, cloudUrl, qrText)).rejects.toMatchObject({ code: "pairing_rejected" });
    expect((await local.request("device.list", {})).devices).toHaveLength(1);
  }, 60_000);

  it("drives workspaces, files, Git and a terminal through the relay, as the local host answers", async () => {
    const c = await select();
    await until(() => status() === "live", 60_000);
    await until(() => c.store.getState().workspaces?.some((w) => w.workspace_id === workspaceId) ?? false);
    const same = async <O extends OpName>(op: O, params: OpParams<O>) => {
      const viaRelay = await c.client.request(op, params);
      expect(viaRelay).toEqual(await local.request(op, params));
      const json = JSON.stringify(viaRelay);
      expect(json).not.toContain(root);
      expect(json).not.toContain(TOKEN);
      return json;
    };
    expect(await same("host.hello", {})).toContain(hostId);
    expect(await same("workspace.tree", { workspace_id: workspaceId, max_depth: 1 } as OpParams<"workspace.tree">)).toContain(
      "README.md",
    );
    expect(await same("files.read", { workspace_id: workspaceId, path: "README.md" })).toContain("r5 remote desktop");
    expect(await same("git.status", { workspace_id: workspaceId })).toContain("notes.txt");
    expect(await same("git.log", { workspace_id: workspaceId })).toContain(
      "first commit over the relay",
    );
    // Only Zwiim operations cross the tunnel: never the legacy API.
    for (const op of ["list_folders", "../../api/list_folders", "terminal_spawn"]) {
      await expect(c.client.request(op as OpName, {} as never), op).rejects.toBeTruthy();
    }
    expect(status()).toBe("live");

    const outputs: TerminalOutput[] = [];
    const off = c.onTerminalOutput((_, o) => outputs.push(o));
    const tid = await c.spawnTerminal(workspaceId);
    c.attachTerminal(tid);
    await until(() => outputs.some((o) => o.kind === "reset"));
    await c.client.request("terminal.write", { terminal_id: tid, data: "echo r5-$((40+2))\r" });
    const shown = () => outputs.map((o) => (o.kind === "append" || o.kind === "reset" ? o.data : "")).join("");
    await until(() => shown().includes("r5-42"));
    await c.client.request("terminal.kill", { terminal_id: tid });
    await until(() => outputs.some((o) => o.kind === "exit"));
    off();
  }, 120_000);

  let sessionId = "";
  const events = () =>
    sessionFrames.flatMap(([id, f]) =>
      id !== sessionId ? [] : f.type === "event" ? [f.event] : f.type === "replay" ? f.events : [],
    );
  const said = () =>
    events()
      .filter((e) => e.type === "agent.message_delta")
      .map((e) => (e.payload as { text: string }).text)
      .join("");
  const turns = () => events().filter((e) => e.type === "agent.turn_complete").length;
  const noDuplicates = () => {
    const seqs = events().map((e) => e.seq);
    expect(new Set(seqs).size).toBe(seqs.length);
    expect([...seqs].sort((a, b) => a - b)).toEqual(seqs);
  };

  it("starts a session, streams it live, sends again, and reads its history", async () => {
    sessionId = await controller!.startSession({
      workspace_id: workspaceId,
      agent: "claude_code",
      blocks: [{ type: "text", text: "hello from the desktop" }],
    });
    await until(() => said().includes("echo: hello from the desktop"), 60_000);
    await until(() => turns() === 1);
    await controller!.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "second" }] });
    await until(() => said().includes("echo: second"), 60_000);
    await until(() => turns() === 2);
    const history = JSON.stringify(await controller!.client.request("session.history", { session_id: sessionId }));
    expect(history).toContain("hello from the desktop");
    expect(history).toContain("echo: second");
    expect(history).not.toContain(root);
    noDuplicates();
  }, 120_000);

  it("recovers a relay cut without duplicate events", async () => {
    for (const s of sockets) s.destroy();
    await until(() => status() !== "live");
    await until(() => status() === "live", 90_000);
    await controller!.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "after the cut" }] });
    await until(() => said().includes("echo: after the cut"), 60_000);
    await until(() => turns() === 3);
    noDuplicates();
  }, 180_000);

  it("keeps local work going through a Cloud outage, then reconnects without enrolling again", async () => {
    await stop(cloudProc);
    expect(fs.readFileSync(path.join(root, "cloud.log"), "utf8")).toContain("[cloud] stopped");
    // What survives a restart: the registry, public keys and owners only.
    const registry = path.join(root, "cloud", "registry.json");
    expect(fs.statSync(registry).mode & 0o777).toBe(0o600);
    expect(fs.readFileSync(registry, "utf8")).toContain(hostId);
    await until(() => status() !== "live", 60_000);
    expect((await local.request("workspace.list", {})).length).toBeGreaterThan(0);
    await expect(listCloudHosts(shell, cloudUrl)).rejects.toBeTruthy();
    startCloud();
    await hostOnline();
    await until(() => status() === "live", 120_000);
    await controller!.sendMessage({ session_id: sessionId, blocks: [{ type: "text", text: "after the outage" }] });
    await until(() => said().includes("echo: after the outage"), 60_000);
    noDuplicates();
  }, 240_000);

  it("shows the host offline while it is down and refreshes from its new epoch", async () => {
    const before = feedEpochs.at(-1);
    await stop(hostProc);
    await until(async () => !(await listCloudHosts(shell, cloudUrl)).some((h) => h.online), 90_000);
    await until(() => status() !== "live");
    startHost();
    await hostUp();
    await hostOnline();
    await until(() => status() === "live" && feedEpochs.at(-1) !== before, 180_000);
    const history = JSON.stringify(await controller!.client.request("session.history", { session_id: sessionId }));
    expect(history).toContain("echo: after the outage");
  }, 300_000);

  it("reconnects as the same device after a Desktop restart", async () => {
    await select();
    await until(() => status() === "live", 60_000);
    expect((await controller!.client.request("host.hello", {})).host_id).toBe(hostId);
  }, 90_000);

  it("signing out keeps the host, its agents and state; signing in reconnects", async () => {
    controller!.dispose();
    controller = null;
    accountToken = null;
    await expect(listCloudHosts(shell, cloudUrl)).rejects.toMatchObject({ code: "unauthenticated" });
    const refused = await select();
    await until(() => status() === "unauthenticated", 60_000);
    expect(refused.client.state.error?.code).toBe("unauthenticated");
    refused.dispose();
    controller = null;
    // The host was never told: it runs, and keeps the session.
    expect(JSON.stringify(await local.request("session.history", { session_id: sessionId }))).toContain("after the outage");
    accountToken = await jwt(USER);
    await select();
    await until(() => status() === "live", 60_000);
  }, 120_000);

  const REAL_PROMPT = "Reply with exactly the word zwiim-r6-ok and nothing else.";
  it.skipIf(!realAgent)("runs a real Claude Code turn through the relay", async () => {
    // The host again, with the real agent in its PATH.
    await stop(hostProc);
    startHost(true);
    await hostUp();
    await hostOnline();
    await until(() => status() === "live", 180_000);
    const realId = await controller!.startSession({
      workspace_id: workspaceId,
      agent: "claude_code",
      blocks: [{ type: "text", text: REAL_PROMPT }],
    });
    const mine = () =>
      sessionFrames.flatMap(([id, f]) => (id !== realId ? [] : f.type === "event" ? [f.event] : f.type === "replay" ? f.events : []));
    await until(() => mine().some((e) => e.type === "agent.turn_complete"), 180_000);
    const reply = mine()
      .filter((e) => e.type === "agent.message_delta")
      .map((e) => (e.payload as { text: string }).text)
      .join("");
    console.log(`real agent replied: ${JSON.stringify(reply.slice(0, 200))}`);
    expect(reply).toContain("zwiim-r6-ok");
  }, 300_000);

  it("the Cloud relayed only ciphertext", () => {
    let bytes = "";
    for (const b of seen) bytes += new TextDecoder("latin1").decode(b);
    expect(bytes.length).toBeGreaterThan(10_000);
    for (const plain of ["hello from the desktop", "echo: second", "r5-42", "r5 remote desktop", "first commit over", REAL_PROMPT, workspaceId, sessionId]) {
      expect(bytes).not.toContain(plain);
    }
  });

  it("a revoked device is dropped at once and cannot reconnect", async () => {
    await local.request("device.revoke", { device_id: identity.device_id });
    await until(() => status() === "unauthenticated", 60_000);
    expect(controller!.client.state.error?.code).toBe("device_refused");
    const again = await select();
    await until(() => again.store.getState().connection.status === "unauthenticated", 60_000);
    expect(again.client.state.error?.code).toBe("device_refused");
  }, 150_000);

  it("keeps secrets out of logs and Cloud tokens out of the host", () => {
    const keys = JSON.parse(fs.readFileSync(path.join(root, "host", "host-keys.json"), "utf8"));
    const secrets = [identity.secret, keys.static_secret, keys.sign_secret, ...issued.map((t) => t.split(".")[2]!)];
    for (const log of ["host.log", "cloud.log", "tunnel.log", "dev/cloudflared.log"]) {
      if (!fs.existsSync(path.join(root, log))) continue;
      const body = fs.readFileSync(path.join(root, log), "utf8");
      for (const s of [...secrets, invitation.code]) expect(body, log).not.toContain(s);
    }
    // No account token or device secret anywhere in the host's data.
    const walk = (dir: string): string[] =>
      fs.readdirSync(dir, { withFileTypes: true }).flatMap((e: any) =>
        e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)],
      );
    for (const file of walk(path.join(root, "host"))) {
      const body = new TextDecoder("latin1").decode(fs.readFileSync(file));
      for (const s of [identity.secret, ...issued.map((t) => t.split(".")[2]!)]) expect(body, file).not.toContain(s);
    }
  });
});
