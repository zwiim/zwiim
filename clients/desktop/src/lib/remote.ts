// Hosts on other computers, reached as this device: directly on this
// network, or through Zwiim Cloud.
//
// The account's Cloud calls go through the native shell, which adds the
// access token (the UI never holds it). The device's identity and its
// pairings live in the OS keychain; the secret is read into memory to run
// the tunnel's handshake. A remote host is driven by the same SDK client and
// controller as the local one — only the transport differs — so every panel
// works unchanged. Which path a connection takes is the SDK's
// `ConnectionManager`'s choice, by the mode the user set.

import {
  CloudClient,
  ConnectionManager,
  decodeKey,
  HostClient,
  HostController,
  pairThroughCloud,
  parsePairingInvitation,
  ZwiimClientError,
  type CloudHost,
  type ConnectionMode,
} from "@zwiim/sdk";
import type { PairedHost, RemoteBridge } from "./bridge";

/**
 * The SDK's Cloud client, its HTTP calls answered by the shell. The token
 * it would send is a placeholder: the shell attaches the real one.
 */
export function cloudClient(remote: RemoteBridge, url: string): CloudClient {
  const proxy = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const method = init?.method === "POST" ? "POST" : "GET";
    let reply;
    try {
      reply = await remote.cloudRequest(method, new URL(String(input)).pathname);
    } catch {
      // As a failed fetch: the SDK reports `cloud_unavailable`.
      throw new TypeError("Zwiim Cloud unreachable");
    }
    return new Response(JSON.stringify(reply.body), {
      status: reply.status,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;
  return new CloudClient({ url, accessToken: "native", fetch: proxy });
}

/** The account's hosts, with presence. */
export function listCloudHosts(remote: RemoteBridge, url: string): Promise<CloudHost[]> {
  return cloudClient(remote, url).hosts();
}

/** A connection to a host on another computer. */
export interface RemoteConnection {
  controller: HostController;
  /** Chooses and reports the path: `lan` or `relay`. */
  manager: ConnectionManager;
}

/**
 * Connect to a paired host, by `mode`: directly on this network, through
 * the Cloud (`cloudUrl`; none without one), or the first that works. The
 * client keeps the connection itself (reconnects, resumes, snapshots after
 * a restart).
 */
export async function connectRemote(
  remote: RemoteBridge,
  cloudUrl: string | null,
  hostId: string,
  mode: ConnectionMode,
): Promise<RemoteConnection> {
  const pin = (await remote.pairedHosts()).find((h) => h.host_id === hostId);
  if (!pin) throw new ZwiimClientError("device_refused", "This computer is not paired with that host");
  const key = await remote.deviceIdentity();
  const manager = new ConnectionManager({
    mode,
    hostId,
    hostStaticPubkey: pin.host_static_pubkey,
    deviceSecret: decodeKey(key.secret),
    lan: remote.lan ?? null,
    cloud: cloudUrl ? cloudClient(remote, cloudUrl) : null,
  });
  const client = new HostClient({ expectedHostId: hostId, transport: manager });
  const controller = new HostController(client);
  client.start();
  return { controller, manager };
}

/**
 * Pair this device with a host from its pairing QR's text, through the
 * Cloud, and pin the host's key. `expectedHostId`: the host the user chose.
 */
export async function pairRemote(
  remote: RemoteBridge,
  cloudUrl: string,
  qrText: string,
  options: { expectedHostId?: string; name?: string | null } = {},
): Promise<PairedHost> {
  const invitation = parsePairingInvitation(qrText);
  if (options.expectedHostId && invitation.host_id !== options.expectedHostId) {
    throw new ZwiimClientError("pairing_invalid", "This pairing code is for a different host");
  }
  const key = await remote.deviceIdentity();
  await pairThroughCloud({
    cloud: cloudClient(remote, cloudUrl),
    invitation,
    deviceSecret: decodeKey(key.secret),
    deviceId: key.device_id,
    deviceName: key.device_name,
  });
  const pinned: PairedHost = {
    host_id: invitation.host_id,
    host_static_pubkey: invitation.host_static_pubkey,
    name: options.name ?? null,
    paired_at: Math.floor(Date.now() / 1000),
  };
  await remote.pinHost(pinned);
  return pinned;
}

/** A host's display name. */
export function hostLabel(host: { host_id: string; name?: string | null }): string {
  return host.name || `Host ${host.host_id.slice(0, 8)}`;
}
