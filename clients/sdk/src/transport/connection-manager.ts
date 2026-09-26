import { backoffDelay, type BackoffOptions, type ConnectionStatus } from "../client/connection";
import { ZwiimClientError } from "../client/errors";
import type { CloudClient } from "../remote/cloud";
import type { ClientMessage, OpName } from "../types/protocol";
import { isLanUnavailable, LanTransport, type LanDial, type LanDiscover } from "./lan";
import { RelayTransport } from "./relay";
import type {
  EventChannel,
  EventChannelHandlers,
  RequestOptions,
  Transport,
} from "./transport";

/**
 * How a remote host is reached:
 *
 * - `auto`: directly on the LAN when the host is there, else through Zwiim
 *   Cloud.
 * - `local`: directly on the LAN only; never contacts the Cloud.
 * - `cloud`: through Zwiim Cloud only; never looks on the LAN.
 */
export type ConnectionMode = "auto" | "local" | "cloud";

/** The path an open connection takes. */
export type ConnectionPath = "lan" | "relay";

/** What a UI shows for a host connection. */
export type ActiveConnection = ConnectionPath | "connecting" | "reconnecting" | "offline";

export interface ConnectionManagerOptions {
  mode: ConnectionMode;
  /** The selected host. */
  hostId: string;
  /** The host's X25519 static public key, pinned at pairing. */
  hostStaticPubkey: string | Uint8Array;
  /** This device's X25519 static secret. */
  deviceSecret: Uint8Array;
  /** The platform's LAN discovery and dialer. Without them, no LAN path. */
  lan?: { discover: LanDiscover; dial: LanDial } | null;
  /** The account's Cloud. Without it, no relay path. */
  cloud?: CloudClient | null;
  /** Default request timeout. */
  timeoutMs?: number;
  lanDiscoverTimeoutMs?: number;
  lanConnectTimeoutMs?: number;
  relayConnectTimeoutMs?: number;
  /**
   * In `auto` mode, after the LAN path is found unavailable, how long to go
   * straight to the relay before looking on the LAN again (grows with each
   * miss, reset by a LAN connection).
   */
  lanRetry?: BackoffOptions;
  now?: () => number;
  random?: () => number;
}

const DEFAULT_LAN_RETRY: BackoffOptions = { baseMs: 5_000, maxMs: 60_000 };

interface Selected {
  path: ConnectionPath;
  transport: LanTransport | RelayTransport;
}

/**
 * The one place a remote host's connection path is chosen. A `Transport`
 * like any other: `HostClient` and everything above it are the same
 * whichever path is taken.
 *
 * A path is chosen only when there is no open connection — on first use,
 * and after the previous connection ended. An open connection is never
 * dropped because another path became available; a reconnect is when
 * `auto` looks on the LAN again. Switching closes the old connection
 * before the new one carries anything, and `HostClient` takes a fresh
 * snapshot on it, so no state crosses paths.
 *
 * In `auto`:
 *
 * 1. The LAN is tried first: the host's advertisement is checked against
 *    the pinned key and the tunnel handshake proves the host holds it.
 * 2. If the host is just not reachable directly (`lan_unavailable`), the
 *    relay is used, and the LAN is skipped for a while (bounded backoff) so
 *    that reconnecting does not keep paying for a LAN miss.
 * 3. An identity or trust failure on the LAN (`host_identity_mismatch`,
 *    `handshake_failed`, `device_refused`) is reported, never hidden
 *    behind the relay.
 *
 * Errors: `lan_unavailable` in `local` mode when the host is not reachable
 * directly; relay errors (`host_offline`, `cloud_unavailable`, …) as
 * `RelayTransport` reports them.
 */
export class ConnectionManager implements Transport {
  readonly mode: ConnectionMode;
  private readonly options: ConnectionManagerOptions;
  private readonly lanRetry: BackoffOptions;
  private readonly now: () => number;
  private readonly random: () => number;
  private current: Selected | null = null;
  private selecting: Promise<Selected> | null = null;
  /** Bumped by `close()`; a selection that finishes after it is discarded. */
  private generation = 0;
  private lanMisses = 0;
  private lanRetryAt = 0;
  private pathValue: ConnectionPath | null = null;
  private readonly listeners = new Set<(path: ConnectionPath | null) => void>();

  constructor(options: ConnectionManagerOptions) {
    this.mode = options.mode;
    this.options = options;
    this.lanRetry = options.lanRetry ?? DEFAULT_LAN_RETRY;
    this.now = options.now ?? Date.now;
    this.random = options.random ?? Math.random;
  }

  /** The path of the current connection; `null` before the first one. */
  get path(): ConnectionPath | null {
    return this.pathValue;
  }

  /** Whether a connection is open now. */
  get connected(): boolean {
    return this.current?.transport.connected ?? false;
  }

