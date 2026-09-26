import { describe, expect, it } from "vitest";
import { HostController } from "../src/client/controller";
import { HostClient, type HostFeedUpdate } from "../src/client/host-client";
import { CloudClient } from "../src/remote/cloud";
import { generateDeviceKeys } from "../src/remote/keys";
import type { SessionFrame } from "../src/state/session";
import {
  activeConnection,
  ConnectionManager,
  type ConnectionMode,
  type ConnectionPath,
} from "../src/transport/connection-manager";
import { LanTransport, parseLanAdvertisement } from "../src/transport/lan";
import { createTransport } from "../src/transport/select";
import type { SessionEvent } from "../src/types/protocol";
import { FakeLan } from "./lan-fakes";
import { FakeCloud, FakeHost, waitFor } from "./relay-fakes";

const BACKOFF = { baseMs: 5, maxMs: 20 };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const delta = (seq: number, text: string): SessionEvent => ({
  session_id: "s1",
  seq,
  type: "agent.message_delta",
  payload: { text },
});

const snapshot = {
  status: "prompting" as const,
  live_message: null,
  tool_calls: [],
  pending_request: null,
  pending_user_message: null,
  last_error: null,
  usage: null,
  options: null,
  commands: [],
};

interface SetupOptions {
  mode?: ConnectionMode;
  onLan?: boolean;
  withCloud?: boolean;
  trusted?: boolean;
  pinned?: string;
  lanRetry?: { baseMs: number; maxMs: number };
}

function setup(opts: SetupOptions = {}) {
  const cloud = new FakeCloud();
  const host = new FakeHost();
  cloud.addHost(host);
  const lan = new FakeLan();
  const listener = opts.onLan === false ? null : lan.advertise(host);
  const device = generateDeviceKeys();
  if (opts.trusted !== false) host.trusted.add(device.publicKey);
  const cloudClient = new CloudClient({
    url: cloud.origin,
    accessToken: "token-u1",
    fetch: cloud.fetch,
    WebSocket: cloud.WebSocket,
  });
  const manager = new ConnectionManager({
    mode: opts.mode ?? "auto",
    hostId: host.hostId,
    hostStaticPubkey: opts.pinned ?? host.publicKey,
    deviceSecret: device.secret,
    lan: { discover: lan.discover, dial: lan.dial },
    cloud: opts.withCloud === false ? null : cloudClient,
    timeoutMs: 2000,
    lanRetry: opts.lanRetry ?? { baseMs: 60_000, maxMs: 60_000 },
  });
  const paths: (ConnectionPath | null)[] = [];
  manager.onPath((p) => paths.push(p));
  const client = new HostClient({
    transport: manager,
    expectedHostId: host.hostId,
    backoff: BACKOFF,
    random: () => 0.5,
  });
  const statuses: string[] = [];
  const frames: [string, SessionFrame][] = [];
  const feed: HostFeedUpdate[] = [];
  client.subscribe({
    connection: (s) => {
      if (statuses[statuses.length - 1] !== s.status) statuses.push(s.status);
    },
    session: (id, f) => frames.push([id, f]),
    feed: (u) => feed.push(u),
  });
  const shown = () => activeConnection(client.state.status, manager.path);
  return { cloud, host, lan, listener, device, cloudClient, manager, client, statuses, frames, feed, paths, shown };
}

type T = ReturnType<typeof setup>;

const tickets = (t: T) => t.cloud.http.filter((r) => r.path.endsWith("/tickets")).length;
const snapshots = (t: T) => t.feed.filter((u) => u.type === "snapshot").length;
const events = (t: T) =>
  t.frames.flatMap(([, f]) =>
    f.type === "event" ? [f.event.seq] : f.type === "replay" ? f.events.map((e) => e.seq) : [],
  );

async function live(t: T) {
  t.client.start();
  await waitFor(() => t.client.state.status === "live" && snapshots(t) > 0);
}

