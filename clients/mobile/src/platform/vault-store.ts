// The app's `SecureStore`: each secret is one item in the platform's secure
// storage (`expo-secure-store`: the iOS Keychain, or on Android values
// encrypted with a key held in the Android Keystore). Values are JSON; the
// device's secret key is stored as unpadded base64url and its public key is
// derived again on read.

import { decodeKey, devicePublicKey, encodeKey, type ConnectionMode, type HostLanEndpoint } from "@zwiim/sdk";
import type { AccountSession, DeviceIdentity, LastHost, PinnedHost, Preferences, SecureStore } from "./capabilities";

/** The part of `expo-secure-store` this uses. */
export interface Vault {
  getItemAsync(key: string): Promise<string | null>;
  setItemAsync(key: string, value: string): Promise<void>;
  deleteItemAsync(key: string): Promise<void>;
}

const IDENTITY = "device-identity";
const TOKEN = "account-token";
const SESSION = "account-session";
/** One item per pinned host; host ids are `[A-Za-z0-9_-]`, as item keys allow. */
const pinKey = (hostId: string) => `pin.${hostId}`;
const isHostId = (id: string) => /^[A-Za-z0-9_-]{1,128}$/.test(id);

function parse(raw: string | null): Record<string, unknown> | null {
  if (raw == null) return null;
  try {
    const v: unknown = JSON.parse(raw);
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

const str = (v: unknown): v is string => typeof v === "string" && v.length > 0;

/** A stored LAN endpoint; anything else reads as none. */
function lanEndpoint(v: unknown): HostLanEndpoint | null {
  if (!v || typeof v !== "object") return null;
  const { addresses, port } = v as Record<string, unknown>;
  if (!Array.isArray(addresses) || !addresses.every(str) || !Number.isInteger(port)) return null;
  return { addresses, port: port as number };
}

export class VaultSecureStore implements SecureStore {
  readonly #vault: Vault;

  constructor(vault: Vault) {
    this.#vault = vault;
  }

  /** An unreadable item reads as absent. */
  async deviceIdentity(): Promise<DeviceIdentity | null> {
    const o = parse(await this.#vault.getItemAsync(IDENTITY));
    if (!o || !str(o.device_id) || !str(o.secret)) return null;
    let secret: Uint8Array;
    try {
      secret = decodeKey(o.secret);
    } catch {
      return null;
    }
    return { device_id: o.device_id, keys: { secret, publicKey: devicePublicKey(secret) } };
  }

  async setDeviceIdentity(identity: DeviceIdentity): Promise<void> {
    await this.#vault.setItemAsync(
      IDENTITY,
      JSON.stringify({ device_id: identity.device_id, secret: encodeKey(identity.keys.secret) }),
    );
  }

  async pinnedHost(hostId: string): Promise<PinnedHost | null> {
    if (!isHostId(hostId)) return null;
    const o = parse(await this.#vault.getItemAsync(pinKey(hostId)));
    if (!o || o.host_id !== hostId || !str(o.host_static_pubkey)) return null;
    return { host_id: hostId, host_static_pubkey: o.host_static_pubkey, lan_endpoint: lanEndpoint(o.lan_endpoint) };
  }

  async pinHost(host: PinnedHost): Promise<void> {
    if (!isHostId(host.host_id)) throw new Error("Not a host id");
    await this.#vault.setItemAsync(
      pinKey(host.host_id),
      JSON.stringify({
        host_id: host.host_id,
        host_static_pubkey: host.host_static_pubkey,
        lan_endpoint: host.lan_endpoint ?? null,
      }),
    );
  }

  /** Kept in the host's pin item, so it goes with the pin. */
  async setHostLanEndpoint(hostId: string, endpoint: HostLanEndpoint | null): Promise<void> {
    const pinned = await this.pinnedHost(hostId);
    if (pinned) await this.pinHost({ ...pinned, lan_endpoint: endpoint });
  }

  async forgetHost(hostId: string): Promise<void> {
    if (isHostId(hostId)) await this.#vault.deleteItemAsync(pinKey(hostId));
  }

  async accountToken(): Promise<string | null> {
    return this.#vault.getItemAsync(TOKEN);
  }

  async setAccountToken(token: string | null): Promise<void> {
    if (token == null) await this.#vault.deleteItemAsync(TOKEN);
    else await this.#vault.setItemAsync(TOKEN, token);
  }

  async accountSession(): Promise<AccountSession | null> {
    const o = parse(await this.#vault.getItemAsync(SESSION));
    if (!o || !str(o.user_id) || !str(o.refresh_token)) return null;
    return { user_id: o.user_id, email: typeof o.email === "string" ? o.email : null, refresh_token: o.refresh_token };
  }

  async setAccountSession(session: AccountSession | null): Promise<void> {
    if (session == null) await this.#vault.deleteItemAsync(SESSION);
    else
      await this.#vault.setItemAsync(
        SESSION,
        JSON.stringify({ user_id: session.user_id, email: session.email, refresh_token: session.refresh_token }),
      );
  }
}

const MODES = "connection-modes";
const LAST_HOST = "last-host";
const isMode = (v: unknown): v is ConnectionMode => v === "auto" || v === "local" || v === "cloud";

/** The app's `Preferences`, one JSON item each in the same vault. */
export class VaultPreferences implements Preferences {
  readonly #vault: Vault;

  constructor(vault: Vault) {
    this.#vault = vault;
  }

  /** Unreadable entries read as absent. */
  async connectionModes(): Promise<Record<string, ConnectionMode>> {
    const o = parse(await this.#vault.getItemAsync(MODES)) ?? {};
    const modes: Record<string, ConnectionMode> = {};
    for (const [hostId, mode] of Object.entries(o)) if (isHostId(hostId) && isMode(mode)) modes[hostId] = mode;
    return modes;
  }

  async setConnectionMode(hostId: string, mode: ConnectionMode): Promise<void> {
    if (!isHostId(hostId)) throw new Error("Not a host id");
    const modes = await this.connectionModes();
    modes[hostId] = mode;
    await this.#vault.setItemAsync(MODES, JSON.stringify(modes));
  }

  async lastHost(): Promise<LastHost | null> {
    const o = parse(await this.#vault.getItemAsync(LAST_HOST));
    if (!o || !str(o.host_id) || !isHostId(o.host_id)) return null;
    return { host_id: o.host_id, name: typeof o.name === "string" ? o.name : null };
  }

  async setLastHost(host: LastHost | null): Promise<void> {
    if (host == null) await this.#vault.deleteItemAsync(LAST_HOST);
    else await this.#vault.setItemAsync(LAST_HOST, JSON.stringify({ host_id: host.host_id, name: host.name }));
  }
}
