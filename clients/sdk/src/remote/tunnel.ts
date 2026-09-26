// The R2 device tunnel over a byte channel (in production, a Cloud relay
// socket): the Noise IK handshake, the encrypted and chunked message layer,
// and the envelopes that carry `/zwiim/v1` requests and event streams.
// Byte-compatible with the host's `tunnel::{noise, envelope}`.
//
//   on the wire:  u16 big-endian length ‖ Noise message        (length > 0)
//   plaintext:    flag ‖ chunk      flag 1 = final chunk, 0 = more follow
//   message:      one JSON envelope (req/res/ws.open/ws.msg/ws.close)
//
// The relay (and the Cloud behind it) only ever sees the wire bytes.

import { ZwiimClientError } from "../client/errors";
import { concatBytes, initiate, TAG_LEN, type CipherState, type NoiseSession } from "./noise";

/** Bound into the handshake with the host id (`tunnel::noise::PROLOGUE`). */
const PROLOGUE = "zwiim-tunnel/1";
const MAX_NOISE_MESSAGE = 65535;
const MAX_CHUNK = MAX_NOISE_MESSAGE - TAG_LEN - 1;
const CHUNK_MORE = 0;
const CHUNK_FINAL = 1;
/** Largest tunnel message, after reassembly. */
export const MAX_MESSAGE_LEN = 16 * 1024 * 1024;

const EMPTY: Uint8Array = new Uint8Array(0);
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

/** The prologue both ends bind: `zwiim-tunnel/1 ‖ 0 ‖ host_id`. */
export function tunnelPrologue(hostId: string): Uint8Array {
  return concatBytes(encoder.encode(PROLOGUE), Uint8Array.of(0), encoder.encode(hostId));
}

/**
 * An ordered, reliable byte stream (a relay socket). Chunk boundaries carry
 * no meaning. `onClose` fires once, whichever end closed.
 */
export interface ByteChannel {
  send(bytes: Uint8Array): void;
  close(): void;
  /** Start delivery; bytes that arrived earlier are delivered first. */
  listen(handlers: {
    onData(bytes: Uint8Array): void;
    onClose(info: { code: number; reason: string }): void;
  }): void;
}

/** Splits a byte stream into length-prefixed frames. */
class FrameReader {
  private buf = EMPTY;

  /** Append bytes; returns the complete frames. Throws on a zero length. */
  push(bytes: Uint8Array): Uint8Array[] {
    this.buf = this.buf.length ? concatBytes(this.buf, bytes) : bytes;
    const frames: Uint8Array[] = [];
    let at = 0;
    while (this.buf.length - at >= 2) {
      const len = (this.buf[at]! << 8) | this.buf[at + 1]!;
      if (len === 0) throw new Error("malformed frame");
      if (this.buf.length - at - 2 < len) break;
      frames.push(this.buf.slice(at + 2, at + 2 + len));
      at += 2 + len;
    }
    this.buf = this.buf.subarray(at);
    return frames;
  }

  /** The bytes of an incomplete frame, handing them over. */
  takeRest(): Uint8Array {
    const rest = this.buf;
    this.buf = EMPTY;
    return rest;
  }
}

function frame(message: Uint8Array): Uint8Array {
  const out = new Uint8Array(2 + message.length);
  out[0] = message.length >> 8;
  out[1] = message.length & 0xff;
  out.set(message, 2);
  return out;
}

// ─── envelopes ───────────────────────────────────────────────────────────

export type Envelope =
  | { t: "req"; id: number; op: string; body: unknown }
  | { t: "res"; id: number; status: number; body: unknown }
  | { t: "ws.open"; id: number; status?: number; body?: unknown }
  | { t: "ws.msg"; id: number; data: string }
  | { t: "ws.close"; id: number; code?: number; reason?: string };

const FIELDS: Record<Envelope["t"], { required: string[]; optional: string[] }> = {
  req: { required: ["id", "op"], optional: ["body"] },
  res: { required: ["id", "status", "body"], optional: [] },
  "ws.open": { required: ["id"], optional: ["status", "body"] },
  "ws.msg": { required: ["id", "data"], optional: [] },
  "ws.close": { required: ["id"], optional: ["code", "reason"] },
};

const isU64 = (v: unknown) => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const isU16 = (v: unknown) => isU64(v) && (v as number) <= 0xffff;

