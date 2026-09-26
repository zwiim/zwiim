// The SDK's LAN inputs on a phone: a TCP dialer over the native `ZwiimLan`
// module, and discovery.

import { parseLanAdvertisement, type ByteChannel, type HostLanEndpoint, type LanDial, type LanDiscover } from "@zwiim/sdk";
import type { PinnedHost } from "./capabilities";

/** The native TCP module's surface (`modules/zwiim-lan`). */
export interface NativeTcp {
  connect(id: string, address: string, port: number, timeoutMs: number): Promise<void>;
  send(id: string, data: string): void;
  close(id: string): void;
  addListener(event: "onData", listener: (e: { id: string; data: string }) => void): { remove(): void };
  addListener(event: "onClose", listener: (e: { id: string; reason: string }) => void): { remove(): void };
}

type Handlers = Parameters<ByteChannel["listen"]>[0];

interface Pending {
  handlers: Handlers | null;
  queue: Uint8Array[];
  closed: { code: number; reason: string } | null;
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

function fromBase64(text: string): Uint8Array {
  const binary = atob(text);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

/**
 * A `LanDial` over the native module: each connection is a `ByteChannel`
 * whose `onClose` fires once, whichever end closed.
 */
export function nativeDial(native: NativeTcp, newId: () => string): LanDial {
  const open = new Map<string, Pending>();
  const end = (id: string, info: { code: number; reason: string }) => {
    const c = open.get(id);
    if (!c || c.closed) return;
    c.closed = info;
    open.delete(id);
    c.handlers?.onClose(info);
  };
  native.addListener("onData", ({ id, data }) => {
    const c = open.get(id);
    if (!c) return;
    const bytes = fromBase64(data);
    if (c.handlers) c.handlers.onData(bytes);
    else c.queue.push(bytes);
  });
  native.addListener("onClose", ({ id, reason }) => end(id, { code: 1006, reason }));

  return async ({ address, port }, { timeoutMs }) => {
    const id = newId();
    // Registered first: bytes can arrive before `connect` resolves here.
    const c: Pending = { handlers: null, queue: [], closed: null };
    open.set(id, c);
    try {
      await native.connect(id, address, port, timeoutMs);
    } catch (e) {
      open.delete(id);
      throw e;
    }
    return {
      send(bytes) {
        if (!c.closed) native.send(id, toBase64(bytes));
      },
      close() {
        if (c.closed) return;
        native.close(id);
        end(id, { code: 1000, reason: "closed by device" });
      },
      listen(h) {
        c.handlers = h;
        for (const b of c.queue.splice(0)) h.onData(b);
        if (c.closed) h.onClose(c.closed);
      },
    };
  };
}

/**
 * Discovery from where the host itself last said it can be dialed
 * (`host.lan_endpoint`), kept with its pinned key. Stands in for mDNS.
 * Nothing is trusted on the host's say-so: the tunnel handshake still
 * proves the host holds the pinned key.
 */
export class ReportedEndpoint {
  readonly #host: PinnedHost;
  readonly #save: (endpoint: HostLanEndpoint | null) => Promise<void>;
  #endpoint: HostLanEndpoint | null;

  constructor(host: PinnedHost, save: (endpoint: HostLanEndpoint | null) => Promise<void>) {
    this.#host = host;
    this.#save = save;
    this.#endpoint = this.#valid(host.lan_endpoint ?? null);
  }

  /** Where the host can be dialed, as last heard; none before it has said. */
  get known(): HostLanEndpoint | null {
    return this.#endpoint;
  }

  readonly discover: LanDiscover = async (hostId) => {
    const endpoint = this.#endpoint;
    return endpoint && hostId === this.#host.host_id ? [this.#ad(endpoint)] : [];
  };

  /** What the host says now (none: its LAN access is off); kept if it changed. */
  async report(endpoint: HostLanEndpoint | null): Promise<void> {
    const next = this.#valid(endpoint);
    if (JSON.stringify(next) === JSON.stringify(this.#endpoint)) return;
    this.#endpoint = next;
    await this.#save(next);
  }

  #ad(endpoint: HostLanEndpoint) {
    const { host_id, host_static_pubkey } = this.#host;
    return { instance: host_id, port: endpoint.port, addresses: endpoint.addresses, txt: { v: "1", id: host_id, pk: host_static_pubkey } };
  }

  /** The endpoint as the SDK would take it from an advertisement; none if it would not. */
  #valid(endpoint: HostLanEndpoint | null): HostLanEndpoint | null {
    const parsed = endpoint && parseLanAdvertisement(this.#ad(endpoint));
    return parsed && { addresses: parsed.addresses, port: parsed.port };
  }
}
