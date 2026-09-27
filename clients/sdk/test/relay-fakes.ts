// A fake Zwiim Cloud and a fake host that speaks the real tunnel: the host
// side of the Noise IK handshake and the envelope protocol, routing requests
// and event streams to a `FakeTransport` (the same scriptable backend the
// HostClient tests use).

import { x25519 } from "@noble/curves/ed25519.js";
import { ZwiimClientError } from "../src/client/errors";
import { encodeKey } from "../src/remote/keys";
import { concatBytes, respond, type CipherState } from "../src/remote/noise";
import { decodeEnvelope, tunnelPrologue, type ByteChannel, type Envelope } from "../src/remote/tunnel";
import type { EventChannel } from "../src/transport/transport";
import type { ClientMessage, KeyPairingInvitation, OpName } from "../src/types/protocol";
import { FakeTransport } from "./fakes";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const EMPTY: Uint8Array = new Uint8Array(0);

function frame(message: Uint8Array): Uint8Array {
  const out = new Uint8Array(2 + message.length);
  out[0] = message.length >> 8;
  out[1] = message.length & 0xff;
  out.set(message, 2);
  return out;
}

/** Length-prefixed frames out of a byte stream. */
class Frames {
  private buf = EMPTY;
  push(bytes: Uint8Array): Uint8Array[] {
    this.buf = concatBytes(this.buf, bytes);
    const out: Uint8Array[] = [];
    while (this.buf.length >= 2) {
      const len = (this.buf[0]! << 8) | this.buf[1]!;
      if (this.buf.length < 2 + len) break;
      out.push(this.buf.slice(2, 2 + len));
      this.buf = this.buf.slice(2 + len);
    }
    return out;
  }
}

// ─── the host ────────────────────────────────────────────────────────────

class HostTunnel {
  private readonly frames = new Frames();
  private send: CipherState | null = null;
  private recv: CipherState | null = null;
  private readonly streams = new Map<number, EventChannel>();
  private message: Uint8Array[] = [];
  devicePubkey: string | null = null;
  ended = false;

  constructor(
    private readonly host: FakeHost,
    private readonly channel: ByteChannel,
  ) {
    channel.listen({
      onData: (bytes) => {
        for (const f of this.frames.push(bytes)) this.onFrame(f);
      },
      onClose: () => this.end(false),
    });
  }

  private onFrame(f: Uint8Array): void {
    if (this.ended) return;
    if (!this.recv) return this.handshake(f);
    const plain = this.recv.decrypt(EMPTY, f);
    this.message.push(plain.subarray(1));
    if (plain[0] !== 1) return;
    const text = decoder.decode(concatBytes(...this.message));
    this.message = [];
    this.host.received.push(text);
    const envelope = decodeEnvelope(text);
    if (!envelope) return this.end(true);
    this.dispatch(envelope);
  }

  private handshake(msg1: Uint8Array): void {
    let first;
    try {
      first = respond({ prologue: tunnelPrologue(this.host.hostId), staticSecret: this.host.secret }, msg1);
    } catch {
      // Cannot read it (wrong host key or host id): end without replying.
      return this.end(true);
    }
    this.devicePubkey = encodeKey(first.remoteStatic);
    // A device pairing as it connects names a one-time code in its hello.
    const ok =
      first.payload.length > 0
        ? this.host.redeem(this.devicePubkey, decoder.decode(first.payload))
        : this.host.trusted.has(this.devicePubkey);
    const { message, session } = first.reply(encoder.encode(JSON.stringify({ ok })));
    this.channel.send(frame(message));
    if (!ok) return this.end(true);
    this.send = session.send;
    this.recv = session.recv;
    this.host.handshakes++;
  }

  write(envelope: Envelope): void {
    if (this.ended || !this.send) return;
    const message = encoder.encode(JSON.stringify(envelope));
    this.channel.send(frame(this.send.encrypt(EMPTY, concatBytes(Uint8Array.of(1), message))));
  }

