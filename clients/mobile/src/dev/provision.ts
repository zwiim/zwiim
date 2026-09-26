// DEV ONLY. Sets this app up against a local dev stack
// (`scripts/dev-stack.mjs`): the stack hands over a Cloud URL, a test
// account token, a host's pairing QR text and its LAN endpoint; the app
// makes its own device key and pairs through the Cloud like a real device.
// Replaced by sign-in and QR pairing in later slices.

import { pairThroughCloud, parsePairingInvitation } from "@zwiim/sdk";
import type { SecureStore } from "../platform/capabilities";
import { cloudFor, type HostTarget } from "../host/open";
import { ensureIdentity } from "../pairing/pairing";

interface Provision {
  cloud_url: string;
  access_token: string;
  /** The host's pairing QR text. */
  invitation: string;
  lan: { address: string; port: number } | null;
}

function parse(body: unknown): Provision {
  const p = body as Partial<Provision> | null;
  const lan = p?.lan;
  if (
    !p ||
    typeof p.cloud_url !== "string" ||
    typeof p.access_token !== "string" ||
    typeof p.invitation !== "string" ||
    (lan != null && (typeof lan.address !== "string" || typeof lan.port !== "number"))
  ) {
    throw new Error("Not a dev provisioning document");
  }
  return { cloud_url: p.cloud_url, access_token: p.access_token, invitation: p.invitation, lan: lan ?? null };
}

/** Fetch the dev stack's provisioning, pair this device, and return the host. */
export async function provisionFromDevStack(
  url: string,
  store: SecureStore,
  fetchImpl: typeof fetch = fetch,
): Promise<HostTarget> {
  const res = await fetchImpl(url);
  if (!res.ok) throw new Error(`Provisioning failed (${res.status})`);
  const p = parse(await res.json());
  await store.setAccountToken(p.access_token);
  const identity = await ensureIdentity(store);
  const invitation = parsePairingInvitation(p.invitation);
  await pairThroughCloud({
    cloud: cloudFor(p.cloud_url, async () => (await store.accountToken()) ?? ""),
    invitation,
    deviceSecret: identity.keys.secret,
    deviceId: identity.device_id,
    deviceName: "Zwiim mobile (dev)",
  });
  await store.pinHost({ host_id: invitation.host_id, host_static_pubkey: invitation.host_static_pubkey });
  return { hostId: invitation.host_id, cloudUrl: p.cloud_url, lanEndpoint: p.lan };
}
