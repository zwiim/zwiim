import { describe, expect, it } from "vitest";
import { isClientError, ZwiimClientError } from "../src/client/errors";
import { HostController } from "../src/client/controller";
import { HostClient, type HostFeedUpdate } from "../src/client/host-client";
import { CloudClient, normalizeCloudUrl, RelayQuotaError, relayCloseError } from "../src/remote/cloud";
import { generateDeviceKeys } from "../src/remote/keys";
import type { SessionFrame } from "../src/state/session";
import { LocalTransport } from "../src/transport/local";
import { RelayTransport } from "../src/transport/relay";
import { createTransport } from "../src/transport/select";
import type { SessionEvent } from "../src/types/protocol";
import { FakeCloud, FakeHost, waitFor } from "./relay-fakes";

const BACKOFF = { baseMs: 5, maxMs: 20 };

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

const delta = (seq: number, text: string): SessionEvent => ({
  session_id: "s1",
  seq,
  type: "agent.message_delta",
  payload: { text },
});

function setup(opts: { token?: string | (() => string); hostId?: string; trusted?: boolean } = {}) {
  const cloud = new FakeCloud();
  const host = new FakeHost();
  cloud.addHost(host);
  const device = generateDeviceKeys();
  if (opts.trusted !== false) host.trusted.add(device.publicKey);
  const cloudClient = new CloudClient({
    url: cloud.origin,
    accessToken: opts.token ?? "token-u1",
    fetch: cloud.fetch,
    WebSocket: cloud.WebSocket,
  });
  const transport = new RelayTransport({
    cloud: cloudClient,
    hostId: opts.hostId ?? host.hostId,
    hostStaticPubkey: host.publicKey,
    deviceSecret: device.secret,
    timeoutMs: 2000,
  });
  const client = new HostClient({ transport, backoff: BACKOFF, random: () => 0.5 });
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
  return { cloud, host, device, cloudClient, transport, client, statuses, frames, feed };
}

type T = ReturnType<typeof setup>;

async function live(t: T) {
  t.client.start();
  await waitFor(() => t.client.state.status === "live" && t.feed.some((u) => u.type === "snapshot"));
}

const tickets = (t: T) => t.cloud.http.filter((r) => r.path.endsWith("/tickets")).length;
const events = (t: T) =>
  t.frames.flatMap(([, f]) =>
    f.type === "event" ? [f.event.seq] : f.type === "replay" ? f.events.map((e) => e.seq) : [],
  );

describe("transport selection", () => {
  it("builds a local or a relay transport; the client above is the same", async () => {
    const t = setup();
    const local = createTransport({ kind: "local", url: "http://127.0.0.1:1", token: "x" });
    expect(local).toBeInstanceOf(LocalTransport);
    const relay = createTransport({
      kind: "relay",
      cloud: t.cloudClient,
      hostId: t.host.hostId,
      hostStaticPubkey: t.host.publicKey,
      deviceSecret: t.device.secret,
    });
    expect(relay).toBeInstanceOf(RelayTransport);

    // The same operation, the same public result, through either transport.
    t.host.backend.handlers["workspace.list"] = () => [{ workspace_id: "w1", name: "repo" }];
    const viaLocal = new LocalTransport({
      url: "http://127.0.0.1:1",
      token: "x",
      fetch: (async (url: string, init: RequestInit) => {
        const op = String(url).split("/").pop() as "workspace.list";
        const body = await t.host.backend.request(op, JSON.parse(String(init.body)));
        return new Response(JSON.stringify(body), { status: 200 });
      }) as typeof fetch,
      // The local event socket, bridged to the same backend.
      WebSocket: class {
        readyState = 0;
        onopen: (() => void) | null = null;
        onmessage: ((ev: { data: string }) => void) | null = null;
        onclose: ((ev: { code: number; reason: string }) => void) | null = null;
        onerror: (() => void) | null = null;
        private readonly ch = t.host.backend.openEvents({
          onOpen: () => {
            this.readyState = 1;
            this.onopen?.();
          },
          onFrame: (f) => this.onmessage?.({ data: JSON.stringify(f) }),
          onClose: (info) => this.onclose?.(info),
        });
        constructor() {
          setTimeout(() => (this.ch as unknown as { open(): void }).open(), 0);
        }
        send(text: string) {
          this.ch.send(JSON.parse(text));
        }
        close() {
          this.ch.close();
        }
      } as never,
    });
    for (const transport of [viaLocal, relay]) {
      const client = new HostClient({ transport, backoff: BACKOFF });
      const controller = new HostController(client);
      client.start();
      await waitFor(() => client.state.status === "live");
      expect(await client.request("workspace.list", {})).toEqual([{ workspace_id: "w1", name: "repo" }]);
      await waitFor(() => controller.store.getState().workspaces?.length === 1);
      controller.dispose();
      client.stop();
    }
  });
});