/** Strict decode, as the host's (`deny_unknown_fields`); null if malformed. */
export function decodeEnvelope(text: string): Envelope | null {
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return null;
  }
  if (!v || typeof v !== "object" || Array.isArray(v)) return null;
  const o = v as Record<string, unknown>;
  const spec = typeof o.t === "string" ? FIELDS[o.t as Envelope["t"]] : undefined;
  if (!spec) return null;
  for (const key of Object.keys(o)) {
    if (key !== "t" && !spec.required.includes(key) && !spec.optional.includes(key)) return null;
  }
  for (const key of spec.required) if (!(key in o)) return null;
  if (!isU64(o.id)) return null;
  switch (o.t) {
    case "req":
      return typeof o.op === "string" ? (o as Envelope) : null;
    case "res":
      return isU16(o.status) ? (o as Envelope) : null;
    case "ws.open":
      return o.status === undefined || isU16(o.status) ? (o as Envelope) : null;
    case "ws.msg":
      return typeof o.data === "string" ? (o as Envelope) : null;
    case "ws.close":
      return (o.code === undefined || isU16(o.code)) &&
        (o.reason === undefined || typeof o.reason === "string")
        ? (o as Envelope)
        : null;
  }
  return null;
}

// ─── handshake ───────────────────────────────────────────────────────────

export interface HandshakeOptions {
  hostId: string;
  /** The device's X25519 static secret. */
  deviceSecret: Uint8Array;
  /** The host's X25519 static public key, pinned at pairing. */
  hostStaticPubkey: Uint8Array;
  timeoutMs: number;
  /**
   * The first message's payload: empty, unless the device pairs as it
   * connects (`../remote/pairing`). Encrypted to the host's static key.
   */
  hello?: Uint8Array;
}

/** Why a byte channel closed, as a client error. */
export type CloseClassifier = (info: { code: number; reason: string }) => ZwiimClientError;

const channelClosed: CloseClassifier = ({ code, reason }) =>
  new ZwiimClientError("network", `Tunnel connection closed (${code}${reason ? ` ${reason}` : ""})`);

/**
 * Run the device end of the handshake on `channel`. Resolves with the open
 * tunnel; rejects with `device_refused` (the host does not trust this
 * device), `handshake_failed` (the reply did not authenticate: wrong host
 * key or host id), or what `classifyClose` makes of an early close.
 */
export function connectTunnel(
  channel: ByteChannel,
  options: HandshakeOptions,
  classifyClose: CloseClassifier = channelClosed,
): Promise<Tunnel> {
  return new Promise((resolve, reject) => {
    const hs = initiate({
      prologue: tunnelPrologue(options.hostId),
      staticSecret: options.deviceSecret,
      remoteStatic: options.hostStaticPubkey,
    }, options.hello);
    const reader = new FrameReader();
    let settled = false;
    const fail = (e: ZwiimClientError) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      channel.close();
      reject(e);
    };
    const timer = setTimeout(
      () => fail(new ZwiimClientError("network", "Tunnel handshake timed out")),
      options.timeoutMs,
    );
    // Frames that follow the reply in the same read belong to the tunnel.
    let tunnel: Tunnel | null = null;
    channel.listen({
      onData(bytes) {
        if (tunnel) return tunnel.receive(bytes);
        if (settled) return;
        let frames: Uint8Array[];
        try {
          frames = reader.push(bytes);
        } catch {
          return fail(new ZwiimClientError("handshake_failed", "Malformed handshake reply"));
        }
        const [reply, ...rest] = frames;
        if (!reply) return;
        let payload: Uint8Array;
        let session: NoiseSession;
        try {
          ({ payload, session } = hs.readReply(reply));
        } catch {
          return fail(
            new ZwiimClientError("handshake_failed", "The host's handshake reply did not authenticate"),
          );
        }
        let ok: unknown;
        try {
          ok = (JSON.parse(decoder.decode(payload)) as { ok?: unknown }).ok;
        } catch {
          ok = undefined;
        }
        if (typeof ok !== "boolean") {
          return fail(new ZwiimClientError("handshake_failed", "Malformed host verdict"));
        }
        if (!ok) {
          return fail(new ZwiimClientError("device_refused", "The host does not trust this device"));
        }
        settled = true;
        clearTimeout(timer);
        tunnel = new Tunnel(channel, session);
        resolve(tunnel);
        for (const f of rest) tunnel.receiveFrame(f);
        const partial = reader.takeRest();
        if (partial.length) tunnel.receive(partial);
      },
      onClose(info) {
        if (tunnel) return tunnel.closed(info);
        // A host that cannot read the first message (a wrong pinned key or
        // host id) ends the stream without replying.
        fail(classifyClose(info));
      },
    });
    channel.send(frame(hs.message));
  });
}