  private dispatch(envelope: Envelope): void {
    switch (envelope.t) {
      case "req": {
        const { id, op, body } = envelope;
        Promise.resolve()
          .then(() => this.host.backend.request(op as OpName, body))
          .then(
            (result) => this.write({ t: "res", id, status: 200, body: result ?? null }),
            (e: unknown) => {
              const err = e instanceof ZwiimClientError ? e : new ZwiimClientError("internal", String(e));
              this.write({
                t: "res",
                id,
                status: err.status ?? 400,
                body: { error: { code: err.code, message: err.message, ...(err.details ? { details: err.details } : {}) } },
              });
            },
          );
        return;
      }
      case "ws.open": {
        const { id } = envelope;
        if (this.host.refuseStreams) {
          this.write({ t: "ws.open", id, status: 503, body: { error: { code: "internal", message: "no" } } });
          return;
        }
        const ch = this.host.backend.openEvents({
          onOpen: () => this.write({ t: "ws.open", id, status: 101 }),
          onFrame: (f) => this.write({ t: "ws.msg", id, data: JSON.stringify(f) }),
          onClose: ({ code, reason }) => {
            if (this.streams.delete(id)) this.write({ t: "ws.close", id, code, reason });
          },
        });
        this.streams.set(id, ch);
        (ch as { open?: () => void }).open?.();
        return;
      }
      case "ws.msg": {
        const ch = this.streams.get(envelope.id);
        ch?.send(JSON.parse(envelope.data) as ClientMessage);
        return;
      }
      case "ws.close": {
        const ch = this.streams.get(envelope.id);
        this.streams.delete(envelope.id);
        ch?.close();
        return;
      }
      default:
        this.end(true);
    }
  }

  /** Revocation: close every stream with 4403, then end the tunnel. */
  revoke(): void {
    for (const id of [...this.streams.keys()]) {
      this.streams.delete(id);
      this.write({ t: "ws.close", id, code: 4403, reason: "device_revoked" });
    }
    this.end(true);
  }

  end(closeChannel: boolean): void {
    if (this.ended) return;
    this.ended = true;
    const streams = [...this.streams.values()];
    this.streams.clear();
    for (const ch of streams) ch.close();
    if (closeChannel) this.channel.close();
  }
}

export class FakeHost {
  readonly secret = x25519.utils.randomSecretKey();
  readonly publicKey = encodeKey(x25519.getPublicKey(this.secret));
  readonly backend = new FakeTransport();
  readonly trusted = new Set<string>();
  readonly tunnels: HostTunnel[] = [];
  /** Every decrypted tunnel message the host received. */
  readonly received: string[] = [];
  handshakes = 0;
  refuseStreams = false;
  /** Outstanding pairing codes → expiry (ms). */
  readonly codes = new Map<string, number>();
  /** Devices paired by code: public key → device id and name. */
  readonly paired = new Map<string, { device_id: string; device_name: string | null }>();
  private nextCode = 0;

  constructor(readonly hostId = "host-1") {
    this.backend.handlers["host.hello"] = () => ({
      host_id: this.hostId,
      protocol: "zwiim/1",
      version: "0.0.0",
      owner: null,
    });
  }

  /** `device.pair_invite`: a v2 QR payload. */
  invite(ttlMs = 300_000, now = Date.now()): KeyPairingInvitation {
    const code = `code-${++this.nextCode}`;
    this.codes.set(code, now + ttlMs);
    return {
      v: 2,
      host_id: this.hostId,
      host_static_pubkey: this.publicKey,
      code,
      expires_at: new Date(now + ttlMs).toISOString(),
    };
  }

  /**
   * Redeem a pairing hello for `pubkey`: the code is spent on any attempt.
   * As the host, the answer is the pairing's alone, even for a trusted key.
   */
  redeem(pubkey: string, hello: string): boolean {
    let pair: { code?: unknown; device_id?: unknown; device_name?: unknown } | undefined;
    try {
      pair = (JSON.parse(hello) as { pair?: typeof pair }).pair;
    } catch {
      return false;
    }
    if (!pair || typeof pair.code !== "string" || typeof pair.device_id !== "string") return false;
    const expires = this.codes.get(pair.code);
    this.codes.delete(pair.code);
    if (expires === undefined || expires <= Date.now()) return false;
    this.trusted.add(pubkey);
    this.paired.set(pubkey, {
      device_id: pair.device_id,
      device_name: typeof pair.device_name === "string" ? pair.device_name : null,
    });
    return true;
  }

