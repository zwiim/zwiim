import { x25519 } from "@noble/curves/ed25519.js";
import { describe, expect, it, vi } from "vitest";
import { initiate, respond, concatBytes } from "../src/remote/noise";
import { decodeKey, devicePublicKey, encodeKey, generateDeviceKeys } from "../src/remote/keys";
import {
  connectTunnel,
  decodeEnvelope,
  MAX_MESSAGE_LEN,
  tunnelPrologue,
  type ByteChannel,
  type Tunnel,
} from "../src/remote/tunnel";
import { isClientError } from "../src/client/errors";

const hex = (s: string) => Uint8Array.from(s.match(/../g)!.map((b) => parseInt(b, 16)));
const toHex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");

// The Noise_IK_25519_ChaChaPoly_BLAKE2s vector from the cacophony suite, as
// shipped with snow (the host's Noise implementation).
const V = {
  prologue: "4a6f686e2047616c74",
  initStatic: "e61ef9919cde45dd5f82166404bd08e38bceb5dfdfded0a34c8df7ed542214d1",
  initEphemeral: "893e28b9dc6ca8d611ab664754b8ceb7bac5117349a4439a6b0569da977c464a",
  respStatic: "4a3acbfdb163dec651dfa3194dece676d437029c62a408b4c5ea9114246e4893",
  respEphemeral: "bbdb4cdbd309f1a1f2e1456967fe288cadd6f712d65dc7b7793d5e63da6b375b",
  initRemoteStatic: "31e0303fd6418d2f8c0e78b91f22e8caed0fbe48656dcf4767e4834f701b8f62",
  handshakeHash: "48f3cb8bc9319da4ba1e9933991b1c4ed4034f1f126a76d3a1fbcfd7f94248d4",
  messages: [
    ["4c756477696720766f6e204d69736573", "ca35def5ae56cec33dc2036731ab14896bc4c75dbb07a61f879f8e3afa4c79440b03ddc7aac5123d06a1b23b71670e32e76c28239a7ca4ac8f784de7e44c1adbfc6e83fef7352a58d9d56157400c0a737b1d171ce368229c7b752ac25b8faf4eca690f6d896f543be02c996ab2b86b76"],
    ["4d757272617920526f746862617264", "95ebc60d2b1fa672c1f46a8aa265ef51bfe38e7ccb39ec5be34069f144808843d9b5a8927f0ac9655ef76833bc7e5561f42e691ac8404efd6fbd6308b6a27c"],
    ["462e20412e20486179656b", "2c256ed08fcd08c2980f954ee4beaccb61c9581340f5dd2fd1cf3b"],
    ["4361726c204d656e676572", "d6033f70eee20945c7c9dba304e397ee3b284ff5e00fd9efb095d3"],
    ["4a65616e2d426170746973746520536179", "a9c068ca5d8babf72560652d8e851adbfac35c8a66e810d560863173e96adf4cfe"],
    ["457567656e2042f6686d20766f6e2042617765726b", "2a09d8f459e5927e40fdd2eddc99bdafb04e13a26f145cb5cfe9e6ba34c94331ebc17d5156"],
  ] as const,
};