// ─── the open tunnel ─────────────────────────────────────────────────────

export interface StreamHandlers {
  /** The host accepted the stream (`ws.open` 101). */
  onOpen(): void;
  onMessage(data: string): void;
  /** Ends the stream; fires once. A refused open carries its HTTP status. */
  onClose(info: { code: number; reason: string; status?: number; body?: unknown }): void;
}

export interface TunnelStream {
  send(data: string): void;
  close(): void;
}

export interface TunnelResponse {
  status: number;
  body: unknown;
}

interface Pending {
  resolve(res: TunnelResponse): void;
  reject(e: ZwiimClientError): void;
}

/** Close code for streams whose tunnel ended under them. */
export const TUNNEL_LOST = 1006;

/**
 * An established tunnel: multiplexes requests and event streams by id.
 * When it ends, pending requests fail — `unknown_outcome` for mutating ones,
 * since the host may have applied them — and open streams close.
 */
export class Tunnel {
  private readonly reader = new FrameReader();
  private readonly send: CipherState;
  private readonly recv: CipherState;
  private message: Uint8Array[] = [];
  private messageLen = 0;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private readonly streams = new Map<number, StreamHandlers & { opened: boolean }>();
  private ended = false;
  private readonly closeListeners = new Set<(reason: string) => void>();

  constructor(
    private readonly channel: ByteChannel,
    session: NoiseSession,
  ) {
    this.send = session.send;
    this.recv = session.recv;
  }

  get isOpen(): boolean {
    return !this.ended;
  }

  /** Called once when the tunnel ends, for any reason. */
  onEnd(listener: (reason: string) => void): () => void {
    this.closeListeners.add(listener);
    return () => this.closeListeners.delete(listener);
  }

