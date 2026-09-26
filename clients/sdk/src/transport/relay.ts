import { relayCloseError, type CloudClient } from "../remote/cloud";
import { decodeKey } from "../remote/keys";
import { connectTunnel, type Tunnel } from "../remote/tunnel";
import { TunnelTransport } from "./tunneled";

export interface RelayTransportOptions {
  /** The account's Cloud connection. */
  cloud: CloudClient;
  /** The host to reach, from `cloud.hosts()` or pairing. */
  hostId: string;
  /**
   * The host's X25519 static public key (base64url or raw), pinned at
   * pairing. Never taken from the Cloud: it is what keeps the Cloud out of
   * the tunnel.
   */
  hostStaticPubkey: string | Uint8Array;
  /** This device's X25519 static secret, whose public key the host trusts. */
  deviceSecret: Uint8Array;
  /** Default request timeout. */
  timeoutMs?: number;
  /** Ticket + relay + Noise handshake budget. */
  connectTimeoutMs?: number;
}

const DEFAULT_TIMEOUT_MS = 60_000;
const DEFAULT_CONNECT_TIMEOUT_MS = 30_000;

/**
 * A host reached through Zwiim Cloud: the same `/zwiim/v1` requests and
 * event socket as `LocalTransport`, carried in the device's end-to-end
 * encrypted tunnel over the Cloud relay.
 *
 * The tunnel is opened on first use (ticket → relay → Noise handshake) and
 * shared by requests and event streams. When it drops, in-flight requests
 * fail (`unknown_outcome` if mutating) and open event streams close; the
 * next request opens a new tunnel with a fresh ticket. Nothing is resent.
 */
export class RelayTransport extends TunnelTransport {
  private readonly cloud: CloudClient;
  private readonly hostId: string;
  private readonly hostKey: Uint8Array;
  private readonly deviceSecret: Uint8Array;
  private readonly connectTimeoutMs: number;

  constructor(options: RelayTransportOptions) {
    super(options.timeoutMs ?? DEFAULT_TIMEOUT_MS);
    this.cloud = options.cloud;
    this.hostId = options.hostId;
    this.hostKey = pinnedKey(options.hostStaticPubkey);
    this.deviceSecret = deviceSecret(options.deviceSecret);
    this.connectTimeoutMs = options.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS;
  }

  /** A fresh ticket, a relay socket, and the handshake. */
  protected async openTunnel(): Promise<Tunnel> {
    const deadline = Date.now() + this.connectTimeoutMs;
    const { ticket } = await this.cloud.ticket(this.hostId);
    const channel = await this.cloud.dial(ticket);
    return connectTunnel(
      channel,
      {
        hostId: this.hostId,
        deviceSecret: this.deviceSecret,
        hostStaticPubkey: this.hostKey,
        timeoutMs: Math.max(1, deadline - Date.now()),
      },
      relayCloseError,
    );
  }
}

/** The pinned host key, raw. */
export function pinnedKey(key: string | Uint8Array): Uint8Array {
  const raw = typeof key === "string" ? decodeKey(key) : key;
  if (raw.length !== 32) throw new Error("hostStaticPubkey must be 32 bytes");
  return raw;
}

export function deviceSecret(secret: Uint8Array): Uint8Array {
  if (secret.length !== 32) throw new Error("deviceSecret must be 32 bytes");
  return secret;
}