describe("Noise IK", () => {
  it("matches the cacophony test vector on both ends", () => {
    const [m0, m1, ...transport] = V.messages;
    const init = initiate(
      {
        prologue: hex(V.prologue),
        staticSecret: hex(V.initStatic),
        remoteStatic: hex(V.initRemoteStatic),
        ephemeralSecret: hex(V.initEphemeral),
      },
      hex(m0![0]),
    );
    expect(toHex(init.message)).toBe(m0![1]);

    const resp = respond(
      { prologue: hex(V.prologue), staticSecret: hex(V.respStatic), ephemeralSecret: hex(V.respEphemeral) },
      init.message,
    );
    expect(toHex(resp.payload)).toBe(m0![0]);
    expect(toHex(resp.remoteStatic)).toBe(toHex(x25519.getPublicKey(hex(V.initStatic))));
    const r = resp.reply(hex(m1![0]));
    expect(toHex(r.message)).toBe(m1![1]);

    const i = init.readReply(r.message);
    expect(toHex(i.payload)).toBe(m1![0]);
    expect(toHex(i.session.handshakeHash)).toBe(V.handshakeHash);
    expect(toHex(r.session.handshakeHash)).toBe(V.handshakeHash);

    // Transport messages alternate, initiator first.
    transport.forEach(([payload, ciphertext], n) => {
      const [sender, receiver] = n % 2 === 0 ? [i.session, r.session] : [r.session, i.session];
      const ct = sender.send.encrypt(new Uint8Array(0), hex(payload));
      expect(toHex(ct)).toBe(ciphertext);
      expect(toHex(receiver.recv.decrypt(new Uint8Array(0), ct))).toBe(payload);
    });
  });

  it("fails a reply from a host holding a different key", () => {
    const device = generateDeviceKeys();
    const hostSecret = x25519.utils.randomSecretKey();
    const impostor = x25519.utils.randomSecretKey();
    const prologue = tunnelPrologue("host-1");
    const init = initiate({ prologue, staticSecret: device.secret, remoteStatic: x25519.getPublicKey(hostSecret) });
    // The impostor cannot read the first message at all.
    expect(() => respond({ prologue, staticSecret: impostor }, init.message)).toThrow();
    // Nor can the right key under another host id (the prologue binds it).
    expect(() => respond({ prologue: tunnelPrologue("host-2"), staticSecret: hostSecret }, init.message)).toThrow();
  });
});

describe("device keys", () => {
  it("round-trips base64url X25519 keys", () => {
    const k = generateDeviceKeys();
    expect(k.publicKey).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(devicePublicKey(k.secret)).toBe(k.publicKey);
    expect(encodeKey(decodeKey(k.publicKey))).toBe(k.publicKey);
    expect(() => decodeKey("short")).toThrow();
    expect(() => decodeKey(k.publicKey + "A")).toThrow();
  });
});

describe("envelopes", () => {
  it("decodes the host's wire shapes", () => {
    expect(decodeEnvelope('{"t":"res","id":1,"status":200,"body":[]}')).toEqual({ t: "res", id: 1, status: 200, body: [] });
    expect(decodeEnvelope('{"t":"ws.open","id":2,"status":101}')).toEqual({ t: "ws.open", id: 2, status: 101 });
    expect(decodeEnvelope('{"t":"ws.msg","id":2,"data":"{}"}')).toEqual({ t: "ws.msg", id: 2, data: "{}" });
    expect(decodeEnvelope('{"t":"ws.close","id":2,"code":4403,"reason":"device_revoked"}')).toEqual({
      t: "ws.close",
      id: 2,
      code: 4403,
      reason: "device_revoked",
    });
  });

  it("rejects anything else, like the host", () => {
    for (const text of [
      '{"t":"req","id":1,"op":"host.hello","path":"/api/x"}',
      '{"t":"http","id":1}',
      '{"id":1,"op":"host.hello"}',
      '{"t":"res","id":-1,"status":200,"body":null}',
      '{"t":"res","id":1,"status":200}',
      '{"t":"ws.msg","id":1,"data":{"op":"ping"}}',
      "not json",
    ]) {
      expect(decodeEnvelope(text), text).toBeNull();
    }
  });
});

// ─── the tunnel over an in-memory byte stream ────────────────────────────