describe("Cloud client", () => {
  it("normalizes Cloud URLs like the host", () => {
    expect(normalizeCloudUrl(" https://Cloud.Example.com/ ")).toBe("https://cloud.example.com");
    expect(normalizeCloudUrl("http://127.0.0.1:8080")).toBe("http://127.0.0.1:8080");
    for (const bad of ["http://cloud.example.com", "https://u:p@c.com", "https://c.com/x", "https://c.com?a", "ftp://c.com", "nope"]) {
      expect(() => normalizeCloudUrl(bad), bad).toThrow(ZwiimClientError);
    }
  });

  it("authenticates to the Cloud with the account token, and only to the Cloud", async () => {
    let n = 0;
    const t = setup({
      token: () => {
        n++;
        return "token-u1";
      },
    });
    expect(await t.cloudClient.hosts()).toEqual([
      { host_id: "host-1", name: null, static_pubkey: t.host.publicKey, online: true, last_seen_at: 1 },
    ]);
    await live(t);
    await t.client.request("workspace.list", {});
    // Every Cloud call carried the token, fetched fresh each time.
    expect(t.cloud.http.every((r) => r.authorization === "Bearer token-u1")).toBe(true);
    expect(n).toBe(t.cloud.http.length);
    // The host never saw it, and the relay only moved ciphertext.
    expect(t.host.received.join("\n")).not.toContain("token-u1");
    const wire = new TextDecoder("latin1").decode(
      new Uint8Array(t.cloud.relayed.flatMap((b) => Array.from(b))),
    );
    expect(wire).not.toContain("token-u1");
    expect(wire).not.toContain("workspace.list");
    expect(wire).not.toContain("host.subscribe");
    // The relay hello carried the ticket and nothing else.
    expect(JSON.parse(t.cloud.hellos[0]!)).toEqual({ type: "device", ticket: "ticket-1" });
    t.client.stop();
  });

  it("stops at a rejected account token", async () => {
    const t = setup({ token: "expired" });
    t.client.start();
    await waitFor(() => t.client.state.status === "unauthenticated");
    expect(t.client.state.error?.code).toBe("unauthenticated");
    await expect(t.cloudClient.hosts()).rejects.toMatchObject({ code: "unauthenticated" });
    const calls = t.cloud.http.length;
    await new Promise((r) => setTimeout(r, 60));
    expect(t.cloud.http.length).toBe(calls); // not retried
  });
});

