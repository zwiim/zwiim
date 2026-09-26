import { ZwiimClientError } from "../client/errors";
import { decodeKey, encodeKey } from "../remote/keys";
import { connectTunnel, type ByteChannel, type Tunnel } from "../remote/tunnel";
import { deviceSecret, pinnedKey } from "./relay";
import { TunnelTransport } from "./tunneled";

/**
 * One host's LAN advertisement, as discovery reports it: the DNS-SD
 * instance `<host_id>._zwiim._tcp.local.` resolved to its addresses, port
 * and TXT record (`v`, `id`, `pk`).
 */
export interface LanAdvertisement {
  /** The DNS-SD instance name: the host id. */
  instance: string;
  port: number;
  addresses: string[];
  txt: Record<string, string>;
}

/** A validated advertisement: where a host says it can be dialed. */
export interface LanEndpoint {
  hostId: string;
  /** The host's static public key as advertised (base64url). Untrusted. */
  hostStaticPubkey: string;
  addresses: string[];
  port: number;
}

/**
 * The platform's LAN discovery: advertisements seen for `hostId` within
 * `timeoutMs`. Only an input: every one is validated and checked against
 * the pinned key before use.
 */
export type LanDiscover = (hostId: string, options: { timeoutMs: number }) => Promise<unknown[]>;

/** The platform's TCP dialer: a byte stream to `address:port`. */
export type LanDial = (
  endpoint: { address: string; port: number },
  options: { timeoutMs: number },
) => Promise<ByteChannel>;

export interface LanTransportOptions {
  /** The host to reach. */
  hostId: string;
  /** The host's X25519 static public key, pinned at pairing. */
  hostStaticPubkey: string | Uint8Array;
  /** This device's X25519 static secret, whose public key the host trusts. */
  deviceSecret: Uint8Array;
  discover: LanDiscover;
  dial: LanDial;
  /** Default request timeout. */
  timeoutMs?: number;
  /** How long to look for the host on the network. */
  discoverTimeoutMs?: number;
  /** Dial + Noise handshake budget, per address. */
  connectTimeoutMs?: number;
}

const DEFAULT_TIMEOUT_MS = 60_000;
const DEFAULT_DISCOVER_TIMEOUT_MS = 2_000;
const DEFAULT_CONNECT_TIMEOUT_MS = 4_000;
const TXT_VERSION = "1";
const MAX_ADDRESSES = 8;

/** Whether `error` only means the host cannot be reached directly. */
export function isLanUnavailable(error: unknown): boolean {
  return error instanceof ZwiimClientError && error.code === "lan_unavailable";
}

const unavailable = (message = "The host is not reachable on this network") =>
  new ZwiimClientError("lan_unavailable", message);

/**
 * Validate one advertisement. `null` for anything malformed: a wrong
 * version, an instance name that is not the TXT `id`, a key that is not 32
 * bytes, a bad port, or no usable address. Says nothing about whether the
 * host is genuine: that is the pinned key's and the handshake's job.
 */
export function parseLanAdvertisement(raw: unknown): LanEndpoint | null {
  if (!raw || typeof raw !== "object") return null;
  const ad = raw as Partial<Record<keyof LanAdvertisement, unknown>>;
  const txt = ad.txt as Record<string, unknown> | undefined;
  if (!txt || typeof txt !== "object") return null;
  if (txt.v !== TXT_VERSION) return null;
  const hostId = txt.id;
  if (typeof hostId !== "string" || !/^[A-Za-z0-9-]{1,63}$/.test(hostId)) return null;
  if (ad.instance !== hostId) return null;
  if (typeof txt.pk !== "string") return null;
  let key: Uint8Array;
  try {
    key = decodeKey(txt.pk);
  } catch {
    return null;
  }
  if (key.length !== 32) return null;
  const port = ad.port;
  if (typeof port !== "number" || !Number.isInteger(port) || port < 1 || port > 65535) return null;
  if (!Array.isArray(ad.addresses)) return null;
  const addresses = [...new Set(ad.addresses.filter(isUsableAddress))].slice(0, MAX_ADDRESSES);
  if (addresses.length === 0) return null;
  return { hostId, hostStaticPubkey: encodeKey(key), addresses, port };
}

