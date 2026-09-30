// The account's hosts, as the Cloud reports them (`GET /v1/hosts`), each
// marked with whether this phone is paired with it. Presence is the Cloud's:
// online now, or the last heartbeat it saw.

import { isClientError, type ActiveConnection, type CloudClient, type CloudHost, type ConnectionMode } from "@zwiim/sdk";
import type { SecureStore } from "../platform/capabilities";

export interface HostEntry {
  host_id: string;
  name: string | null;
  /** The key the host enrolled with. The pairing QR's key must match it. */
  static_pubkey: string;
  online: boolean;
  /** Unix seconds. */
  last_seen_at: number | null;
  /** This phone holds a pinned key for it. */
  paired: boolean;
}

/**
 * The account's hosts, online first, then by most recently seen.
 * `renewToken` is asked for a fresh access token once if the Cloud refuses
 * the current one.
 */
export async function accountHosts(
  cloud: CloudClient,
  store: SecureStore,
  renewToken: () => Promise<unknown>,
): Promise<HostEntry[]> {
  let hosts: CloudHost[];
  try {
    hosts = await cloud.hosts();
  } catch (e) {
    if (!isClientError(e, "unauthenticated")) throw e;
    await renewToken();
    hosts = await cloud.hosts();
  }
  const entries = await Promise.all(
    hosts.map(async (h) => ({
      host_id: h.host_id,
      name: h.name,
      static_pubkey: h.static_pubkey,
      online: h.online,
      last_seen_at: h.last_seen_at,
      paired: (await store.pinnedHost(h.host_id)) !== null,
    })),
  );
  return entries.sort((a, b) => Number(b.online) - Number(a.online) || (b.last_seen_at ?? 0) - (a.last_seen_at ?? 0));
}

/** What a host is called on screen. */
export function hostName(host: { host_id: string; name: string | null }): string {
  return host.name?.trim() || `Host ${host.host_id.slice(0, 8)}`;
}

/** "5m ago", "2h ago", "3d ago". */
export function ago(seconds: number, now: number): string {
  const d = Math.max(0, now - seconds);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86_400) return `${Math.floor(d / 3600)}h ago`;
  return `${Math.floor(d / 86_400)}d ago`;
}

/** The last host opened first; the rest keep their order. */
export function byLastUsed(hosts: HostEntry[], lastHostId: string | null): HostEntry[] {
  const last = hosts.find((h) => h.host_id === lastHostId);
  return last ? [last, ...hosts.filter((h) => h !== last)] : hosts;
}

/**
 * A host row's mono meta line. `now` in Unix seconds. `active` is this
 * phone's connection to the host, if it has one open: only a live LAN
 * connection says "same network"; nothing else guesses it.
 */
export function hostMeta(
  host: HostEntry,
  now: number,
  { mode = "auto", active = null }: { mode?: ConnectionMode; active?: ActiveConnection | null } = {},
): string {
  if (!host.paired) return "not paired on this phone";
  switch (active) {
    case "lan":
      return "⌂ same network · paired";
    case "relay":
      return "☁\uFE0E via Zwiim Cloud · paired";
    case "connecting":
      return "connecting…";
    case "reconnecting":
      return "reconnecting…";
  }
  if (host.online) return mode === "local" ? "online · paired" : "☁\uFE0E via Zwiim Cloud · paired";
  return host.last_seen_at ? `offline · last seen ${ago(host.last_seen_at, now)}` : "offline";
}

/**
 * Setting this phone up, while it is paired with none of the account's
 * hosts: the Mac to pair (online first), or null while no Mac has turned on
 * remote access yet. Undefined once a host is paired: the list takes over.
 */
export function macToPair(hosts: HostEntry[]): HostEntry | null | undefined {
  if (hosts.some((h) => h.paired)) return undefined;
  return hosts.find((h) => h.online) ?? hosts[0] ?? null;
}