  /** Called with the path of each new connection. */
  onPath(listener: (path: ConnectionPath | null) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  async request(op: OpName, params: unknown, options?: RequestOptions): Promise<unknown> {
    // A failure to connect happens before anything is sent: never
    // `unknown_outcome`.
    const { transport } = await this.active();
    return transport.request(op, params, options);
  }

  openEvents(handlers: EventChannelHandlers): EventChannel {
    let closed = false;
    let inner: EventChannel | null = null;
    const finish = (code: number, reason: string) => {
      if (closed) return;
      closed = true;
      handlers.onClose({ code, reason });
    };
    this.active().then(
      ({ transport }) => {
        if (closed) return;
        inner = transport.openEvents({
          onOpen: () => {
            if (!closed) handlers.onOpen();
          },
          onFrame: (frame) => {
            if (!closed) handlers.onFrame(frame);
          },
          onClose: ({ code, reason }) => finish(code, reason),
        });
      },
      (e: unknown) => finish(1006, e instanceof Error ? e.message : "connect failed"),
    );
    return {
      send(msg: ClientMessage) {
        if (!closed) inner?.send(msg);
      },
      close() {
        const i = inner;
        inner = null;
        finish(1000, "closed by client");
        i?.close();
      },
    };
  }

  /** Close the connection. A later request chooses a path again. */
  close(): void {
    this.generation++;
    this.selecting = null;
    const current = this.current;
    this.current = null;
    current?.transport.close();
  }

  /** The open connection, or a newly chosen one. */
  private active(): Promise<Selected> {
    const current = this.current;
    if (current?.transport.connected) return Promise.resolve(current);
    if (this.selecting) return this.selecting;
    const generation = this.generation;
    const attempt = (async () => {
      // The old connection ended: let go of it before opening another.
      if (this.current) {
        this.current.transport.close();
        this.current = null;
      }
      const next = await this.select();
      if (generation !== this.generation) {
        next.transport.close();
        throw new ZwiimClientError("network", "Transport closed");
      }
      this.current = next;
      this.setPath(next.path);
      return next;
    })();
    this.selecting = attempt;
    const clear = () => {
      if (this.selecting === attempt) this.selecting = null;
    };
    attempt.then(clear, clear);
    return attempt;
  }

  private async select(): Promise<Selected> {
    switch (this.mode) {
      case "cloud":
        return { path: "relay", transport: await this.openRelay() };
      case "local":
        return { path: "lan", transport: await this.openLan() };
      case "auto": {
        const hasRelay = !!this.options.cloud;
        // Without a relay there is nothing to save time for: always look.
        if (this.options.lan && (!hasRelay || this.now() >= this.lanRetryAt)) {
          try {
            const transport = await this.openLan();
            this.lanMisses = 0;
            this.lanRetryAt = 0;
            return { path: "lan", transport };
          } catch (e) {
            if (!isLanUnavailable(e) || !hasRelay) throw e;
            this.lanRetryAt = this.now() + backoffDelay(this.lanMisses, this.random, this.lanRetry);
            this.lanMisses++;
          }
        }
        return { path: "relay", transport: await this.openRelay() };
      }
    }
  }

  private async openLan(): Promise<LanTransport> {
    const lan = this.options.lan;
    if (!lan) throw new ZwiimClientError("lan_unavailable", "Local network access is not available here");
    const transport = new LanTransport({
      hostId: this.options.hostId,
      hostStaticPubkey: this.options.hostStaticPubkey,
      deviceSecret: this.options.deviceSecret,
      discover: lan.discover,
      dial: lan.dial,
      timeoutMs: this.options.timeoutMs,
      discoverTimeoutMs: this.options.lanDiscoverTimeoutMs,
      connectTimeoutMs: this.options.lanConnectTimeoutMs,
    });
    await transport.connect();
    return transport;
  }

  private async openRelay(): Promise<RelayTransport> {
    const cloud = this.options.cloud;
    if (!cloud) throw new ZwiimClientError("cloud_unavailable", "Zwiim Cloud is not configured");
    const transport = new RelayTransport({
      cloud,
      hostId: this.options.hostId,
      hostStaticPubkey: this.options.hostStaticPubkey,
      deviceSecret: this.options.deviceSecret,
      timeoutMs: this.options.timeoutMs,
      connectTimeoutMs: this.options.relayConnectTimeoutMs,
    });
    await transport.connect();
    return transport;
  }

  private setPath(path: ConnectionPath | null): void {
    if (this.pathValue === path) return;
    this.pathValue = path;
    for (const l of [...this.listeners]) l(path);
  }
}

/**
 * The connection a UI shows, from `HostClient`'s status and the manager's
 * path: `lan` / `relay` only while live.
 */
export function activeConnection(status: ConnectionStatus, path: ConnectionPath | null): ActiveConnection {
  switch (status) {
    case "live":
      return path ?? "connecting";
    case "connecting":
    case "handshaking":
      return "connecting";
    case "reconnecting":
      return "reconnecting";
    default:
      return "offline";
  }
}
