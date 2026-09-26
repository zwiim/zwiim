import { CloudClient, ZwiimClientError, type ConnectionMode, type LanDial } from "@zwiim/sdk";
import type { SecureStore } from "../platform/capabilities";
import { ReportedEndpoint } from "../platform/lan";
import { HostConnection, type HostConnectionConfig } from "./connection";

/** A paired host, and the ways this app may reach it. */
export interface HostTarget {
  hostId: string;
  /** The account's Cloud, for the relay path. */
  cloudUrl: string | null;
  /** Where the host listens on the local network, if known before it says (dev provisioning). */
  lanEndpoint: { address: string; port: number } | null;
}

/** The account's Cloud; `accessToken` is asked for a current token on every call. */
export function cloudFor(url: string, accessToken: () => Promise<string>): CloudClient {
  return new CloudClient({ url, accessToken });
}

/**
 * A connection to a paired host as this device, with its pinned key. Not
 * yet connected.
 */
export async function openHostConnection(
  store: SecureStore,
  target: HostTarget,
  options: {
    mode: ConnectionMode;
    dial: LanDial | null;
    accessToken: () => Promise<string>;
  } & Pick<HostConnectionConfig, "reachability" | "tuning">,
): Promise<HostConnection> {
  const pinned = await store.pinnedHost(target.hostId);
  const identity = await store.deviceIdentity();
  if (!pinned || !identity) throw new ZwiimClientError("device_refused", "This device is not paired with that host");
  const given = target.lanEndpoint && { addresses: [target.lanEndpoint.address], port: target.lanEndpoint.port };
  const lanEndpoint = new ReportedEndpoint({ ...pinned, lan_endpoint: pinned.lan_endpoint ?? given }, (endpoint) =>
    store.setHostLanEndpoint(pinned.host_id, endpoint),
  );
  return new HostConnection({
    hostId: pinned.host_id,
    hostStaticPubkey: pinned.host_static_pubkey,
    deviceSecret: identity.keys.secret,
    cloud: target.cloudUrl ? cloudFor(target.cloudUrl, options.accessToken) : null,
    lan: options.dial ? { discover: lanEndpoint.discover, dial: options.dial } : null,
    lanEndpoint,
    mode: options.mode,
    reachability: options.reachability,
    tuning: options.tuning,
  });
}
