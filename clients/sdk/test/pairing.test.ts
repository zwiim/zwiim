import { describe, expect, it } from "vitest";
import { HostClient } from "../src/client/host-client";
import { CloudClient } from "../src/remote/cloud";
import { encodeKey, generateDeviceKeys } from "../src/remote/keys";
import { pairThroughCloud, parsePairingInvitation } from "../src/remote/pairing";
import { RelayTransport } from "../src/transport/relay";
import { FakeCloud, FakeHost, waitFor } from "./relay-fakes";

const NOW = Date.parse("2026-09-26T12:00:00Z");
const valid = {
  v: 2,
  host_id: "host-1",
  host_static_pubkey: encodeKey(new Uint8Array(32).fill(7)),
  code: "abc123",
  expires_at: "2026-09-26T12:05:00Z",
};
const text = (o: object) => JSON.stringify(o);
const code = (f: () => unknown) => {
  try {
    f();
  } catch (e) {
    return (e as { code?: string }).code;
  }
  return "ok";
};

describe("pairing QR payload", () => {
  it("reads a v2 invitation", () => {
    expect(parsePairingInvitation(`  ${text(valid)}\n`, NOW)).toEqual(valid);
  });

  it("rejects anything else", () => {
    expect(code(() => parsePairingInvitation("not json", NOW))).toBe("pairing_invalid");
    expect(code(() => parsePairingInvitation("[]", NOW))).toBe("pairing_invalid");
    expect(code(() => parsePairingInvitation(text({ ...valid, v: 1 }), NOW))).toBe("pairing_invalid");
    // A v1 QR (addresses, bearer credential pairing) is not accepted.
    expect(code(() => parsePairingInvitation(text({ ...valid, addresses: ["http://10.0.0.2:3080"] }), NOW))).toBe(
      "pairing_invalid",
    );
    expect(code(() => parsePairingInvitation(text({ ...valid, host_id: "bad id!" }), NOW))).toBe("pairing_invalid");
    expect(code(() => parsePairingInvitation(text({ ...valid, host_static_pubkey: "short" }), NOW))).toBe(
      "pairing_invalid",
    );
    expect(code(() => parsePairingInvitation(text({ ...valid, code: "" }), NOW))).toBe("pairing_invalid");
    expect(code(() => parsePairingInvitation(text({ ...valid, expires_at: "soon" }), NOW))).toBe("pairing_invalid");
  });

  it("rejects an expired code", () => {
    expect(code(() => parsePairingInvitation(text(valid), Date.parse(valid.expires_at)))).toBe("pairing_expired");
  });
});

function setup() {
  const cloud = new FakeCloud();
  const host = new FakeHost();
  cloud.addHost(host);
  const cloudClient = new CloudClient({
    url: cloud.origin,
    accessToken: "token-u1",
    fetch: cloud.fetch,
    WebSocket: cloud.WebSocket,
  });
  const device = generateDeviceKeys();
  const pair = (invitation = host.invite(), keys = device, deviceId = "laptop-1") =>
    pairThroughCloud({
      cloud: cloudClient,
      invitation,
      deviceSecret: keys.secret,
      deviceId,
      deviceName: "Laptop",
      timeoutMs: 2000,
    });
  return { cloud, host, cloudClient, device, pair };
}

describe("pairing through the Cloud", () => {
  it("pairs the device's key, then connects like any trusted device", async () => {
    const t = setup();
    const invitation = t.host.invite();
    await t.pair(invitation);
    expect(t.host.trusted.has(t.device.publicKey)).toBe(true);
    expect(t.host.paired.get(t.device.publicKey)).toEqual({ device_id: "laptop-1", device_name: "Laptop" });
    // The code never crossed the relay readably.
    const relayed = new TextDecoder("latin1").decode(Uint8Array.from(t.cloud.relayed.flatMap((b) => [...b])));
    expect(relayed).not.toContain(invitation.code);

    const client = new HostClient({
      transport: new RelayTransport({
        cloud: t.cloudClient,
        hostId: invitation.host_id,
        hostStaticPubkey: invitation.host_static_pubkey,
        deviceSecret: t.device.secret,
      }),
      backoff: { baseMs: 5, maxMs: 20 },
    });
    client.start();
    await waitFor(() => client.state.status === "live");
    client.stop();
  });

  it("refuses a used, unknown or expired code", async () => {
    const t = setup();
    const invitation = t.host.invite();
    await t.pair(invitation);
    // Replayed by another device.
    const other = generateDeviceKeys();
    await expect(t.pair(invitation, other, "laptop-2")).rejects.toMatchObject({ code: "pairing_rejected" });
    expect(t.host.trusted.has(other.publicKey)).toBe(false);
    // A code the host never issued.
    await expect(t.pair({ ...t.host.invite(), code: "forged" }, other, "laptop-2")).rejects.toMatchObject({
      code: "pairing_rejected",
    });
    // Expired on the device's clock: nothing is sent.
    const before = t.cloud.http.length;
    await expect(t.pair(t.host.invite(-1), other, "laptop-2")).rejects.toMatchObject({ code: "pairing_expired" });
    expect(t.cloud.http.length).toBe(before);
    // Expired on the host's clock.
    const late = { ...t.host.invite(-1), expires_at: new Date(Date.now() + 60_000).toISOString() };
    await expect(t.pair(late, other, "laptop-2")).rejects.toMatchObject({ code: "pairing_rejected" });
    expect(t.host.trusted.has(other.publicKey)).toBe(false);
  });

  it("refuses a QR whose host key disagrees with the account's host", async () => {
    const t = setup();
    const forged = { ...t.host.invite(), host_static_pubkey: generateDeviceKeys().publicKey };
    await expect(t.pair(forged)).rejects.toMatchObject({ code: "pairing_invalid" });
    // Refused before any ticket: the code was not spent.
    expect(t.cloud.http.some((r) => r.path.endsWith("/tickets"))).toBe(false);
    expect(t.host.codes.has(forged.code)).toBe(true);
  });

  it("refuses a host outside the account", async () => {
    const t = setup();
    const stranger = new FakeHost("host-2");
    t.cloud.addHost(stranger, "u2");
    await expect(t.pair(stranger.invite())).rejects.toMatchObject({ code: "host_unknown" });
    expect(stranger.trusted.size).toBe(0);
  });

  it("reports an offline host and an unreachable Cloud as such", async () => {
    const t = setup();
    t.cloud.setOnline("host-1", false);
    await expect(t.pair()).rejects.toMatchObject({ code: "host_offline" });
    t.cloud.down = true;
    await expect(t.pair()).rejects.toMatchObject({ code: "cloud_unavailable" });
  });

  it("refuses an invalid device id before contacting anyone", async () => {
    const t = setup();
    await expect(t.pair(t.host.invite(), t.device, "bad id")).rejects.toMatchObject({ code: "invalid_input" });
    expect(t.cloud.http).toEqual([]);
  });
});
