import { generateDeviceKeys, parseLanAdvertisement, type ByteChannel } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import { nativeDial, ReportedEndpoint, type NativeTcp } from "../src/platform/lan";
import { MemorySecureStore } from "../src/platform/memory-store";

/** The native TCP module, scripted. */
class FakeNativeTcp implements NativeTcp {
  sent: [string, string][] = [];
  closed: string[] = [];
  refuse = false;
  private data: ((e: { id: string; data: string }) => void)[] = [];
  private ends: ((e: { id: string; reason: string }) => void)[] = [];
  /** Bytes the "host" sends as soon as the connection opens. */
  greeting: string | null = null;

  async connect(id: string): Promise<void> {
    if (this.refuse) throw new Error("Connection refused");
    if (this.greeting) this.emitData(id, this.greeting);
  }
  send(id: string, data: string): void {
    this.sent.push([id, data]);
  }
  close(id: string): void {
    this.closed.push(id);
  }
  addListener(event: "onData" | "onClose", listener: (e: any) => void) {
    (event === "onData" ? this.data : this.ends).push(listener);
    return { remove() {} };
  }
  emitData(id: string, data: string) {
    for (const l of this.data) l({ id, data });
  }
  emitClose(id: string, reason: string) {
    for (const l of this.ends) l({ id, reason });
  }
}

const ids = () => {
  let n = 0;
  return () => `c${++n}`;
};

function collect(channel: ByteChannel) {
  const got: { data: number[][]; closes: { code: number; reason: string }[] } = { data: [], closes: [] };
  channel.listen({ onData: (b) => got.data.push([...b]), onClose: (i) => got.closes.push(i) });
  return got;
}

describe("LAN dial over the native module", () => {
  it("moves bytes both ways as base64, keeping what arrived before listening", async () => {
    const native = new FakeNativeTcp();
    native.greeting = btoa("\x01\x02");
    const dial = nativeDial(native, ids());
    const channel = await dial({ address: "192.168.1.20", port: 7000 }, { timeoutMs: 100 });
    const got = collect(channel);
    expect(got.data).toEqual([[1, 2]]);
    native.emitData("c1", btoa("\xff"));
    expect(got.data).toEqual([[1, 2], [255]]);
    const big = new Uint8Array(100_000).map((_, i) => i & 0xff);
    channel.send(big);
    expect(native.sent).toHaveLength(1);
    const [id, data] = native.sent[0]!;
    expect(id).toBe("c1");
    expect(Uint8Array.from(atob(data), (c) => c.charCodeAt(0))).toEqual(big);
  });

  it("reports the end once, whichever side closed", async () => {
    const native = new FakeNativeTcp();
    const dial = nativeDial(native, ids());
    const a = await dial({ address: "10.0.0.2", port: 1 }, { timeoutMs: 100 });
    const b = await dial({ address: "10.0.0.2", port: 1 }, { timeoutMs: 100 });
    const gotA = collect(a);
    const gotB = collect(b);
    a.close();
    a.close();
    native.emitClose("c1", "late");
    expect(native.closed).toEqual(["c1"]);
    expect(gotA.closes).toEqual([{ code: 1000, reason: "closed by device" }]);
    native.emitClose("c2", "closed by host");
    native.emitData("c2", btoa("x"));
    b.close();
    expect(gotB.closes).toEqual([{ code: 1006, reason: "closed by host" }]);
    expect(gotB.data).toEqual([]);
    expect(native.closed).toEqual(["c1"]);
    // Nothing more goes out on a closed channel.
    b.send(Uint8Array.of(1));
    expect(native.sent).toEqual([]);
  });

  it("a refused connection is an error, and leaves nothing behind", async () => {
    const native = new FakeNativeTcp();
    native.refuse = true;
    const dial = nativeDial(native, ids());
    await expect(dial({ address: "10.0.0.2", port: 1 }, { timeoutMs: 100 })).rejects.toThrow("refused");
    native.emitData("c1", btoa("x")); // ignored
  });
});

describe("the host's reported LAN endpoint", () => {
  const key = generateDeviceKeys().publicKey;
  const pinned = { host_id: "host-1", host_static_pubkey: key };

  it("discovers nothing until the host says, then its endpoint for that host only", async () => {
    const saved: unknown[] = [];
    const r = new ReportedEndpoint(pinned, async (e) => void saved.push(e));
    expect(r.known).toBeNull();
    expect(await r.discover("host-1", { timeoutMs: 100 })).toEqual([]);

    await r.report({ addresses: ["192.168.1.20", "10.0.0.5"], port: 7000 });
    expect(r.known).toEqual({ addresses: ["192.168.1.20", "10.0.0.5"], port: 7000 });
    const [ad] = await r.discover("host-1", { timeoutMs: 100 });
    // Carries the pinned key, never one the host sent.
    expect(parseLanAdvertisement(ad)).toEqual({ hostId: "host-1", hostStaticPubkey: key, addresses: ["192.168.1.20", "10.0.0.5"], port: 7000 });
    expect(await r.discover("host-2", { timeoutMs: 100 })).toEqual([]);

    // Kept only when it changed; LAN access off forgets it.
    await r.report({ addresses: ["192.168.1.20", "10.0.0.5"], port: 7000 });
    await r.report(null);
    expect(saved).toEqual([{ addresses: ["192.168.1.20", "10.0.0.5"], port: 7000 }, null]);
    expect(await r.discover("host-1", { timeoutMs: 100 })).toEqual([]);
  });

  it("starts from what was kept, and treats a malformed report as none", async () => {
    const r = new ReportedEndpoint({ ...pinned, lan_endpoint: { addresses: ["192.168.1.20"], port: 7000 } }, async () => {});
    expect(r.known).toEqual({ addresses: ["192.168.1.20"], port: 7000 });
    await r.report({ addresses: ["host.example", "224.0.0.1"], port: 7000 });
    expect(r.known).toBeNull();
    await r.report({ addresses: ["192.168.1.20"], port: 70000 });
    expect(r.known).toBeNull();
  });
});

describe("in-memory secure store", () => {
  it("keeps the device identity, pinned hosts and account token", async () => {
    const store = new MemorySecureStore();
    expect(await store.deviceIdentity()).toBeNull();
    expect(await store.pinnedHost("host-1")).toBeNull();
    expect(await store.accountToken()).toBeNull();
    const keys = generateDeviceKeys();
    await store.setDeviceIdentity({ device_id: "d1", keys });
    keys.secret.fill(0);
    const stored = await store.deviceIdentity();
    expect(stored?.device_id).toBe("d1");
    expect(stored?.keys.secret.some((b) => b !== 0)).toBe(true);
    await store.pinHost({ host_id: "host-1", host_static_pubkey: "pk" });
    expect(await store.pinnedHost("host-1")).toEqual({ host_id: "host-1", host_static_pubkey: "pk" });
    await store.setAccountToken("t");
    expect(await store.accountToken()).toBe("t");
    await store.setAccountToken(null);
    expect(await store.accountToken()).toBeNull();
  });

  it("does not expose its secrets as fields", () => {
    const store = new MemorySecureStore();
    expect(Object.keys(store)).toEqual([]);
    expect(JSON.stringify(store)).toBe("{}");
  });
});
