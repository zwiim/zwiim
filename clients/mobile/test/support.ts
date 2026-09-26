// A paired host on a fake network and a fake Cloud, from the SDK's own test
// fakes: the real tunnel, the real ConnectionManager and HostClient.

import { CloudClient, generateDeviceKeys } from "@zwiim/sdk";
import { FakeLan } from "../../sdk/test/lan-fakes";
import { FakeCloud, FakeHost } from "../../sdk/test/relay-fakes";
import { HostConnection, type HostConnectionConfig, type HostConnectionView } from "../src/host/connection";
import type { Vault } from "../src/platform/vault-store";

export { waitFor } from "../../sdk/test/relay-fakes";

export const TUNING = { backoff: { baseMs: 5, maxMs: 20 }, timeoutMs: 2000, lanRetry: { baseMs: 60_000, maxMs: 60_000 } };

export function world(opts: { onLan?: boolean; trusted?: boolean } = {}) {
  const cloud = new FakeCloud();
  const host = new FakeHost();
  cloud.addHost(host);
  const lan = new FakeLan();
  const listener = opts.onLan === false ? null : lan.advertise(host);
  const device = generateDeviceKeys();
  if (opts.trusted !== false) host.trusted.add(device.publicKey);
  const cloudClient = () =>
    new CloudClient({ url: cloud.origin, accessToken: "token-u1", fetch: cloud.fetch, WebSocket: cloud.WebSocket });
  const views: HostConnectionView[] = [];
  const connect = (config: Partial<HostConnectionConfig> = {}) => {
    const c = new HostConnection({
      hostId: host.hostId,
      hostStaticPubkey: host.publicKey,
      deviceSecret: device.secret,
      cloud: cloudClient(),
      lan: { discover: lan.discover, dial: lan.dial },
      tuning: TUNING,
      ...config,
    });
    c.subscribe((v) => views.push(v));
    return c;
  };
  return { cloud, host, lan, listener, device, connect, views };
}

/** The platform's secure storage: survives the app, as a Map outlives the store instances. */
export class MapVault implements Vault {
  readonly items = new Map<string, string>();
  async getItemAsync(key: string) {
    return this.items.get(key) ?? null;
  }
  async setItemAsync(key: string, value: string) {
    this.items.set(key, value);
  }
  async deleteItemAsync(key: string) {
    this.items.delete(key);
  }
}