  accept(channel: ByteChannel): void {
    this.tunnels.push(new HostTunnel(this, channel));
  }

  get live(): HostTunnel[] {
    return this.tunnels.filter((t) => !t.ended);
  }

  revoke(devicePubkey: string): void {
    this.trusted.delete(devicePubkey);
    for (const t of this.live) if (t.devicePubkey === devicePubkey) t.revoke();
  }

  /** A restart: every tunnel ends and the feed starts a new epoch. */
  restart(epoch: string): void {
    for (const t of this.live) t.end(true);
    this.backend.feedEpoch = epoch;
    this.backend.feedSeq = 0;
  }
}

// ─── the Cloud ───────────────────────────────────────────────────────────

interface Relay {
  device: FakeWebSocket;
  hostEnd: ByteChannel;
  hostHandlers: Parameters<ByteChannel["listen"]>[0] | null;
  hostQueue: Uint8Array[];
  closed: boolean;
}

export class FakeCloud {
  readonly origin = "https://cloud.test";
  /** account token → user id */
  readonly accounts = new Map<string, string>([["token-u1", "u1"]]);
  readonly hosts = new Map<string, { owner: string; host: FakeHost; online: boolean; name: string | null }>();
  readonly tickets = new Map<string, string>();
  readonly relays: Relay[] = [];
  /** Every binary message the relay moved. */
  readonly relayed: Uint8Array[] = [];
  readonly http: { method: string; path: string; authorization: string | null }[] = [];
  readonly hellos: string[] = [];
  down = false;
  private nextTicket = 0;

  addHost(host: FakeHost, owner = "u1", name: string | null = null): void {
    this.hosts.set(host.hostId, { owner, host, online: true, name });
  }

  setOnline(hostId: string, online: boolean): void {
    const h = this.hosts.get(hostId)!;
    h.online = online;
    if (!online) for (const r of this.relays) this.cutRelay(r, 1000);
  }

  get openRelays(): Relay[] {
    return this.relays.filter((r) => !r.closed);
  }

  /** Drop every relay: the device sees `code`, the host a clean close. */
  cut(code = 1006): void {
    for (const r of this.openRelays) this.cutRelay(r, code);
  }

  private cutRelay(r: Relay, deviceCode: number): void {
    if (r.closed) return;
    r.closed = true;
    r.device.remoteClose(deviceCode, "");
    r.hostHandlers?.onClose({ code: 1000, reason: "" });
  }