describe("LAN advertisements", () => {
  const host = new FakeHost("0b6c1f7e-6a53-4e4e-9d57-1f0e8f7f2a11");
  const good = {
    instance: host.hostId,
    port: 50123,
    addresses: ["192.168.1.20", "fe80::1%en0", "192.168.1.20"],
    txt: { v: "1", id: host.hostId, pk: host.publicKey },
  };

  it("keeps the host id, key, port and addresses only", () => {
    expect(parseLanAdvertisement({ ...good, extra: "ignored" })).toEqual({
      hostId: host.hostId,
      hostStaticPubkey: host.publicKey,
      addresses: ["192.168.1.20", "fe80::1%en0"],
      port: 50123,
    });
  });

  it("rejects malformed advertisements", () => {
    const bad = [
      null,
      "text",
      { ...good, txt: undefined },
      { ...good, txt: { ...good.txt, v: "2" } },
      { ...good, instance: "someone-else" },
      { ...good, txt: { ...good.txt, id: "a b" } },
      { ...good, txt: { ...good.txt, pk: "AAAA" } },
      { ...good, txt: { ...good.txt, pk: 7 } },
      { ...good, port: 0 },
      { ...good, port: 70000 },
      { ...good, port: "50123" },
      { ...good, addresses: [] },
      { ...good, addresses: "192.168.1.20" },
      // Names are never resolved; unspecified, multicast and broadcast
      // addresses are not dialed.
      { ...good, addresses: ["host.local", "0.0.0.0", "224.0.0.251", "255.255.255.255", "::", "ff02::fb"] },
      { ...good, addresses: ["300.1.1.1"] },
    ];
    for (const ad of bad) expect(parseLanAdvertisement(ad), JSON.stringify(ad)).toBeNull();
  });
});

describe("LAN transport", () => {
  function lanTransport(t: T, overrides: { pinned?: string } = {}) {
    return new LanTransport({
      hostId: t.host.hostId,
      hostStaticPubkey: overrides.pinned ?? t.host.publicKey,
      deviceSecret: t.device.secret,
      discover: t.lan.discover,
      dial: t.lan.dial,
    });
  }

  it("dials the advertised endpoint and runs the same tunnel as the relay", async () => {
    const t = setup();
    const transport = lanTransport(t);
    expect(await transport.request("host.hello", {})).toMatchObject({ host_id: t.host.hostId });
    expect(t.lan.dials).toEqual([`192.168.1.20:${t.listener!.port}`]);
    expect(t.host.handshakes).toBe(1);
    expect(t.cloud.http).toEqual([]);
    // Only ciphertext on the wire.
    expect(t.host.received.some((m) => m.includes("host.hello"))).toBe(true);
    transport.close();
  });

  it("refuses an advertisement whose key is not the pinned one, without dialing", async () => {
    const t = setup();
    // An impostor on the network advertises its own key under the host's id.
    const impostor = new FakeHost(t.host.hostId);
    t.lan.advertise(impostor, "192.168.1.66");
    const err = await lanTransport(t).request("host.hello", {}).catch((e) => e);
    expect(err).toMatchObject({ code: "host_identity_mismatch" });
    expect(t.lan.dials).toEqual([]);
    expect(impostor.handshakes + t.host.handshakes).toBe(0);
  });

  it("does not trust a host that claims the pinned key but cannot prove it", async () => {
    const t = setup();
    t.listener!.announced = false;
    const impostor = new FakeHost(t.host.hostId);
    const l = t.lan.advertise(impostor, "192.168.1.66", false);
    t.lan.extra.push(t.lan.ad(l, { pk: t.host.publicKey }));
    const err = await lanTransport(t).request("host.hello", {}).catch((e) => e);
    // It cannot read the handshake, so it cannot answer: nothing is
    // exchanged, and the LAN path is just unavailable.
    expect(err).toMatchObject({ code: "lan_unavailable" });
    expect(t.lan.dials).toEqual([`192.168.1.66:${l.port}`]);
    expect(impostor.handshakes).toBe(0);
    expect(impostor.received).toEqual([]);
  });

  it("gets nothing from the host when the pinned key is not the host's", async () => {
    const t = setup();
    const other = new FakeHost();
    // Discovery agrees with the wrong pin: the handshake still fails.
    t.listener!.announced = false;
    t.lan.extra.push(t.lan.ad(t.listener!, { pk: other.publicKey }));
    const err = await lanTransport(t, { pinned: other.publicKey }).request("host.hello", {}).catch((e) => e);
    expect(err).toMatchObject({ code: "lan_unavailable" });
    expect(t.lan.dials).toHaveLength(1);
    expect(t.host.handshakes).toBe(0);
  });

  it("refuses unknown and revoked devices exactly as through the Cloud", async () => {
    const unknown = setup({ trusted: false });
    expect(await lanTransport(unknown).request("host.hello", {}).catch((e) => e)).toMatchObject({
      code: "device_refused",
    });

    const revoked = setup();
    const transport = lanTransport(revoked);
    await transport.request("host.hello", {});
    revoked.host.revoke(revoked.device.publicKey);
    await waitFor(() => !transport.connected);
    expect(await transport.request("host.hello", {}).catch((e) => e)).toMatchObject({ code: "device_refused" });
    // Same verdict through the relay.
    const relay = createTransport({
      kind: "relay",
      cloud: revoked.cloudClient,
      hostId: revoked.host.hostId,
      hostStaticPubkey: revoked.host.publicKey,
      deviceSecret: revoked.device.secret,
    });
    expect(await relay.request("host.hello", {}).catch((e) => e)).toMatchObject({ code: "device_refused" });
  });

  it("is unavailable when the host is not advertised, or not answering where it was", async () => {
    const t = setup({ onLan: false });
    expect(await lanTransport(t).request("host.hello", {}).catch((e) => e)).toMatchObject({
      code: "lan_unavailable",
    });
    const s = setup();
    s.lan.stale = true;
    s.lan.leave();
    expect(await lanTransport(s).request("host.hello", {}).catch((e) => e)).toMatchObject({
      code: "lan_unavailable",
    });
    expect(s.lan.dials).toHaveLength(1);
  });

  it("builds from a target like the other transports", () => {
    const t = setup();
    const target = {
      hostId: t.host.hostId,
      hostStaticPubkey: t.host.publicKey,
      deviceSecret: t.device.secret,
    };
    expect(createTransport({ kind: "lan", ...target, discover: t.lan.discover, dial: t.lan.dial })).toBeInstanceOf(
      LanTransport,
    );
    expect(createTransport({ kind: "remote", mode: "auto", ...target })).toBeInstanceOf(ConnectionManager);
  });
});