describe("Cloud usage and limits", () => {
  const requests: { path: string; body: string | null; type: string | null }[] = [];
  const cloudAnswering = (status: number, body: unknown) =>
    new CloudClient({
      url: "https://cloud.test",
      accessToken: "t",
      fetch: (async (input: RequestInfo | URL, init?: RequestInit) => {
        const headers = new Headers(init?.headers);
        requests.push({
          path: new URL(String(input)).pathname,
          body: typeof init?.body === "string" ? init.body : null,
          type: headers.get("content-type"),
        });
        return new Response(JSON.stringify(body), { status });
      }) as typeof fetch,
    });
  const quota = {
    code: "relay_quota_exceeded",
    message: "m",
    channel: "browser",
    plan: "free",
    used_bytes: 5368709120,
    limit_bytes: 5368709120,
    resets_at: 1797033600,
  };

  it("asks for a browser ticket by channel, and a control ticket as before", async () => {
    requests.length = 0;
    const cloud = cloudAnswering(200, { ticket: "k", expires_at: 0 });
    await cloud.ticket("host-1");
    await cloud.ticket("host-1", "browser");
    expect(requests).toEqual([
      { path: "/v1/hosts/host-1/tickets", body: null, type: null },
      { path: "/v1/hosts/host-1/tickets", body: '{"channel":"browser"}', type: "application/json" },
    ]);
  });

  it("turns a refused ticket into a RelayQuotaError with its limit", async () => {
    const error = await cloudAnswering(429, { error: quota }).ticket("host-1", "browser").catch((e) => e);
    expect(error).toBeInstanceOf(RelayQuotaError);
    expect(error).toMatchObject({
      code: "relay_quota_exceeded",
      status: 429,
      limit: "browser",
      plan: "free",
      usedBytes: 5368709120,
      limitBytes: 5368709120,
      resetsAt: 1797033600,
    });
    expect(isClientError(error, "relay_quota_exceeded")).toBe(true);
    // Other 429s are the Cloud's request rate limit.
    await expect(
      cloudAnswering(429, { error: { code: "rate_limited", message: "m" } }).ticket("host-1"),
    ).rejects.toMatchObject({ code: "cloud_unavailable" });
  });

  it("names a relay closed at a limit", () => {
    expect(relayCloseError({ code: 4429, reason: "relay_quota_exceeded" }).code).toBe("relay_quota_exceeded");
  });

  it("reads the account's usage, or no plan", async () => {
    const usage = {
      plan: { id: "free", name: "Free" },
      period_start: 1794441600,
      resets_at: 1797033600,
      channels: { browser: { used: 1, limit: 5368709120 }, total: { used: 2, limit: null } },
    };
    expect(await cloudAnswering(200, usage).usage()).toEqual(usage);
    expect(await cloudAnswering(200, { plan: null }).usage()).toEqual({ plan: null });
    await expect(cloudAnswering(200, []).usage()).rejects.toMatchObject({ code: "invalid_response" });
  });
});

