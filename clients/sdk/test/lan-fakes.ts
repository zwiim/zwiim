// A fake local network: hosts advertise themselves (as mDNS would report
// them) and accept TCP-like byte streams, which `FakeHost` serves with the
// real tunnel. Cut it, take it down, or leave stale advertisements behind to
// act out network changes.

import type { ByteChannel } from "../src/remote/tunnel";
import type { LanAdvertisement, LanDial, LanDiscover } from "../src/transport/lan";
import type { FakeHost } from "./relay-fakes";

interface Listener {
  host: FakeHost;
  address: string;
  port: number;
  /** Accepting connections (the host's LAN access is on). */
  up: boolean;
  /** Reported by discovery. */
  announced: boolean;
}

interface Link {
  device: ByteChannel;
  host: ByteChannel;
  /** Close; `abrupt` loses what is still in flight (the network went away). */
  cut(abrupt?: boolean): void;
  closed: boolean;
}

/** Two connected byte-stream ends; delivery is asynchronous, like a network. */
function pipe(onClose: () => void): Link {
  type Handlers = Parameters<ByteChannel["listen"]>[0];
  const handlers: [Handlers | null, Handlers | null] = [null, null];
  const queues: [Uint8Array[], Uint8Array[]] = [[], []];
  const link = {} as Link;
  link.closed = false;
  // Bytes already sent still arrive after an orderly close, as over TCP.
  let delivering = true;
  const end = (side: 0 | 1): ByteChannel => ({
    send(bytes) {
      if (link.closed) return;
      const copy = bytes.slice();
      const other = (1 - side) as 0 | 1;
      setTimeout(() => {
        if (!delivering) return;
        const h = handlers[other];
        if (h) h.onData(copy);
        else queues[other].push(copy);
      }, 0);
    },
    close() {
      link.cut();
    },
    listen(h) {
      handlers[side] = h;
      for (const b of queues[side].splice(0)) h.onData(b);
    },
  });
  link.device = end(0);
  link.host = end(1);
  link.cut = (abrupt = false) => {
    if (link.closed) return;
    link.closed = true;
    if (abrupt) delivering = false;
    onClose();
    setTimeout(() => {
      delivering = false;
      for (const h of handlers) h?.onClose({ code: 1006, reason: "" });
    }, 0);
  };
  return link;
}

export class FakeLan {
  private readonly listeners: Listener[] = [];
  /** Extra advertisements, e.g. an impostor's. */
  readonly extra: LanAdvertisement[] = [];
  readonly links: Link[] = [];
  /** This device is on the network (Wi-Fi on). */
  connected = true;
  /** Advertisements seen before the network changed are still reported. */
  stale = false;
  private staleAds: LanAdvertisement[] = [];
  discoveries = 0;
  dials: string[] = [];
  private nextPort = 50_000;

  /** The host turns LAN access on at `address`. */
  advertise(host: FakeHost, address = "192.168.1.20", announced = true): Listener {
    const listener = { host, address, port: this.nextPort++, up: true, announced };
    this.listeners.push(listener);
    return listener;
  }

  ad(l: Listener, overrides: Partial<LanAdvertisement> & { pk?: string } = {}): LanAdvertisement {
    const { pk, ...rest } = overrides;
    return {
      instance: l.host.hostId,
      port: l.port,
      addresses: [l.address],
      txt: { v: "1", id: l.host.hostId, pk: pk ?? l.host.publicKey },
      ...rest,
    };
  }

  get openLinks(): Link[] {
    return this.links.filter((l) => !l.closed);
  }

  /** Every LAN connection drops (the network went away under them). */
  cut(): void {
    for (const l of this.openLinks) l.cut(true);
  }

  /** Wi-Fi off: connections drop, nothing is found or reachable. */
  leave(): void {
    this.staleAds = this.current();
    this.connected = false;
    this.cut();
  }

  join(): void {
    this.connected = true;
    this.staleAds = [];
  }

  private current(): LanAdvertisement[] {
    return [...this.listeners.filter((l) => l.up && l.announced).map((l) => this.ad(l)), ...this.extra];
  }

  readonly discover: LanDiscover = async (hostId) => {
    this.discoveries++;
    await new Promise((r) => setTimeout(r, 0));
    const ads = this.connected ? this.current() : this.stale ? this.staleAds : [];
    return ads.filter((a) => a.instance === hostId);
  };

  readonly dial: LanDial = async ({ address, port }) => {
    this.dials.push(`${address}:${port}`);
    await new Promise((r) => setTimeout(r, 0));
    const listener = this.listeners.find((l) => l.address === address && l.port === port && l.up);
    if (!this.connected || !listener) throw new Error("connect ECONNREFUSED");
    const link = pipe(() => {});
    this.links.push(link);
    listener.host.accept(link.host);
    return link.device;
  };
}