describe("automatic mode", () => {
  it("connects directly when the host is on the LAN, without the Cloud", async () => {
    const t = setup();
    await live(t);
    expect(t.manager.path).toBe("lan");
    expect(t.shown()).toBe("lan");
    expect(tickets(t)).toBe(0);
    expect(t.cloud.relays).toHaveLength(0);
    expect(await t.client.request("workspace.list", {})).toEqual([]);
    t.client.stop();
  });

  it("uses the relay when the host is not on the LAN, and does not look offline", async () => {
    const t = setup({ onLan: false });
    await live(t);
    expect(t.manager.path).toBe("relay");
    expect(t.shown()).toBe("relay");
    expect(t.lan.discoveries).toBe(1);
    expect(t.statuses).not.toContain("error");
    expect(t.client.state.error).toBeNull();
    t.client.stop();
  });

  it("uses the relay when the advertised host does not answer", async () => {
    const t = setup();
    t.lan.stale = true;
    t.lan.leave();
    await live(t);
    expect(t.manager.path).toBe("relay");
    expect(t.lan.dials).toHaveLength(1);
    t.client.stop();
  });

  it("reports an identity mismatch on the LAN instead of hiding it behind the relay", async () => {
    const t = setup();
    const impostor = new FakeHost(t.host.hostId);
    t.lan.advertise(impostor, "192.168.1.66");
    t.client.start();
    await waitFor(() => t.client.state.status === "error");
    expect(t.client.state.error?.code).toBe("host_identity_mismatch");
    expect(t.shown()).toBe("offline");
    expect(tickets(t)).toBe(0);
    await sleep(30);
    expect(t.lan.discoveries).toBe(1); // terminal: not retried
  });

  it("reports a refused device on the LAN instead of trying the relay", async () => {
    const t = setup({ trusted: false });
    t.client.start();
    await waitFor(() => t.client.state.status === "unauthenticated");
    expect(t.client.state.error?.code).toBe("device_refused");
    expect(tickets(t)).toBe(0);
  });

  it("works on the LAN alone when there is no Cloud", async () => {
    const t = setup({ withCloud: false });
    await live(t);
    expect(t.manager.path).toBe("lan");
    t.lan.leave();
    await waitFor(() => t.client.state.status === "reconnecting" && t.client.state.error?.code === "lan_unavailable");
    expect(t.cloud.http).toEqual([]);
    t.lan.join();
    await waitFor(() => t.client.state.status === "live");
    t.client.stop();
  });
});

