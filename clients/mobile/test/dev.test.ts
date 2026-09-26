import { afterEach, describe, expect, it, vi } from "vitest";
import { runChecks, type CheckResult } from "../src/dev/checks";
import { provisionFromDevStack } from "../src/dev/provision";
import { openHostConnection } from "../src/host/open";
import { MemorySecureStore } from "../src/platform/memory-store";
import { TUNING, world } from "./support";

const PROVISION = "http://127.0.0.1:9/abc/provision";
const token = async () => "token-u1";

/** The dev stack's provisioning for `w`'s host, and the phone's globals pointed at `w`'s Cloud. */
function stack(w: ReturnType<typeof world>) {
  const invitation = JSON.stringify(w.host.invite());
  const doc = {
    cloud_url: w.cloud.origin,
    access_token: "token-u1",
    invitation,
    lan: w.listener ? { address: w.listener.address, port: w.listener.port } : null,
  };
  vi.stubGlobal("fetch", w.cloud.fetch);
  vi.stubGlobal("WebSocket", w.cloud.WebSocket);
  const fetchProvision = (async (url: string) => {
    expect(url).toBe(PROVISION);
    return new Response(JSON.stringify(doc), { status: 200 });
  }) as unknown as typeof fetch;
  return { doc, fetchProvision };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("dev provisioning", () => {
  it("pairs a new device key through the Cloud and pins the host", async () => {
    const w = world({ trusted: false });
    const { fetchProvision } = stack(w);
    const store = new MemorySecureStore();
    const target = await provisionFromDevStack(PROVISION, store, fetchProvision);
    expect(target).toEqual({
      hostId: "host-1",
      cloudUrl: w.cloud.origin,
      lanEndpoint: { address: w.listener!.address, port: w.listener!.port },
    });
    const identity = await store.deviceIdentity();
    expect(identity?.device_id).toMatch(/^mobile-[0-9a-f]{16}$/);
    expect(w.host.trusted.has(identity!.keys.publicKey)).toBe(true);
    expect(await store.pinnedHost("host-1")).toEqual({ host_id: "host-1", host_static_pubkey: w.host.publicKey });
    expect(await store.accountToken()).toBe("token-u1");
  });

  it("refuses a document that is not one", async () => {
    const store = new MemorySecureStore();
    const bad = (async () => new Response(JSON.stringify({ cloud_url: 1 }), { status: 200 })) as unknown as typeof fetch;
    await expect(provisionFromDevStack(PROVISION, store, bad)).rejects.toThrow("Not a dev provisioning document");
    expect(await store.accountToken()).toBeNull();
  });
});

describe("dev checks", () => {
  it("pass in every mode, each over its path", async () => {
    const w = world({ trusted: false });
    w.host.backend.handlers["workspace.list"] = () => [{ workspace_id: "w1", name: "repo" }];
    const { fetchProvision } = stack(w);
    const store = new MemorySecureStore();
    const target = await provisionFromDevStack(PROVISION, store, fetchProvision);
    const marks: string[] = [];
    const reported: CheckResult[] = [];
    const results = await runChecks(
      (mode) => openHostConnection(store, target, { mode, dial: w.lan.dial, accessToken: token, tuning: TUNING }),
      (r) => reported.push(r),
      async (e) => {
        marks.push(`${e} http=${w.cloud.http.length}`);
      },
    );
    expect(results.filter((r) => !r.ok)).toEqual([]);
    expect(reported).toEqual(results);
    expect(results.map((r) => r.step)).toEqual(
      ["auto", "cloud", "local"].flatMap((m) =>
        ["connect", "host.hello", "workspace.list", "events", "disconnect", "reconnect"].map((s) => `${m}: ${s}`),
      ),
    );
    expect(results.find((r) => r.step === "auto: connect")?.detail).toMatch(/^live over lan/);
    expect(results.find((r) => r.step === "cloud: connect")?.detail).toMatch(/^live over relay/);
    expect(results.find((r) => r.step === "local: workspace.list")?.detail).toBe("1 workspace(s)");
    // Automatic and local-only never contacted the Cloud.
    const http = (m: string) => Number(marks.find((x) => x.startsWith(m))!.split("http=")[1]);
    expect(http("auto:end")).toBe(http("auto:start"));
    expect(http("local:end")).toBe(http("local:start"));
    expect(http("cloud:end")).toBeGreaterThan(http("cloud:start"));
  });

  it("report a failure with its code, and go on", async () => {
    const w = world({ trusted: false, onLan: false });
    const { fetchProvision } = stack(w);
    const store = new MemorySecureStore();
    const target = await provisionFromDevStack(PROVISION, store, fetchProvision);
    const results = await runChecks(
      (mode) => openHostConnection(store, target, { mode, dial: w.lan.dial, accessToken: token, tuning: TUNING }),
      () => {},
      undefined,
      500,
    );
    // No LAN: automatic uses the relay (not the expected LAN); local cannot connect.
    expect(results.find((r) => r.step === "auto: connect")).toMatchObject({ ok: false, detail: "internal: took relay, expected lan" });
    expect(results.find((r) => r.step === "cloud: reconnect")?.ok).toBe(true);
    expect(results.find((r) => r.step === "local: connect")?.ok).toBe(false);
  });
});

describe("opening a host connection", () => {
  it("needs a paired host", async () => {
    const store = new MemorySecureStore();
    await expect(
      openHostConnection(store, { hostId: "host-1", cloudUrl: null, lanEndpoint: null }, { mode: "auto", dial: null, accessToken: token }),
    ).rejects.toMatchObject({ code: "device_refused" });
  });
});
