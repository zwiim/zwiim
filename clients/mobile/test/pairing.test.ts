import { CloudClient, generateDeviceKeys, ZwiimClientError, type KeyPairingInvitation } from "@zwiim/sdk";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AccountAuth } from "../src/account/auth";
import { accountHosts } from "../src/account/hosts";
import { openHostConnection } from "../src/host/open";
import {
  ensureIdentity,
  pairDevice,
  pairingProblem,
  pairingStep,
  problemCopy,
  readPairingCode,
  type PairingState,
} from "../src/pairing/pairing";
import { MemorySecureStore } from "../src/platform/memory-store";
import { VaultSecureStore, type Vault } from "../src/platform/vault-store";
import { FakeCloud, FakeHost } from "../../sdk/test/relay-fakes";
import { ANON_KEY, FakeSupabase, SUPABASE } from "./auth-fakes";
import { TUNING, waitFor } from "./support";

afterEach(() => {
  vi.unstubAllGlobals();
});

function pairingWorld() {
  const cloud = new FakeCloud();
  const host = new FakeHost();
  cloud.addHost(host, "u1", "Studio Mac");
  const client = new CloudClient({ url: cloud.origin, accessToken: "token-u1", fetch: cloud.fetch, WebSocket: cloud.WebSocket });
  const chosen = { host_id: host.hostId, static_pubkey: host.publicKey };
  return { cloud, host, client, chosen };
}

const pair = (store: MemorySecureStore, client: CloudClient, invitation: KeyPairingInvitation) =>
  pairDevice({ store, cloud: client, invitation, deviceName: "Pixel" }).then(
    () => null,
    (e: unknown) => pairingProblem(e),
  );

describe("reading a pairing QR", () => {
  it("accepts the chosen host's current code", () => {
    const { host, chosen } = pairingWorld();
    const invite = host.invite();
    expect(readPairingCode(JSON.stringify(invite), chosen)).toEqual({ invitation: invite });
  });

  it("names what is wrong with any other", () => {
    const { host, chosen } = pairingWorld();
    const other = new FakeHost("host-2");
    expect(readPairingCode("https://example.com", chosen)).toEqual({ problem: "not_a_code" });
    expect(readPairingCode(JSON.stringify({ v: 1, url: "x" }), chosen)).toEqual({ problem: "not_a_code" });
    const old = host.invite(1000, Date.now() - 5000);
    expect(readPairingCode(JSON.stringify(old), chosen)).toEqual({ problem: "expired" });
    expect(readPairingCode(JSON.stringify(other.invite()), chosen)).toEqual({ problem: "wrong_host" });
    // The chosen host's id with another key: not the host the account enrolled.
    const forged = { ...host.invite(), host_static_pubkey: other.publicKey };
    expect(readPairingCode(JSON.stringify(forged), chosen)).toEqual({ problem: "wrong_host" });
  });
});

