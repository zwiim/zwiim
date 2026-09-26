import { generateDeviceKeys } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import { VaultPreferences, VaultSecureStore } from "../src/platform/vault-store";
import { MapVault } from "./support";

describe("VaultSecureStore", () => {
  it("keeps the identity, pins and account across app restarts", async () => {
    const vault = new MapVault();
    const before = new VaultSecureStore(vault);
    const keys = generateDeviceKeys();
    await before.setDeviceIdentity({ device_id: "mobile-1", keys });
    await before.pinHost({ host_id: "host-1", host_static_pubkey: "pk-1" });
    await before.setAccountToken("access-1");
    await before.setAccountSession({ user_id: "u1", email: "u1@example.com", refresh_token: "refresh-1" });

    // A new process: a new store over the same platform storage.
    const after = new VaultSecureStore(vault);
    const identity = await after.deviceIdentity();
    expect(identity?.device_id).toBe("mobile-1");
    expect([...identity!.keys.secret]).toEqual([...keys.secret]);
    expect(identity!.keys.publicKey).toBe(keys.publicKey);
    expect(await after.pinnedHost("host-1")).toEqual({ host_id: "host-1", host_static_pubkey: "pk-1", lan_endpoint: null });
    expect(await after.pinnedHost("host-2")).toBeNull();
    expect(await after.accountToken()).toBe("access-1");
    expect(await after.accountSession()).toEqual({ user_id: "u1", email: "u1@example.com", refresh_token: "refresh-1" });
  });

  it("stores the secret key once, never the public key, one item per host", async () => {
    const vault = new MapVault();
    const store = new VaultSecureStore(vault);
    const keys = generateDeviceKeys();
    await store.setDeviceIdentity({ device_id: "mobile-1", keys });
    await store.pinHost({ host_id: "host-1", host_static_pubkey: "pk-1" });
    await store.pinHost({ host_id: "host-2", host_static_pubkey: "pk-2" });
    expect([...vault.items.keys()].sort()).toEqual(["device-identity", "pin.host-1", "pin.host-2"]);
    expect(JSON.parse(vault.items.get("device-identity")!)).toEqual({
      device_id: "mobile-1",
      secret: expect.stringMatching(/^[A-Za-z0-9_-]{43}$/),
    });
  });

  it("forgets the account on sign-out, keeping the device and its pairings", async () => {
    const vault = new MapVault();
    const store = new VaultSecureStore(vault);
    await store.setDeviceIdentity({ device_id: "mobile-1", keys: generateDeviceKeys() });
    await store.pinHost({ host_id: "host-1", host_static_pubkey: "pk-1" });
    await store.setAccountToken("access-1");
    await store.setAccountSession({ user_id: "u1", email: null, refresh_token: "r" });
    await store.setAccountToken(null);
    await store.setAccountSession(null);
    expect([...vault.items.keys()].sort()).toEqual(["device-identity", "pin.host-1"]);
    expect(await store.accountSession()).toBeNull();
  });

  it("keeps a host's LAN endpoint with its pin, until the pin goes or is replaced", async () => {
    const vault = new MapVault();
    const store = new VaultSecureStore(vault);
    const endpoint = { addresses: ["192.168.1.20"], port: 7000 };
    // Not pinned: nothing kept.
    await store.setHostLanEndpoint("host-1", endpoint);
    expect(vault.items.size).toBe(0);

    await store.pinHost({ host_id: "host-1", host_static_pubkey: "pk-1" });
    await store.setHostLanEndpoint("host-1", endpoint);
    expect(await new VaultSecureStore(vault).pinnedHost("host-1")).toEqual({ host_id: "host-1", host_static_pubkey: "pk-1", lan_endpoint: endpoint });
    await store.setHostLanEndpoint("host-1", null);
    expect((await store.pinnedHost("host-1"))?.lan_endpoint).toBeNull();

    // Pairing again starts over.
    await store.setHostLanEndpoint("host-1", endpoint);
    await store.pinHost({ host_id: "host-1", host_static_pubkey: "pk-2" });
    expect((await store.pinnedHost("host-1"))?.lan_endpoint).toBeNull();
    // A damaged endpoint reads as none; the pin still reads.
    vault.items.set("pin.host-1", JSON.stringify({ host_id: "host-1", host_static_pubkey: "pk-2", lan_endpoint: { addresses: "x", port: 1 } }));
    expect(await store.pinnedHost("host-1")).toEqual({ host_id: "host-1", host_static_pubkey: "pk-2", lan_endpoint: null });
  });

  it("forgets one host's pin, keeping the rest", async () => {
    const store = new VaultSecureStore(new MapVault());
    await store.pinHost({ host_id: "host-1", host_static_pubkey: "pk-1" });
    await store.pinHost({ host_id: "host-2", host_static_pubkey: "pk-2" });
    await store.forgetHost("host-1");
    await store.forgetHost("../x");
    expect(await store.pinnedHost("host-1")).toBeNull();
    expect(await store.pinnedHost("host-2")).not.toBeNull();
  });

  it("reads damaged items as absent", async () => {
    const vault = new MapVault();
    const store = new VaultSecureStore(vault);
    vault.items.set("device-identity", "{not json");
    vault.items.set("pin.host-1", JSON.stringify({ host_id: "host-2", host_static_pubkey: "pk" }));
    vault.items.set("account-session", JSON.stringify({ user_id: "u1" }));
    expect(await store.deviceIdentity()).toBeNull();
    expect(await store.pinnedHost("host-1")).toBeNull();
    expect(await store.accountSession()).toBeNull();
    vault.items.set("device-identity", JSON.stringify({ device_id: "d", secret: "!!" }));
    expect(await store.deviceIdentity()).toBeNull();
  });

  it("refuses host ids the platform could not key", async () => {
    const store = new VaultSecureStore(new MapVault());
    await expect(store.pinHost({ host_id: "../x", host_static_pubkey: "pk" })).rejects.toThrow("Not a host id");
    expect(await store.pinnedHost("a b")).toBeNull();
  });
});

describe("VaultPreferences", () => {
  it("keeps each host's connection mode and the last host across app restarts", async () => {
    const vault = new MapVault();
    const before = new VaultPreferences(vault);
    expect(await before.connectionModes()).toEqual({});
    expect(await before.lastHost()).toBeNull();
    await before.setConnectionMode("host-1", "cloud");
    await before.setConnectionMode("host-2", "local");
    await before.setConnectionMode("host-1", "auto");
    await before.setLastHost({ host_id: "host-2", name: "Studio Mac" });

    const after = new VaultPreferences(vault);
    expect(await after.connectionModes()).toEqual({ "host-1": "auto", "host-2": "local" });
    expect(await after.lastHost()).toEqual({ host_id: "host-2", name: "Studio Mac" });
    await after.setLastHost(null);
    expect(await after.lastHost()).toBeNull();
    // One item for all modes; nothing else is written.
    expect([...vault.items.keys()].sort()).toEqual(["connection-modes"]);
  });

  it("reads damaged entries as absent", async () => {
    const vault = new MapVault();
    const prefs = new VaultPreferences(vault);
    vault.items.set("connection-modes", JSON.stringify({ "host-1": "cloud", "host-2": "sideways", "bad id": "auto" }));
    vault.items.set("last-host", "{");
    expect(await prefs.connectionModes()).toEqual({ "host-1": "cloud" });
    expect(await prefs.lastHost()).toBeNull();
    vault.items.set("connection-modes", "[]");
    expect(await prefs.connectionModes()).toEqual({});
    await expect(prefs.setConnectionMode("bad id", "auto")).rejects.toThrow("Not a host id");
  });
});