describe("relay connection", () => {
  it("takes a ticket, dials the relay, and completes the Noise handshake", async () => {
    const t = setup();
    await live(t);
    expect(tickets(t)).toBe(1);
    expect(t.host.handshakes).toBe(1);
    expect(t.transport.connected).toBe(true);
    expect(t.statuses).toEqual(["connecting", "handshaking", "live"]);
    // One tunnel carries every request and the event stream.
    await Promise.all([t.client.request("workspace.list", {}), t.client.request("terminal.list", {})]);
    expect(t.cloud.relays.length).toBe(1);
    t.client.stop();
    await waitFor(() => t.cloud.openRelays.length === 0);
    expect(t.transport.connected).toBe(false);
  });

  it("uses each ticket once and a fresh one per connection", async () => {
    const t = setup();
    await live(t);
    t.cloud.cut();
    await waitFor(() => t.host.handshakes === 2 && t.client.state.status === "live");
    expect(tickets(t)).toBe(2);
    expect(t.cloud.tickets.size).toBe(0);
    const hellos = t.cloud.hellos.map((h) => JSON.parse(h).ticket);
    expect(new Set(hellos).size).toBe(hellos.length);
    t.client.stop();
  });

  it("multiplexes concurrent requests and matches answers out of order", async () => {
    const t = setup();
    await live(t);
    const gates: (() => void)[] = [];
    t.host.backend.handlers["files.read"] = (p: { path: string }) =>
      new Promise((resolve) => gates.push(() => resolve({ path: p.path })));
    const reads = ["a", "b", "c"].map((path) =>
      t.client.request("files.read", { workspace_id: "w1", path } as never),
    );
    await waitFor(() => gates.length === 3);
    for (const open of gates.reverse()) open();
    expect(await Promise.all(reads)).toEqual([{ path: "a" }, { path: "b" }, { path: "c" }]);
    t.client.stop();
  });

  it("maps host errors exactly as the local transport does", async () => {
    const t = setup();
    await live(t);
    t.host.backend.handlers["workspace.remove_worktree"] = () => {
      throw new ZwiimClientError("workspace_busy", "busy", 409);
    };
    const err = await t.client
      .request("workspace.remove_worktree", { workspace_id: "w1" } as never)
      .catch((e) => e);
    expect(err).toMatchObject({ code: "workspace_busy", status: 409, message: "busy" });
    t.client.stop();
  });

  it("delivers the event stream: feed and session events", async () => {
    const t = setup();
    await live(t);
    t.host.backend.feedEvent("workspace.removed", { workspace_id: "w2" });
    await waitFor(() => t.feed.some((u) => u.type === "event"));
    t.client.attachSession("s1");
    await waitFor(() => t.host.backend.channel.sent.some((m) => m.op === "session.subscribe"));
    const ch = t.host.backend.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 1 });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(2, "hi") });
    await waitFor(() => events(t).includes(2));
    expect(t.frames.map(([, f]) => f.type)).toEqual(["attaching", "snapshot", "event"]);
    t.client.stop();
  });
});