describe("pairing through the Cloud", () => {
  it("pairs a new device key, pins the QR's host key, and keeps one identity", async () => {
    const { host, client } = pairingWorld();
    const store = new MemorySecureStore();
    expect(await pair(store, client, host.invite())).toBeNull();
    const identity = (await store.deviceIdentity())!;
    expect(identity.device_id).toMatch(/^mobile-[0-9a-f]{16}$/);
    expect(host.paired.get(identity.keys.publicKey)).toEqual({ device_id: identity.device_id, device_name: "Pixel" });
    expect(await store.pinnedHost(host.hostId)).toEqual({ host_id: host.hostId, host_static_pubkey: host.publicKey });
    expect((await ensureIdentity(store)).device_id).toBe(identity.device_id);
  });

  it("refuses a code already used", async () => {
    const { host, client } = pairingWorld();
    const invite = host.invite();
    expect(await pair(new MemorySecureStore(), client, invite)).toBeNull();
    const store = new MemorySecureStore();
    expect(await pair(store, client, invite)).toBe("rejected");
    expect(await store.pinnedHost(host.hostId)).toBeNull();
  });

  it("refuses a code the host let expire", async () => {
    const { host, client } = pairingWorld();
    const invite = host.invite();
    host.codes.set(invite.code, Date.now() - 1);
    expect(await pair(new MemorySecureStore(), client, invite)).toBe("rejected");
  });

  it("refuses a host whose Cloud key differs from the QR's", async () => {
    const { host, client } = pairingWorld();
    const forged = { ...host.invite(), host_static_pubkey: new FakeHost("x").publicKey };
    const store = new MemorySecureStore();
    expect(await pair(store, client, forged)).toBe("wrong_host");
    expect(await store.pinnedHost(host.hostId)).toBeNull();
  });

  it("reports a host outside the account, offline, or an unreachable Cloud", async () => {
    const { cloud, host, client } = pairingWorld();
    const stranger = new FakeHost("host-9");
    cloud.addHost(stranger, "u2");
    expect(await pair(new MemorySecureStore(), client, stranger.invite())).toBe("unknown_host");
    cloud.setOnline(host.hostId, false);
    expect(await pair(new MemorySecureStore(), client, host.invite())).toBe("offline");
    cloud.down = true;
    expect(await pair(new MemorySecureStore(), client, host.invite())).toBe("unreachable");
  });

  it("gives every problem words and one next action", () => {
    expect(problemCopy("expired", "Studio Mac")).toMatchObject({ title: "This code expired", action: "scan" });
    expect(problemCopy("rejected", "Studio Mac").body).toMatch(/used already or expired/);
    expect(problemCopy("wrong_host", "Studio Mac").body).toBe("It doesn't match Studio Mac in your account.");
    expect(problemCopy("offline", "Studio Mac")).toMatchObject({ title: "Studio Mac is offline", action: "retry" });
    expect(problemCopy("signed_out", "x").action).toBe("sign_in");
    expect(pairingProblem(new Error("boom"))).toBe("failed");
  });
});

describe("the pairing screen's steps", () => {
  const { host, chosen } = pairingWorld();
  const invite = host.invite();
  const step = (s: PairingState, ...events: Parameters<typeof pairingStep>[1][]) =>
    events.reduce((st, e) => pairingStep(st, e, chosen), s);

  it("goes explain → scan → confirm → pairing → done", () => {
    const scanned = step({ step: "explain" }, { type: "scan" }, { type: "scanned", text: JSON.stringify(invite) });
    expect(scanned).toEqual({ step: "confirm", invitation: invite });
    // The camera keeps reporting the code: nothing changes.
    expect(step(scanned, { type: "scanned", text: "junk" })).toBe(scanned);
    const done = step(scanned, { type: "pair" }, { type: "paired" });
    expect(done).toEqual({ step: "done", hostId: host.hostId });
  });

  it("fails on a bad scan, and scans again", () => {
    const failed = step({ step: "scan" }, { type: "scanned", text: "junk" });
    expect(failed).toEqual({ step: "failed", problem: "not_a_code", invitation: null });
    expect(step(failed, { type: "retry" })).toEqual({ step: "scan" });
  });

  it("retries the same code when only the network failed, and asks for a new one otherwise", () => {
    const pairing: PairingState = { step: "pairing", invitation: invite };
    const offline = step(pairing, { type: "error", error: new ZwiimClientError("host_offline", "offline") });
    expect(offline).toEqual({ step: "failed", problem: "offline", invitation: invite });
    expect(step(offline, { type: "retry" })).toEqual({ step: "confirm", invitation: invite });
    const used = step(pairing, { type: "error", error: new ZwiimClientError("pairing_rejected", "used") });
    expect(used).toMatchObject({ step: "failed", problem: "rejected" });
    expect(step(used, { type: "retry" })).toEqual({ step: "scan" });
  });
});