/** Two connected byte channels; bytes arrive re-chunked at random sizes. */
function pipe(seed = 1): [ByteChannel & { taps: Uint8Array[] }, ByteChannel] {
  let rnd = seed;
  const next = () => ((rnd = (rnd * 1103515245 + 12345) % 2 ** 31), rnd);
  type H = Parameters<ByteChannel["listen"]>[0];
  const ends: { h: H | null; queue: Uint8Array[]; closed: boolean }[] = [
    { h: null, queue: [], closed: false },
    { h: null, queue: [], closed: false },
  ];
  const taps: Uint8Array[] = [];
  const make = (me: number) => {
    const peer = ends[1 - me]!;
    return {
      taps,
      send(bytes: Uint8Array) {
        if (ends[me]!.closed) return;
        taps.push(bytes);
        let at = 0;
        while (at < bytes.length) {
          const n = 1 + (next() % 5000);
          const piece = bytes.slice(at, at + n);
          at += n;
          setTimeout(() => (peer.h ? peer.h.onData(piece) : peer.queue.push(piece)), 0);
        }
      },
      close() {
        if (ends[me]!.closed) return;
        ends[me]!.closed = true;
        peer.closed = true;
        setTimeout(() => peer.h?.onClose({ code: 1000, reason: "" }), 0);
        setTimeout(() => ends[me]!.h?.onClose({ code: 1000, reason: "" }), 0);
      },
      listen(h: H) {
        ends[me]!.h = h;
        for (const b of ends[me]!.queue.splice(0)) h.onData(b);
      },
    };
  };
  return [make(0), make(1)];
}

/** A minimal host end: handshake, then echo each request's op back. */
function echoHost(channel: ByteChannel, hostSecret: Uint8Array, ok = true) {
  let buf: Uint8Array = new Uint8Array(0);
  let session: ReturnType<ReturnType<typeof respond>["reply"]>["session"] | null = null;
  let parts: Uint8Array[] = [];
  const seen: string[] = [];
  const send = (text: string) => {
    const plain = concatBytes(Uint8Array.of(1), new TextEncoder().encode(text));
    const ct = session!.send.encrypt(new Uint8Array(0), plain);
    channel.send(concatBytes(Uint8Array.of(ct.length >> 8, ct.length & 0xff), ct));
  };
  channel.listen({
    onData(bytes) {
      buf = concatBytes(buf, bytes);
      while (buf.length >= 2 && buf.length >= 2 + ((buf[0]! << 8) | buf[1]!)) {
        const len = (buf[0]! << 8) | buf[1]!;
        const f = buf.slice(2, 2 + len);
        buf = buf.slice(2 + len);
        if (!session) {
          const r = respond({ prologue: tunnelPrologue("host-1"), staticSecret: hostSecret }, f);
          const out = r.reply(new TextEncoder().encode(JSON.stringify({ ok })));
          channel.send(concatBytes(Uint8Array.of(out.message.length >> 8, out.message.length & 0xff), out.message));
          session = out.session;
          if (!ok) channel.close();
          continue;
        }
        const plain = session.recv.decrypt(new Uint8Array(0), f);
        parts.push(plain.subarray(1));
        if (plain[0] !== 1) continue;
        const env = JSON.parse(new TextDecoder().decode(concatBytes(...parts)));
        parts = [];
        seen.push(env.op);
        if (env.t === "req") send(JSON.stringify({ t: "res", id: env.id, status: 200, body: { op: env.op, size: JSON.stringify(env.body).length } }));
      }
    },
    onClose() {},
  });
  return { seen };
}

async function open(ok = true, seed = 1) {
  const device = generateDeviceKeys();
  const hostSecret = x25519.utils.randomSecretKey();
  const [d, h] = pipe(seed);
  const host = echoHost(h, hostSecret, ok);
  const tunnel = connectTunnel(d, {
    hostId: "host-1",
    deviceSecret: device.secret,
    hostStaticPubkey: x25519.getPublicKey(hostSecret),
    timeoutMs: 2000,
  });
  return { tunnel, host, wire: d.taps };
}