describe("reconnect", () => {
  it("resubscribes the feed and resumes sessions from their cursor, without duplicates", async () => {
    const t = setup();
    await live(t);
    t.client.attachSession("s1");
    await waitFor(() => t.host.backend.channel.sent.some((m) => m.op === "session.subscribe"));
    let ch = t.host.backend.channel;
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 1 });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(2, "a") });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(2, "a") }); // duplicate
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(3, "b") });
    await waitFor(() => events(t).includes(3));

    t.cloud.cut();
    await waitFor(() => t.client.state.status === "reconnecting");
    await waitFor(
      () =>
        t.client.state.status === "live" &&
        t.host.backend.channels.length === 2 &&
        t.host.backend.channel.sent.some((m) => m.op === "session.subscribe"),
    );
    ch = t.host.backend.channel;
    expect(ch.sent.map((m) => m.op)).toContain("host.subscribe");
    expect(ch.sent.find((m) => m.op === "session.subscribe")).toMatchObject({ epoch: "e1", since_seq: 3 });
    // The replay overlaps what was already delivered; only 4 is new.
    ch.frame({
      type: "replay",
      subscription_id: "sub-1",
      session_id: "s1",
      epoch: "e1",
      events: [delta(3, "b"), delta(4, "c")],
      high_water_seq: 4,
    });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(4, "c") });
    ch.frame({ type: "event", subscription_id: "sub-1", event: delta(5, "d") });
    await waitFor(() => events(t).includes(5));
    expect(events(t)).toEqual([2, 3, 4, 5]);
    t.client.stop();
  });

  it("recovers a host restart through the new epoch's snapshots", async () => {
    const t = setup();
    await live(t);
    t.client.attachSession("s1");
    await waitFor(() => t.host.backend.channel.sent.some((m) => m.op === "session.subscribe"));
    t.host.backend.channel.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e1", snapshot, event_seq: 5 });
    await waitFor(() => t.frames.some(([, f]) => f.type === "snapshot"));

    t.host.restart("e2");
    await waitFor(() => t.client.state.status === "live" && t.host.backend.channels.length === 2);
    await waitFor(() => t.feed.filter((u) => u.type === "snapshot").length === 2);
    expect(t.feed.filter((u) => u.type === "snapshot").map((u) => (u as any).snapshot.epoch)).toEqual(["e1", "e2"]);
    // The session's old cursor is answered from another epoch: start over.
    await waitFor(() => t.host.backend.channel.sent.some((m) => m.op === "session.subscribe"));
    const ch = t.host.backend.channel;
    ch.frame({ type: "replay", subscription_id: "sub-1", session_id: "s1", epoch: "e2", events: [delta(1, "x")], high_water_seq: 1 });
    await waitFor(() => ch.sent.filter((m) => m.op === "session.subscribe").length === 2);
    expect(ch.sent.filter((m) => m.op === "session.subscribe")[1]).not.toHaveProperty("since_seq");
    ch.frame({ type: "snapshot", subscription_id: "sub-1", session_id: "s1", epoch: "e2", snapshot, event_seq: 1 });
    await waitFor(() => t.frames.filter(([, f]) => f.type === "snapshot").length === 2);
    expect(events(t)).toEqual([]); // the stale replay was not delivered
    t.client.stop();
  });

  it("takes a new feed snapshot when the cursor is stale (a gap in seq)", async () => {
    const t = setup();
    await live(t);
    const ch = t.host.backend.channel;
    t.host.backend.feedEvent("workspace.removed", { workspace_id: "w2" });
    t.host.backend.feedSeq += 2; // two events the client never gets
    t.host.backend.feedEvent("workspace.removed", { workspace_id: "w3" });
    await waitFor(() => ch.sent.filter((m) => m.op === "host.subscribe").length === 2);
    await waitFor(() => t.feed.filter((u) => u.type === "snapshot").length === 2);
    expect(t.feed.map((u) => (u.type === "event" ? u.event.seq : `snap@${u.snapshot.seq}`))).toEqual([
      "snap@0",
      1,
      "snap@4",
    ]);
    t.client.stop();
  });

  it("never resends a mutation whose outcome is unknown", async () => {
    const t = setup();
    await live(t);
    t.host.backend.handlers["session.send"] = () => new Promise(() => {}); // applied, never answered
    t.host.backend.handlers["git.status"] = () => new Promise(() => {});
    const send = t.client.sendMessage({ session_id: "s1", blocks: [{ type: "text", text: "hi" }] }).catch((e) => e);
    const read = t.client.request("git.status", { workspace_id: "w1" } as never).catch((e) => e);
    await waitFor(() => t.host.backend.count("session.send") === 1 && t.host.backend.count("git.status") === 1);
    t.cloud.cut();
    expect(await send).toMatchObject({ code: "unknown_outcome" });
    expect(await read).toMatchObject({ code: "network" });
    await waitFor(() => t.client.state.status === "live" && t.host.handshakes === 2);
    await new Promise((r) => setTimeout(r, 30));
    expect(t.host.backend.count("session.send")).toBe(1);
    t.client.stop();
  });

  it("fails a request that cannot reach the host as not sent", async () => {
    const t = setup();
    await live(t);
    t.client.stop();
    t.cloud.down = true;
    const err = await t.client.request("session.start", { workspace_id: "w1", agent: "claude_code", blocks: [] } as never).catch((e) => e);
    expect(err).toMatchObject({ code: "cloud_unavailable" }); // never `unknown_outcome`: nothing was sent
    expect(t.host.backend.count("session.start")).toBe(0);
  });
});

