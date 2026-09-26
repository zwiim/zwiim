// Pairing a device with a host through Zwiim Cloud, from the host's v2 QR.
//
// The QR (`device.pair_invite` on the host) carries the host's id, its
// static key and a one-time code — no address, no reusable secret. The device
// pins that key and opens the tunnel with the code in its first handshake
// message; the host redeems the code for the key the device proves it holds,
// and from then on trusts the device. The Cloud only relays the handshake.

import { ZwiimClientError } from "../client/errors";
import type { KeyPairingInvitation } from "../types/protocol";
import { isValidHostId, relayCloseError, type CloudClient } from "./cloud";
import { decodeKey } from "./keys";
import { connectTunnel } from "./tunnel";

const FIELDS = ["v", "host_id", "host_static_pubkey", "code", "expires_at"];
const MAX_CODE_LEN = 128;
const MAX_DEVICE_ID_LEN = 64;
const DEFAULT_TIMEOUT_MS = 30_000;

const invalid = (message: string) => new ZwiimClientError("pairing_invalid", message);

/** A device id the host accepts (`is_valid_device_id`). */
export function isValidDeviceId(deviceId: string): boolean {
  return deviceId.length > 0 && deviceId.length <= MAX_DEVICE_ID_LEN && /^[A-Za-z0-9_-]+$/.test(deviceId);
}

/**
 * Read a host's pairing QR payload (its JSON text). Throws `pairing_invalid`
 * for anything that is not exactly a v2 invitation, and `pairing_expired`
 * once its code has expired.
 */
export function parsePairingInvitation(text: string, now: number = Date.now()): KeyPairingInvitation {
  let raw: unknown;
  try {
    raw = JSON.parse(text.trim());
  } catch {
    throw invalid("This is not a Zwiim pairing code");
  }
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) throw invalid("This is not a Zwiim pairing code");
  const o = raw as Record<string, unknown>;
  if (Object.keys(o).some((k) => !FIELDS.includes(k))) throw invalid("This is not a Zwiim pairing code");
  if (o.v !== 2) throw invalid("This pairing code is for a different version of Zwiim");
  const { host_id, host_static_pubkey, code, expires_at } = o;
  if (typeof host_id !== "string" || !isValidHostId(host_id)) throw invalid("The pairing code names no valid host");
  if (typeof host_static_pubkey !== "string") throw invalid("The pairing code carries no host key");
  try {
    decodeKey(host_static_pubkey);
  } catch {
    throw invalid("The pairing code's host key is invalid");
  }
  if (typeof code !== "string" || code.length === 0 || code.length > MAX_CODE_LEN) {
    throw invalid("The pairing code is malformed");
  }
  const expires = typeof expires_at === "string" ? Date.parse(expires_at) : NaN;
  if (Number.isNaN(expires)) throw invalid("The pairing code has no valid expiry");
  if (expires <= now) {
    throw new ZwiimClientError("pairing_expired", "This pairing code has expired. Show a new one on the host.");
  }
  return { v: 2, host_id, host_static_pubkey, code, expires_at: expires_at as string };
}

export interface PairThroughCloudOptions {
  cloud: CloudClient;
  invitation: KeyPairingInvitation;
  /** This device's X25519 static secret; the host trusts its public key. */
  deviceSecret: Uint8Array;
  deviceId: string;
  deviceName?: string;
  timeoutMs?: number;
  now?: () => number;
}

/**
 * Pair this device with the invitation's host through the Cloud relay. The
 * host must be one of the account's, enrolled with the key the QR pins. On
 * success the host trusts this device; pin `invitation.host_static_pubkey`
 * and connect with `RelayTransport`.
 */
export async function pairThroughCloud(options: PairThroughCloudOptions): Promise<void> {
  const { cloud, invitation, deviceSecret, deviceId } = options;
  const now = options.now?.() ?? Date.now();
  if (Date.parse(invitation.expires_at) <= now) {
    throw new ZwiimClientError("pairing_expired", "This pairing code has expired. Show a new one on the host.");
  }
  if (!isValidDeviceId(deviceId)) throw new ZwiimClientError("invalid_input", "Invalid device id");
  const hostKey = decodeKey(invitation.host_static_pubkey);

  const host = (await cloud.hosts()).find((h) => h.host_id === invitation.host_id);
  if (!host) {
    throw new ZwiimClientError("host_unknown", "This pairing code is for a host that is not in your account");
  }
  // The QR is the trust anchor; the Cloud's record must agree with it.
  if (host.static_pubkey !== invitation.host_static_pubkey) {
    throw invalid("This pairing code does not match the host registered to your account");
  }

  const hello = new TextEncoder().encode(
    JSON.stringify({
      pair: { code: invitation.code, device_id: deviceId, device_name: options.deviceName ?? null },
    }),
  );
  const { ticket } = await cloud.ticket(invitation.host_id);
  const channel = await cloud.dial(ticket);
  try {
    const tunnel = await connectTunnel(
      channel,
      {
        hostId: invitation.host_id,
        deviceSecret,
        hostStaticPubkey: hostKey,
        timeoutMs: options.timeoutMs ?? DEFAULT_TIMEOUT_MS,
        hello,
      },
      relayCloseError,
    );
    tunnel.close();
  } catch (e) {
    if (e instanceof ZwiimClientError && e.code === "device_refused") {
      throw new ZwiimClientError(
        "pairing_rejected",
        "The host refused this pairing code. It may have expired or been used already; show a new one on the host.",
      );
    }
    throw e;
  }
}