describe("local-only mode", () => {
  it("connects directly and never calls the Cloud", async () => {
    const t = setup({ mode: "local" });
    await live(t);
    expect(t.manager.path).toBe("lan");
    t.lan.cut();
    await waitFor(() => t.client.state.status === "live" && t.lan.openLinks.length === 1);
    expect(t.cloud.http).toEqual([]);
    t.client.stop();
  });

  it("fails clearly when the host is not reachable locally, and keeps trying locally", async () => {
    const t = setup({ mode: "local", onLan: false });
    t.client.start();
    await waitFor(() => t.client.state.status === "reconnecting");
    expect(t.client.state.error?.code).toBe("lan_unavailable");
    expect(t.shown()).toBe("reconnecting");
    await waitFor(() => t.lan.discoveries >= 3);
    expect(t.cloud.http).toEqual([]);
    expect(t.cloud.relays).toHaveLength(0);
    // The host turns LAN access on: connected, still without the Cloud.
    t.lan.advertise(t.host);
    await waitFor(() => t.client.state.status === "live");
    expect(t.cloud.http).toEqual([]);
    t.client.stop();
  });
});

describe("cloud-only mode", () => {
  it("uses the relay and never looks on the LAN", async () => {
    const t = setup({ mode: "cloud" });
    await live(t);
    expect(t.manager.path).toBe("relay");
    expect(t.shown()).toBe("relay");
    t.cloud.cut();
    await waitFor(() => t.client.state.status === "live" && tickets(t) === 2);
    expect(t.lan.discoveries).toBe(0);
    expect(t.lan.dials).toEqual([]);
    t.client.stop();
  });
});

describe("path changes", () => {
  it("falls back to the relay when the LAN goes away (Wi-Fi to cellular)", async () => {
    const t = setup();
    await live(t);
    t.client.attachSession("s1");
    await waitFor(() => t.host.backend.channel.sent.some((m) => m.op === "session.subscribe"));
    let ch = t.host.backend.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 1 });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(2, "a") });
    await waitFor(() => events(t).includes(2));

    t.lan.stale = true; // the old advertisement lingers in the cache
    t.lan.leave();
    await waitFor(() => t.client.state.status === "live" && t.manager.path === "relay");
    expect(t.paths).toEqual(["lan", "relay"]);
    // The LAN was tried first, then the relay; the old path is closed.
    expect(t.lan.discoveries).toBe(2);
    expect(t.lan.openLinks).toHaveLength(0);
    expect(t.cloud.openRelays).toHaveLength(1);
    // A fresh baseline on the new path; the session resumes from its cursor.
    await waitFor(() => snapshots(t) === 2);
    await waitFor(() => t.host.backend.channel.sent.some((m) => m.op === "session.subscribe"));
    ch = t.host.backend.channel;
    expect(ch.sent.find((m) => m.op === "session.subscribe")).toMatchObject({ epoch: "e1", since_seq: 2 });
    ch.frame({ type: "replay", subscription_id: "sub-1", session_id: "s1", epoch: "e1", events: [delta(2, "a"), delta(3, "b")], high_water_seq: 3 });
    await waitFor(() => events(t).includes(3));
    expect(events(t)).toEqual([2, 3]);
    t.client.stop();
  });

  it("stays on a healthy relay when the LAN comes back, and prefers the LAN after a reconnect", async () => {
    const t = setup({ lanRetry: { baseMs: 1, maxMs: 1 } });
    t.lan.leave();
    await live(t);
    expect(t.manager.path).toBe("relay");

    t.lan.join(); // back on the same Wi-Fi
    await sleep(50);
    expect(t.manager.path).toBe("relay");
    expect(t.lan.discoveries).toBe(1);
    expect(await t.client.request("workspace.list", {})).toEqual([]);
    expect(t.lan.discoveries).toBe(1);

    t.cloud.cut();
    await waitFor(() => t.client.state.status === "live" && t.manager.path === "lan");
    expect(t.paths).toEqual(["relay", "lan"]);
    expect(t.cloud.openRelays).toHaveLength(0);
    t.client.stop();
  });

  it("does not look on the LAN at every relay reconnect", async () => {
    const t = setup({ onLan: false, lanRetry: { baseMs: 60_000, maxMs: 60_000 } });
    await live(t);
    for (let i = 0; i < 3; i++) {
      const before = tickets(t);
      t.cloud.cut();
      await waitFor(() => t.client.state.status === "live" && tickets(t) === before + 1);
    }
    expect(t.lan.discoveries).toBe(1);
    t.client.stop();
  });

  it("retries the LAN first after a LAN drop", async () => {
    const t = setup();
    await live(t);
    t.lan.cut();
    await waitFor(() => t.client.state.status === "live" && t.lan.links.length === 2);
    expect(t.manager.path).toBe("lan");
    expect(tickets(t)).toBe(0);
    t.client.stop();
  });

  it("recovers a host restart over the LAN through a new epoch, without duplicates", async () => {
    const t = setup();
    await live(t);
    t.host.backend.feedEvent("workspace.removed", { workspace_id: "w2" });
    await waitFor(() => t.feed.some((u) => u.type === "event"));

    t.host.restart("e2");
    await waitFor(() => t.client.state.status === "live" && snapshots(t) === 2);
    expect(t.manager.path).toBe("lan");
    const seen = t.feed.map((u) => (u.type === "event" ? `${u.event.seq}` : `snap:${u.snapshot.epoch}`));
    expect(seen).toEqual(["snap:e1", "1", "snap:e2"]);
    t.client.stop();
  });

  it("never resends a mutation interrupted on the LAN", async () => {
    const t = setup();
    await live(t);
    t.host.backend.handlers["session.send"] = () => new Promise(() => {});
    const send = t.client
      .sendMessage({ session_id: "s1", blocks: [{ type: "text", text: "hi" }] })
      .catch((e) => e);
    await waitFor(() => t.host.backend.count("session.send") === 1);
    t.lan.leave();
    expect(await send).toMatchObject({ code: "unknown_outcome" });
    await waitFor(() => t.client.state.status === "live" && t.manager.path === "relay");
    await sleep(30);
    expect(t.host.backend.count("session.send")).toBe(1);
    t.client.stop();
  });
});