describe("tunnel", () => {
  it("carries requests over a re-chunked stream, large ones in several Noise messages", async () => {
    const { tunnel, host, wire } = await open(true, 7);
    const t: Tunnel = await tunnel;
    const big = "x".repeat(200_000);
    const [a, b] = await Promise.all([
      t.request("files.write", { content: big }, { timeoutMs: 2000, mutating: true }),
      t.request("workspace.list", {}, { timeoutMs: 2000, mutating: false }),
    ]);
    expect(a.body).toEqual({ op: "files.write", size: big.length + 14 });
    expect(b.body).toEqual({ op: "workspace.list", size: 2 });
    expect(host.seen).toEqual(["files.write", "workspace.list"]);
    // Nothing readable crossed the wire.
    const all = new TextDecoder("latin1").decode(concatBytes(...wire));
    expect(all).not.toContain("workspace.list");
    expect(all).not.toContain("xxxxxxxx");
  });

  it("sends a large message in time slices, keeping later messages in order", async () => {
    const { tunnel, host, wire } = await open(true, 3);
    const t = await tunnel;
    // Each clock read moves 5 ms on, so a send slice ends after a frame or two.
    let clock = 0;
    const now = vi.spyOn(Date, "now").mockImplementation(() => (clock += 5));
    try {
      const sent = wire.length;
      const big = "x".repeat(1_000_000);
      const a = t.request("files.write", { content: big }, { timeoutMs: 5000, mutating: true });
      const b = t.request("workspace.list", {}, { timeoutMs: 5000, mutating: false });
      // The upload did not go out in one loop that blocks the JS thread.
      expect(wire.length - sent).toBeLessThan(Math.ceil(big.length / 65518));
      const [ra, rb] = await Promise.all([a, b]);
      expect(ra.body).toEqual({ op: "files.write", size: big.length + 14 });
      expect(rb.body).toEqual({ op: "workspace.list", size: 2 });
      expect(host.seen).toEqual(["files.write", "workspace.list"]);
    } finally {
      now.mockRestore();
    }
  });

  it("refuses to send a message over the limit", async () => {
    const t = await (await open()).tunnel;
    await expect(
      t.request("files.write", { content: "x".repeat(MAX_MESSAGE_LEN) }, { timeoutMs: 2000, mutating: true }),
    ).rejects.toMatchObject({ code: "too_large" });
  });

  it("reports a host that refuses the device", async () => {
    const { tunnel } = await open(false);
    await expect(tunnel).rejects.toMatchObject({ code: "device_refused" });
  });

  it("fails the handshake against a host with another key", async () => {
    const device = generateDeviceKeys();
    const [d, h] = pipe();
    // The "host" answers with a handshake made for a different key.
    const other = x25519.utils.randomSecretKey();
    h.listen({
      onData() {
        const fake = initiate({ prologue: tunnelPrologue("host-1"), staticSecret: other, remoteStatic: x25519.getPublicKey(other) });
        const reply = fake.message.subarray(0, 48);
        h.send(concatBytes(Uint8Array.of(0, reply.length), reply));
      },
      onClose() {},
    });
    const err = await connectTunnel(d, {
      hostId: "host-1",
      deviceSecret: device.secret,
      hostStaticPubkey: x25519.getPublicKey(x25519.utils.randomSecretKey()),
      timeoutMs: 2000,
    }).catch((e) => e);
    expect(isClientError(err, "handshake_failed")).toBe(true);
  });

  it("fails pending requests when the stream ends: reads network, mutations unknown_outcome", async () => {
    const device = generateDeviceKeys();
    const hostSecret = x25519.utils.randomSecretKey();
    const [d, h] = pipe();
    let session: any = null;
    h.listen({
      onData(bytes) {
        if (session) return; // swallow requests, never answer
        const r = respond({ prologue: tunnelPrologue("host-1"), staticSecret: hostSecret }, bytes.subarray(2));
        const out = r.reply(new TextEncoder().encode('{"ok":true}'));
        session = out.session;
        h.send(concatBytes(Uint8Array.of(out.message.length >> 8, out.message.length & 0xff), out.message));
      },
      onClose() {},
    });
    const t = await connectTunnel(d, {
      hostId: "host-1",
      deviceSecret: device.secret,
      hostStaticPubkey: x25519.getPublicKey(hostSecret),
      timeoutMs: 2000,
    });
    const read = t.request("git.status", {}, { timeoutMs: 5000, mutating: false }).catch((e) => e);
    const write = t.request("session.send", {}, { timeoutMs: 5000, mutating: true }).catch((e) => e);
    const slow = t.request("files.read", {}, { timeoutMs: 20, mutating: false }).catch((e) => e);
    expect((await slow).code).toBe("timeout");
    h.close();
    expect((await read).code).toBe("network");
    expect((await write).code).toBe("unknown_outcome");
    expect(t.isOpen).toBe(false);
  });
});
