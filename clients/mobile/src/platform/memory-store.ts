import type { HostLanEndpoint } from "@zwiim/sdk";
import type { AccountSession, DeviceIdentity, PinnedHost, SecureStore } from "./capabilities";

/**
 * A `SecureStore` held in this process's memory only: nothing is written
 * anywhere, and everything is gone when the app exits. For tests and the
 * dev screen; the app uses `VaultSecureStore`.
 */
export class MemorySecureStore implements SecureStore {
  #identity: DeviceIdentity | null = null;
  readonly #hosts = new Map<string, PinnedHost>();
  #token: string | null = null;
  #session: AccountSession | null = null;

  async deviceIdentity(): Promise<DeviceIdentity | null> {
    return this.#identity;
  }

  async setDeviceIdentity(identity: DeviceIdentity): Promise<void> {
    this.#identity = { device_id: identity.device_id, keys: { ...identity.keys, secret: identity.keys.secret.slice() } };
  }

  async pinnedHost(hostId: string): Promise<PinnedHost | null> {
    return this.#hosts.get(hostId) ?? null;
  }

  async pinHost(host: PinnedHost): Promise<void> {
    this.#hosts.set(host.host_id, { host_id: host.host_id, host_static_pubkey: host.host_static_pubkey });
  }

  async setHostLanEndpoint(hostId: string, endpoint: HostLanEndpoint | null): Promise<void> {
    const pinned = this.#hosts.get(hostId);
    if (pinned) this.#hosts.set(hostId, { ...pinned, lan_endpoint: endpoint && { ...endpoint } });
  }

  async forgetHost(hostId: string): Promise<void> {
    this.#hosts.delete(hostId);
  }

  async accountToken(): Promise<string | null> {
    return this.#token;
  }

  async setAccountToken(token: string | null): Promise<void> {
    this.#token = token;
  }

  async accountSession(): Promise<AccountSession | null> {
    return this.#session;
  }

  async setAccountSession(session: AccountSession | null): Promise<void> {
    this.#session = session && { ...session };
  }
}