  readonly fetch = (async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const url = new URL(String(input));
    const method = init?.method ?? "GET";
    const authorization = new Headers(init?.headers).get("authorization");
    this.http.push({ method, path: url.pathname, authorization });
    if (this.down) throw new TypeError("fetch failed");
    const json = (status: number, body: unknown) =>
      new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    const error = (status: number, code: string) => json(status, { error: { code, message: code } });
    const user = this.accounts.get(authorization?.replace(/^Bearer /, "") ?? "");
    if (!user) return error(401, "unauthenticated");
    if (method === "GET" && url.pathname === "/v1/hosts") {
      return json(
        200,
        [...this.hosts.values()]
          .filter((h) => h.owner === user)
          .map((h) => ({
            host_id: h.host.hostId,
            name: h.name,
            static_pubkey: h.host.publicKey,
            online: h.online,
            last_seen_at: 1,
          })),
      );
    }
    const m = /^\/v1\/hosts\/([^/]+)\/tickets$/.exec(url.pathname);
    if (method === "POST" && m) {
      const h = this.hosts.get(m[1]!);
      if (!h || h.owner !== user) return error(404, "not_found");
      if (!h.online) return error(503, "host_offline");
      const ticket = `ticket-${++this.nextTicket}`;
      this.tickets.set(ticket, h.host.hostId);
      return json(200, { ticket, expires_at: 0 });
    }
    if (method === "POST" && url.pathname === "/v1/enrollments") {
      return json(200, { enrollment_token: "enroll-1", expires_at: 0 });
    }
    return error(404, "not_found");
  }) as typeof fetch;

  get WebSocket(): new (url: string) => WebSocket {
    const cloud = this;
    return class extends FakeWebSocket {
      constructor(url: string) {
        super(cloud, url);
      }
    } as unknown as new (url: string) => WebSocket;
  }

  /** @internal */
  hello(ws: FakeWebSocket, text: string): void {
    this.hellos.push(text);
    let ticket: unknown;
    try {
      const v = JSON.parse(text);
      ticket = v.type === "device" ? v.ticket : undefined;
    } catch {
      ticket = undefined;
    }
    const hostId = typeof ticket === "string" ? this.tickets.get(ticket) : undefined;
    if (typeof ticket === "string") this.tickets.delete(ticket);
    if (!hostId) return ws.remoteClose(4401, "unauthenticated");
    const h = this.hosts.get(hostId)!;
    if (!h.online) return ws.remoteClose(4503, "host_offline");
    const relay: Relay = {
      device: ws,
      hostHandlers: null,
      hostQueue: [],
      closed: false,
      hostEnd: {
        send: (bytes) => {
          if (relay.closed) return;
          this.relayed.push(bytes);
          ws.deliver(bytes);
        },
        close: () => {
          if (relay.closed) return;
          relay.closed = true;
          ws.remoteClose(1000, "");
        },
        listen: (handlers) => {
          relay.hostHandlers = handlers;
          for (const b of relay.hostQueue.splice(0)) handlers.onData(b);
        },
      },
    };
    ws.relay = relay;
    this.relays.push(relay);
    h.host.accept(relay.hostEnd);
    ws.deliverText(JSON.stringify({ type: "ready" }));
  }

  /** @internal Device → host bytes. */
  toHost(relay: Relay, bytes: Uint8Array): void {
    if (relay.closed) return;
    this.relayed.push(bytes);
    if (relay.hostHandlers) relay.hostHandlers.onData(bytes);
    else relay.hostQueue.push(bytes);
  }

  /** @internal The device closed its socket. */
  deviceClosed(relay: Relay): void {
    if (relay.closed) return;
    relay.closed = true;
    relay.hostHandlers?.onClose({ code: 1000, reason: "" });
  }
}

/** The device's relay socket. Delivery is asynchronous, like a network. */
class FakeWebSocket {
  binaryType = "blob";
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: ((ev: { code: number; reason: string }) => void) | null = null;
  relay: Relay | null = null;
  private helloSent = false;

  constructor(
    private readonly cloud: FakeCloud,
    readonly url: string,
  ) {
    setTimeout(() => {
      if (cloud.down) {
        this.readyState = 3;
        this.onerror?.();
        this.onclose?.({ code: 1006, reason: "" });
        return;
      }
      this.readyState = 1;
      this.onopen?.();
    }, 0);
  }

  send(data: string | Uint8Array): void {
    if (this.readyState !== 1) return;
    if (!this.helloSent) {
      this.helloSent = true;
      const text = String(data);
      setTimeout(() => this.cloud.hello(this, text), 0);
      return;
    }
    if (typeof data === "string") return this.remoteClose(4400, "malformed");
    const relay = this.relay;
    const copy = data.slice();
    if (relay) setTimeout(() => this.cloud.toHost(relay, copy), 0);
  }

  close(): void {
    if (this.readyState >= 2) return;
    this.readyState = 3;
    const relay = this.relay;
    if (relay) setTimeout(() => this.cloud.deviceClosed(relay), 0);
    setTimeout(() => this.onclose?.({ code: 1000, reason: "" }), 0);
  }

  deliver(bytes: Uint8Array): void {
    const copy = bytes.slice();
    setTimeout(() => {
      if (this.readyState === 1) this.onmessage?.({ data: copy.buffer });
    }, 0);
  }

  deliverText(text: string): void {
    setTimeout(() => {
      if (this.readyState === 1) this.onmessage?.({ data: text });
    }, 0);
  }

  /** The Cloud closes: after what it already sent is delivered. */
  remoteClose(code: number, reason: string): void {
    setTimeout(() => {
      if (this.readyState >= 2) return;
      this.readyState = 3;
      this.onclose?.({ code, reason });
    }, 0);
  }
}

/** Poll until `cond` holds (real timers). */
export async function waitFor(cond: () => boolean, timeoutMs = 2000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error("waitFor timed out");
    await new Promise((r) => setTimeout(r, 2));
  }
}