describe("login → hosts → pairing → connection", () => {
  it("pairs once and reconnects after a restart from the kept identity and pin", async () => {
    // The account: Supabase issues a token the Cloud accepts.
    const supabase = new FakeSupabase();
    const cloud = new FakeCloud();
    const host = new FakeHost();
    cloud.addHost(host, "u1", "Studio Mac");
    vi.stubGlobal("fetch", cloud.fetch);
    vi.stubGlobal("WebSocket", cloud.WebSocket);

    const items = new Map<string, string>();
    const vault: Vault = {
      getItemAsync: async (k) => items.get(k) ?? null,
      setItemAsync: async (k, v) => void items.set(k, v),
      deleteItemAsync: async (k) => void items.delete(k),
    };
    const launch = () => {
      const store = new VaultSecureStore(vault);
      const auth = new AccountAuth({
        config: { url: SUPABASE, anonKey: ANON_KEY },
        store,
        openAuthSession: supabase.openAuthSession,
        fetch: supabase.fetch,
        now: () => supabase.now,
      });
      const accessToken = () => auth.accessToken();
      return { store, auth, accessToken, cloud: new CloudClient({ url: cloud.origin, accessToken }) };
    };

    // First launch: sign in, list, pair from the QR, connect.
    const app = launch();
    await app.auth.signIn();
    for (const t of supabase.issued) cloud.accounts.set(t, "u1");
    const hosts = await accountHosts(app.cloud, app.store, () => app.auth.accessToken(true));
    expect(hosts.map((h) => [h.name, h.online, h.paired])).toEqual([["Studio Mac", true, false]]);
    const read = readPairingCode(JSON.stringify(host.invite()), hosts[0]!);
    if (!("invitation" in read)) throw new Error(read.problem);
    await pairDevice({ store: app.store, cloud: app.cloud, invitation: read.invitation, deviceName: "Pixel" });
    expect((await accountHosts(app.cloud, app.store, async () => {}))[0]!.paired).toBe(true);

    const target = { hostId: host.hostId, cloudUrl: cloud.origin, lanEndpoint: null };
    const first = await openHostConnection(app.store, target, { mode: "auto", dial: null, accessToken: app.accessToken, tuning: TUNING });
    first.connect();
    await waitFor(() => first.view.status === "live");
    expect(first.view.active).toBe("relay");
    expect(await first.request("host.hello", {})).toMatchObject({ host_id: host.hostId });
    first.close();
    const codesLeft = host.codes.size;

    // Killed and relaunched: nothing but the platform storage survives.
    const again = launch();
    expect(await again.auth.current()).toEqual({ user_id: "u1", email: "u1@example.com" });
    const second = await openHostConnection(again.store, target, {
      mode: "cloud",
      dial: null,
      accessToken: again.accessToken,
      tuning: TUNING,
    });
    second.connect();
    await waitFor(() => second.view.status === "live");
    expect(await second.request("host.hello", {})).toMatchObject({ host_id: host.hostId });
    second.close();
    // No second pairing: no code was spent, and the host knows one device.
    expect(host.codes.size).toBe(codesLeft);
    expect(host.paired.size).toBe(1);

    // Local only has no LAN path without discovery: it says so, never the Cloud.
    const local = await openHostConnection(again.store, target, { mode: "local", dial: null, accessToken: again.accessToken, tuning: TUNING });
    const before = cloud.http.length;
    local.connect();
    await waitFor(() => local.view.error?.code === "lan_unavailable");
    expect(cloud.http.length).toBe(before);
    local.close();
  });

  it("still refuses a host that cannot prove the pinned key", async () => {
    const { cloud, host } = pairingWorld();
    vi.stubGlobal("fetch", cloud.fetch);
    vi.stubGlobal("WebSocket", cloud.WebSocket);
    const store = new MemorySecureStore();
    const keys = generateDeviceKeys();
    host.trusted.add(keys.publicKey);
    await store.setDeviceIdentity({ device_id: "mobile-1", keys });
    // Pinned at pairing; the Mac answering now has another key.
    await store.pinHost({ host_id: host.hostId, host_static_pubkey: new FakeHost("x").publicKey });
    const c = await openHostConnection(
      store,
      { hostId: host.hostId, cloudUrl: cloud.origin, lanEndpoint: null },
      { mode: "cloud", dial: null, accessToken: async () => "token-u1", tuning: TUNING },
    );
    const seen: string[] = [];
    c.subscribe((v) => seen.push(v.status));
    c.connect();
    // The host cannot read a handshake sealed to another key: it closes, and
    // nothing reaches it, however often the phone retries.
    await waitFor(() => cloud.relays.length >= 3);
    expect(seen).not.toContain("live");
    expect(c.view.hostId).toBeNull();
    expect(host.handshakes).toBe(0);
    expect(host.received).toEqual([]);
    c.close();
  });
});