describe("failures", () => {
  it("waits out an offline host and connects when it returns", async () => {
    const t = setup();
    t.cloud.setOnline(t.host.hostId, false);
    t.client.start();
    await waitFor(() => t.client.state.error?.code === "host_offline" && t.client.state.attempt >= 2);
    expect(t.client.state.status).toBe("reconnecting");
    t.cloud.setOnline(t.host.hostId, true);
    await waitFor(() => t.client.state.status === "live");
    t.client.stop();
  });

  it("maps a relay closed as host_offline", async () => {
    const t = setup();
    const { ticket } = await t.cloudClient.ticket(t.host.hostId);
    t.cloud.setOnline(t.host.hostId, false);
    await expect(t.cloudClient.dial(ticket)).rejects.toMatchObject({ code: "host_offline" });
    await expect(t.cloudClient.dial("ticket-bogus")).rejects.toMatchObject({ code: "network" });
  });

  it("stops for good when the host revokes the device", async () => {
    const t = setup();
    await live(t);
    t.host.revoke(t.device.publicKey);
    await waitFor(() => t.client.state.status === "unauthenticated");
    expect(t.client.state.error?.code).toBe("device_refused");
    const handshakes = t.cloud.relays.length;
    await new Promise((r) => setTimeout(r, 60));
    expect(t.cloud.relays.length).toBe(handshakes);
  });

  it("refuses an untrusted device at the handshake", async () => {
    const t = setup({ trusted: false });
    t.client.start();
    await waitFor(() => t.client.state.status === "unauthenticated");
    expect(t.client.state.error?.code).toBe("device_refused");
    expect(t.host.received).toEqual([]); // no request ever reached the host
  });

  it("stops at a host the account does not have", async () => {
    for (const hostId of ["host-9", "bad id!"]) {
      const t = setup({ hostId });
      t.client.start();
      await waitFor(() => t.client.state.status === "error");
      expect(t.client.state.error?.code).toBe("host_unknown");
    }
    // Another account's host looks the same as a missing one.
    const t = setup();
    t.cloud.accounts.set("token-u2", "u2");
    const other = new CloudClient({ url: t.cloud.origin, accessToken: "token-u2", fetch: t.cloud.fetch, WebSocket: t.cloud.WebSocket });
    expect(await other.hosts()).toEqual([]);
    await expect(other.ticket(t.host.hostId)).rejects.toMatchObject({ code: "host_unknown" });
  });

  it("reconnects after a relay disconnect and closes the event stream cleanly", async () => {
    const t = setup();
    await live(t);
    const first = t.host.backend.channel;
    t.cloud.cut(1006);
    await waitFor(() => first.closed);
    await waitFor(() => t.client.state.status === "live" && t.host.backend.channels.length === 2);
    expect(t.statuses).toEqual(["connecting", "handshaking", "live", "reconnecting", "handshaking", "live"]);
    t.client.stop();
    await waitFor(() => t.host.backend.channel.closed);
  });

  it("rides out a Cloud outage without losing client state", async () => {
    const t = setup();
    const controller = new HostController(t.client);
    t.host.backend.handlers["workspace.list"] = () => [{ workspace_id: "w1", name: "repo" }];
    await live(t);
    await waitFor(() => controller.store.getState().workspaces?.length === 1);
    t.cloud.down = true;
    t.cloud.cut(1012);
    await waitFor(() => t.client.state.error?.code === "cloud_unavailable" && t.client.state.attempt >= 2);
    expect(t.client.state.status).toBe("reconnecting");
    expect(controller.store.getState().workspaces?.map((w) => w.workspace_id)).toEqual(["w1"]);
    t.cloud.down = false;
    await waitFor(() => t.client.state.status === "live");
    await waitFor(() => t.feed.filter((u) => u.type === "snapshot").length === 2);
    expect(controller.store.getState().workspaces?.map((w) => w.workspace_id)).toEqual(["w1"]);
    controller.dispose();
    t.client.stop();
  });

  it("closes an event stream the host refuses; the client retries", async () => {
    const t = setup();
    t.host.refuseStreams = true;
    t.client.start();
    await waitFor(() => t.client.state.status === "reconnecting");
    t.host.refuseStreams = false;
    await waitFor(() => t.client.state.status === "live");
    t.client.stop();
  });

  it("classifies client errors", () => {
    expect(isClientError(new ZwiimClientError("host_offline", ""), "host_offline")).toBe(true);
  });
});