  /** One `/zwiim/v1/<op>` request. Never resent. */
  request(
    op: string,
    body: unknown,
    options: { timeoutMs: number; mutating: boolean },
  ): Promise<TunnelResponse> {
    const lost = (message: string) =>
      new ZwiimClientError(options.mutating ? "unknown_outcome" : "network", message);
    if (this.ended) return Promise.reject(new ZwiimClientError("network", `${op}: tunnel closed`));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (!this.pending.delete(id)) return;
        reject(
          options.mutating
            ? lost(`${op}: no response in time`)
            : new ZwiimClientError("timeout", `${op} timed out`),
        );
      }, options.timeoutMs);
      this.pending.set(id, {
        resolve: (res) => {
          clearTimeout(timer);
          resolve(res);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e.code === "network" ? lost(`${op}: ${e.message}`) : e);
        },
      });
      this.write({ t: "req", id, op, body: body ?? {} });
    });
  }

  /** Open a `/zwiim/v1/ws` event stream through the tunnel. */
  openStream(handlers: StreamHandlers): TunnelStream {
    const id = this.nextId++;
    const entry = { ...handlers, opened: false };
    if (this.ended) {
      queueMicrotask(() => handlers.onClose({ code: TUNNEL_LOST, reason: "tunnel closed" }));
      return { send() {}, close() {} };
    }
    this.streams.set(id, entry);
    this.write({ t: "ws.open", id });
    return {
      send: (data) => {
        if (this.streams.get(id)?.opened) this.write({ t: "ws.msg", id, data });
      },
      close: () => {
        if (!this.streams.delete(id)) return;
        this.write({ t: "ws.close", id });
        entry.onClose({ code: 1000, reason: "closed by client" });
      },
    };
  }

  /** End the tunnel; the host reads a clean close. */
  close(): void {
    this.end("closed by client");
  }

  // ─── receive ───────────────────────────────────────────────────────────

  /** @internal Bytes from the channel. */
  receive(bytes: Uint8Array): void {
    if (this.ended) return;
    let frames: Uint8Array[];
    try {
      frames = this.reader.push(bytes);
    } catch {
      return this.end("malformed tunnel frame");
    }
    for (const f of frames) this.receiveFrame(f);
  }

  /** @internal One whole frame. */
  receiveFrame(f: Uint8Array): void {
    if (this.ended) return;
    let plain: Uint8Array;
    try {
      plain = this.recv.decrypt(EMPTY, f);
    } catch {
      return this.end("tunnel message failed authentication");
    }
    const flag = plain[0];
    if (flag !== CHUNK_FINAL && flag !== CHUNK_MORE) return this.end("malformed tunnel message");
    const chunk = plain.subarray(1);
    this.messageLen += chunk.length;
    if (this.messageLen > MAX_MESSAGE_LEN) return this.end("tunnel message too large");
    this.message.push(chunk);
    if (flag === CHUNK_MORE) return;
    const bytes = concatBytes(...this.message);
    this.message = [];
    this.messageLen = 0;
    let text: string;
    try {
      text = decoder.decode(bytes);
    } catch {
      return this.end("malformed envelope");
    }
    const envelope = decodeEnvelope(text);
    if (!envelope) return this.end("malformed envelope");
    this.dispatch(envelope);
  }

  /** @internal The channel closed. */
  closed(info: { code: number; reason: string }): void {
    this.end(`relay closed (${info.code}${info.reason ? ` ${info.reason}` : ""})`, false);
  }

  private dispatch(envelope: Envelope): void {
    switch (envelope.t) {
      case "res": {
        const p = this.pending.get(envelope.id);
        if (!p) return; // timed out; the late answer is dropped
        this.pending.delete(envelope.id);
        p.resolve({ status: envelope.status, body: envelope.body });
        return;
      }
      case "ws.open": {
        const s = this.streams.get(envelope.id);
        if (!s || s.opened) return;
        if (envelope.status === 101) {
          s.opened = true;
          s.onOpen();
          return;
        }
        this.streams.delete(envelope.id);
        const status = envelope.status ?? 0;
        const code =
          (envelope.body as { error?: { code?: unknown } } | undefined)?.error?.code;
        s.onClose({
          code: TUNNEL_LOST,
          reason: typeof code === "string" ? code : `HTTP ${status}`,
          status,
          body: envelope.body,
        });
        return;
      }
      case "ws.msg": {
        const s = this.streams.get(envelope.id);
        if (s?.opened) s.onMessage(envelope.data);
        return;
      }
      case "ws.close": {
        const s = this.streams.get(envelope.id);
        if (!s) return;
        this.streams.delete(envelope.id);
        s.onClose({ code: envelope.code ?? 1005, reason: envelope.reason ?? "" });
        return;
      }
      case "req":
        // Only the device sends requests.
        this.end("unexpected request envelope");
        return;
    }
  }

  // ─── send ──────────────────────────────────────────────────────────────

  private write(envelope: Envelope): void {
    if (this.ended) return;
    const message = encoder.encode(JSON.stringify(envelope));
    if (message.length > MAX_MESSAGE_LEN) {
      // Cannot be sent; fail the request/stream it belongs to.
      const p = this.pending.get(envelope.id);
      if (p) {
        this.pending.delete(envelope.id);
        p.reject(new ZwiimClientError("too_large", "Request exceeds the tunnel message limit"));
      }
      return;
    }
    let at = 0;
    do {
      const chunk = message.subarray(at, at + MAX_CHUNK);
      at += chunk.length;
      const plain = concatBytes(
        Uint8Array.of(at < message.length ? CHUNK_MORE : CHUNK_FINAL),
        chunk,
      );
      this.channel.send(frame(this.send.encrypt(EMPTY, plain)));
    } while (at < message.length);
  }

  private end(reason: string, closeChannel = true): void {
    if (this.ended) return;
    this.ended = true;
    if (closeChannel) this.channel.close();
    const pending = [...this.pending.values()];
    this.pending.clear();
    for (const p of pending) p.reject(new ZwiimClientError("network", reason));
    const streams = [...this.streams.values()];
    this.streams.clear();
    for (const s of streams) s.onClose({ code: TUNNEL_LOST, reason: "tunnel closed" });
    for (const l of [...this.closeListeners]) l(reason);
  }
}