/** A unicast IPv4 literal, or an IPv6 one. Never a name to resolve. */
function isUsableAddress(address: unknown): address is string {
  if (typeof address !== "string") return false;
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(address);
  if (v4) {
    const octets = v4.slice(1).map(Number);
    if (octets.some((o) => o > 255)) return false;
    const [a] = octets as [number];
    // Not unspecified, multicast or broadcast.
    return a !== 0 && a < 224;
  }
  if (!/^[0-9A-Fa-f:.]+(%[\w.-]+)?$/.test(address) || !address.includes(":")) return false;
  const lower = address.toLowerCase();
  return lower !== "::" && !lower.startsWith("ff");
}

/**
 * A host reached directly on the local network: the same device tunnel
 * the Cloud relay carries, over a TCP connection to the host's advertised
 * endpoint. The host is found by discovery, but trusted only as the
 * pinned key says: an advertisement with another key is refused
 * (`host_identity_mismatch`) rather than used, and the Noise handshake
 * proves the host holds the pinned key. The host's device check and
 * revocation apply as through the Cloud.
 *
 * Fails with `lan_unavailable` when the host cannot be reached directly
 * (not advertised, or no advertised address answers); with
 * `host_identity_mismatch`, `handshake_failed` or `device_refused` when it
 * answers but the identity or trust check fails.
 */
export class LanTransport extends TunnelTransport {
  private readonly hostId: string;
  private readonly hostKey: Uint8Array;
  private readonly deviceSecret: Uint8Array;
  private readonly discover: LanDiscover;
  private readonly dial: LanDial;
  private readonly discoverTimeoutMs: number;
  private readonly connectTimeoutMs: number;

  constructor(options: LanTransportOptions) {
    super(options.timeoutMs ?? DEFAULT_TIMEOUT_MS);
    this.hostId = options.hostId;
    this.hostKey = pinnedKey(options.hostStaticPubkey);
    this.deviceSecret = deviceSecret(options.deviceSecret);
    this.discover = options.discover;
    this.dial = options.dial;
    this.discoverTimeoutMs = options.discoverTimeoutMs ?? DEFAULT_DISCOVER_TIMEOUT_MS;
    this.connectTimeoutMs = options.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS;
  }

  protected async openTunnel(): Promise<Tunnel> {
    const endpoints = await this.find();
    let lastError: unknown = null;
    for (const endpoint of endpoints) {
      for (const address of endpoint.addresses) {
        try {
          return await this.connectTo(address, endpoint.port);
        } catch (e) {
          // The host answered and refused us, or is not the pinned host:
          // another address will not change that.
          if (!isLanUnavailable(e)) throw e;
          lastError = e;
        }
      }
    }
    throw lastError ?? unavailable();
  }

  /** The host's advertisements, checked against the pinned key. */
  private async find(): Promise<LanEndpoint[]> {
    let found: unknown[];
    try {
      found = await this.discover(this.hostId, { timeoutMs: this.discoverTimeoutMs });
    } catch {
      throw unavailable("Local network discovery is unavailable");
    }
    const pinned = encodeKey(this.hostKey);
    const endpoints = (Array.isArray(found) ? found : [])
      .map(parseLanAdvertisement)
      .filter((e): e is LanEndpoint => e?.hostId === this.hostId);
    if (endpoints.some((e) => e.hostStaticPubkey !== pinned)) {
      // Someone on this network claims this host's id with another key.
      // Never switch to it, and never quietly go around it.
      throw new ZwiimClientError(
        "host_identity_mismatch",
        "A host on this network claims this host's identity with a different key",
      );
    }
    if (endpoints.length === 0) throw unavailable();
    return endpoints;
  }

  private async connectTo(address: string, port: number): Promise<Tunnel> {
    const deadline = Date.now() + this.connectTimeoutMs;
    let channel: ByteChannel;
    try {
      channel = await this.dial({ address, port }, { timeoutMs: this.connectTimeoutMs });
    } catch {
      throw unavailable();
    }
    try {
      return await connectTunnel(
        channel,
        {
          hostId: this.hostId,
          deviceSecret: this.deviceSecret,
          hostStaticPubkey: this.hostKey,
          timeoutMs: Math.max(1, deadline - Date.now()),
        },
        // Closed before any reply: nothing that holds the pinned key
        // answered there.
        () => unavailable(),
      );
    } catch (e) {
      if (e instanceof ZwiimClientError && (e.code === "device_refused" || e.code === "handshake_failed")) {
        throw e;
      }
      throw unavailable();
    }
  }
}