describe("isolation", () => {
  it("connects only to the selected host among several on the LAN", async () => {
    const t = setup();
    const other = new FakeHost("host-2");
    other.trusted.add(t.device.publicKey);
    t.lan.advertise(other, "192.168.1.30");
    await live(t);
    expect(t.lan.dials).toEqual([`192.168.1.20:${t.listener!.port}`]);
    expect(other.handshakes).toBe(0);
    t.client.stop();
  });

  it("switching hosts closes the old connection; nothing carries over", async () => {
    const t = setup();
    const b = new FakeHost("host-2");
    b.trusted.add(t.device.publicKey);
    b.backend.handlers["workspace.list"] = () => [{ workspace_id: "wb", name: "b" }];
    t.lan.advertise(b, "192.168.1.30");
    const controllerA = new HostController(t.client);
    await live(t);
    t.host.backend.handlers["workspace.list"] = () => [{ workspace_id: "wa", name: "a" }];

    // Switch: stop A, then a new client and manager for B.
    controllerA.dispose();
    t.client.stop();
    expect(t.lan.openLinks).toHaveLength(0);
    const managerB = new ConnectionManager({
      mode: "auto",
      hostId: b.hostId,
      hostStaticPubkey: b.publicKey,
      deviceSecret: t.device.secret,
      lan: { discover: t.lan.discover, dial: t.lan.dial },
    });
    const clientB = new HostClient({ transport: managerB, expectedHostId: b.hostId, backoff: BACKOFF });
    const controllerB = new HostController(clientB);
    clientB.start();
    await waitFor(() => controllerB.store.getState().workspaces?.length === 1);
    expect(controllerB.store.getState().workspaces).toEqual([{ workspace_id: "wb", name: "b" }]);
    expect(t.lan.openLinks).toHaveLength(1);
    controllerB.dispose();
    clientB.stop();
  });

  it("drops a connection chosen after close()", async () => {
    const t = setup();
    const pending = t.manager.request("host.hello", {}).catch((e) => e);
    t.manager.close();
    expect(await pending).toMatchObject({ code: "network" });
    await sleep(10);
    expect(t.lan.openLinks).toHaveLength(0);
    expect(t.manager.path).toBeNull();
  });
});

describe("active connection", () => {
  it("shows the path only while live", () => {
    expect(activeConnection("live", "lan")).toBe("lan");
    expect(activeConnection("live", "relay")).toBe("relay");
    expect(activeConnection("connecting", "lan")).toBe("connecting");
    expect(activeConnection("handshaking", null)).toBe("connecting");
    expect(activeConnection("reconnecting", "relay")).toBe("reconnecting");
    for (const s of ["disconnected", "unauthenticated", "error"] as const) {
      expect(activeConnection(s, "lan")).toBe("offline");
    }
  });
});
