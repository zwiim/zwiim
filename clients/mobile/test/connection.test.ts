import { CLIENT_REVISION, ZwiimClientError } from "@zwiim/sdk";
import { hello } from "../../sdk/test/fakes";
import { describe, expect, it, vi } from "vitest";
import { errorInfo, type HostConnectionView } from "../src/host/connection";
import type { HostLanEndpoint } from "@zwiim/sdk";
import { ReportedEndpoint } from "../src/platform/lan";
import { waitFor, world } from "./support";

const live = (v: HostConnectionView) => v.status === "live";

describe("HostConnection", () => {
  it("starts disconnected in the chosen mode, contacting nothing", async () => {
    const w = world();
    const c = w.connect({ mode: "cloud" });
    expect(c.view).toEqual({
      mode: "cloud",
      status: "disconnected",
      active: "offline",
      hostId: null,
      error: null,
      events: 0,
      retryAt: null,
      lanKnown: true,
      compat: null,
    });
    await new Promise((r) => setTimeout(r, 10));
    expect(w.cloud.http).toEqual([]);
    expect(w.lan.discoveries).toBe(0);
    c.close();
  });

  it("introduces this app, and an old app against a Host needing a newer one is blocked", async () => {
    const w = world();
    const required = CLIENT_REVISION + 1;
    const introductions: unknown[] = [];
    w.host.backend.handlers["host.hello"] = (params) => {
      introductions.push(params);
      return hello({ revision: required, min_client_revision: required });
    };
    const c = w.connect({ client: { app: "mobile", version: "0.1.0", build: "1", platform: "ios" } });
    c.connect();
    await waitFor(() => c.view.status === "incompatible");
    expect(introductions[0]).toMatchObject({
      client: { app: "mobile", version: "0.1.0", build: "1", platform: "ios", revision: CLIENT_REVISION },
    });
    expect(c.view).toMatchObject({
      error: { code: "update_required" },
      compat: { status: "update_required", update: "client", requiredClientRevision: required },
    });
    // Host operations are refused before they reach the Host; the Host stays healthy.
    const before = w.host.backend.requests.length;
    await expect(c.request("workspace.list", {})).rejects.toMatchObject({ code: "update_required" });
    expect(w.host.backend.requests.length).toBe(before);
    c.close();
  });

  it("automatic: takes the LAN when the host is there, without contacting the Cloud", async () => {
    const w = world();
    const c = w.connect();
    c.connect();
    await waitFor(() => live(c.view));
    expect(c.view).toMatchObject({ mode: "auto", status: "live", active: "lan", hostId: "host-1", error: null });
    expect(w.cloud.http).toEqual([]);
    expect(w.lan.openLinks).toHaveLength(1);
    c.close();
  });

  it("automatic: falls back to the relay when the host is not on the LAN", async () => {
    const w = world({ onLan: false });
    const c = w.connect();
    c.connect();
    await waitFor(() => live(c.view));
    expect(c.view.active).toBe("relay");
    expect(w.cloud.openRelays).toHaveLength(1);
    c.close();
  });

  it("cloud only: never looks on the LAN", async () => {
    const w = world();
    const c = w.connect({ mode: "cloud" });
    c.connect();
    await waitFor(() => live(c.view));
    expect(c.view.active).toBe("relay");
    expect(w.lan.discoveries).toBe(0);
    c.close();
  });

  it("local only: never contacts the Cloud, and reports the LAN miss while retrying", async () => {
    const w = world({ onLan: false });
    const c = w.connect({ mode: "local" });
    c.connect();
    await waitFor(() => c.view.status === "reconnecting");
    expect(c.view).toMatchObject({ active: "reconnecting", error: { code: "lan_unavailable" } });
    // When the next attempt starts, for the Host bar's countdown.
    expect(c.view.retryAt).toBeGreaterThan(Date.now() - 1000);
    expect(w.cloud.http).toEqual([]);
    // The host turns LAN access on: the next attempt finds it.
    w.lan.advertise(w.host);
    await waitFor(() => live(c.view), 3000);
    expect(c.view).toMatchObject({ active: "lan", error: null, retryAt: null });
    c.close();
  });

  it("maps the connection's states for the screen", async () => {
    const w = world();
    const c = w.connect();
    c.connect();
    await waitFor(() => live(c.view));
    const seen = w.views.map((v) => `${v.status}/${v.active}`);
    expect(seen[0]).toBe("connecting/connecting");
    expect(seen).toContain("handshaking/connecting");
    expect(seen.at(-1)).toBe("live/lan");
    // The network drops: reconnecting, then live again.
    w.lan.cut();
    await waitFor(() => c.view.status === "reconnecting");
    expect(c.view.active).toBe("reconnecting");
    await waitFor(() => live(c.view), 3000);
    c.close();
  });

  it("disconnects and reconnects", async () => {
    const w = world();
    const c = w.connect();
    c.connect();
    await waitFor(() => live(c.view));
    c.disconnect();
    expect(c.view).toMatchObject({ status: "disconnected", active: "offline" });
    await waitFor(() => w.lan.openLinks.length === 0);
    c.connect();
    await waitFor(() => live(c.view));
    expect(c.view.active).toBe("lan");
    expect(w.lan.links).toHaveLength(2);
    c.close();
  });

  it("changing the mode while connected reconnects by the new mode", async () => {
    const w = world();
    const c = w.connect();
    c.connect();
    await waitFor(() => c.view.active === "lan");
    c.setMode("cloud");
    expect(c.view.mode).toBe("cloud");
    await waitFor(() => c.view.active === "relay");
    await waitFor(() => w.lan.openLinks.length === 0);
    c.close();
  });

  it("runs requests over the open connection only", async () => {
    const w = world();
    w.host.backend.handlers["workspace.list"] = () => [{ workspace_id: "w1", name: "repo" }];
    const c = w.connect();
    await expect(c.request("host.hello", {})).rejects.toMatchObject({ code: "network" });
    c.connect();
    await waitFor(() => live(c.view));
    expect(await c.request("host.hello", {})).toMatchObject({ host_id: "host-1", protocol: "zwiim/1" });
    expect(await c.request("workspace.list", {})).toEqual([{ workspace_id: "w1", name: "repo" }]);
    c.disconnect();
    await expect(c.request("host.hello", {})).rejects.toMatchObject({ code: "network" });
    c.close();
  });

  it("maps errors to a code and message", async () => {
    const w = world();
    const c = w.connect();
    c.connect();
    await waitFor(() => live(c.view));
    w.host.backend.handlers["agent.list"] = () => {
      throw new ZwiimClientError("not_found", "No such agent", 404);
    };
    const failure = await c.request("agent.list", {}).catch((e: unknown) => errorInfo(e));
    expect(failure).toEqual({ code: "not_found", message: "No such agent" });
    // A host refusal leaves the connection up.
    expect(c.view.status).toBe("live");
    expect(errorInfo(new Error("plain"))).toEqual({ code: "internal", message: "plain" });
    expect(errorInfo("text")).toEqual({ code: "internal", message: "text" });
    c.close();
  });

  it("a host that does not trust this device leaves it unauthenticated", async () => {
    const w = world({ trusted: false });
    const c = w.connect({ mode: "cloud" });
    c.connect();
    await waitFor(() => c.view.status === "unauthenticated");
    expect(c.view).toMatchObject({ active: "offline", error: { code: "device_refused" } });
    c.close();
  });

  it("a key other than the pinned one is refused", async () => {
    const w = world();
    const other = world();
    const c = w.connect({ mode: "local", hostStaticPubkey: other.host.publicKey });
    c.connect();
    await waitFor(() => c.view.status === "error");
    expect(c.view.error?.code).toBe("host_identity_mismatch");
    c.close();
  });

  it("counts host events, and stops reporting once closed", async () => {
    const w = world();
    const c = w.connect();
    c.connect();
    await waitFor(() => c.view.events === 1); // the snapshot
    w.host.backend.feedEvent("workspace.removed", { workspace_id: "w2" });
    await waitFor(() => c.view.events === 2);
    const listener = vi.fn();
    c.subscribe(listener);
    c.close();
    await waitFor(() => w.lan.openLinks.length === 0);
    w.lan.advertise(w.host);
    await new Promise((r) => setTimeout(r, 30));
    expect(listener).not.toHaveBeenCalled();
    expect(c.view.events).toBe(2);
    // Closed for good.
    c.connect();
    await new Promise((r) => setTimeout(r, 30));
    expect(w.lan.links).toHaveLength(1);
  });

  it("reconnects at once when the network comes back", async () => {
    const w = world();
    let notify: (online: boolean) => void = () => {};
    const off = vi.fn();
    const c = w.connect({
      tuning: { backoff: { baseMs: 60_000, maxMs: 60_000 } },
      reachability: {
        subscribe(l) {
          notify = l;
          return off;
        },
      },
    });
    c.connect();
    await waitFor(() => live(c.view));
    w.lan.cut();
    await waitFor(() => c.view.status === "reconnecting");
    notify(true);
    await waitFor(() => live(c.view));
    c.close();
    expect(off).toHaveBeenCalled();
  });
});

describe("the host's LAN endpoint", () => {
  /** A host on the LAN that says where it listens, and a phone that knows `known` of it. */
  function setup(known: HostLanEndpoint | "current" | null = null) {
    const w = world();
    const listener = w.listener!;
    const current = { addresses: [listener.address], port: listener.port };
    w.host.backend.handlers["host.lan_endpoint"] = () => ({ endpoint: current });
    const saved: (HostLanEndpoint | null)[] = [];
    const record = new ReportedEndpoint(
      { host_id: w.host.hostId, host_static_pubkey: w.host.publicKey, lan_endpoint: known === "current" ? current : known },
      async (e) => void saved.push(e),
    );
    const connect = (mode: "auto" | "local") =>
      w.connect({ mode, lan: { discover: record.discover, dial: w.lan.dial }, lanEndpoint: record });
    return { w, current, record, saved, connect };
  }

  it("automatic: first through the Cloud, learning where the host is, then directly", async () => {
    const t = setup();
    const first = t.connect("auto");
    expect(first.view.lanKnown).toBe(false);
    first.connect();
    await waitFor(() => live(first.view));
    expect(first.view.active).toBe("relay");
    await waitFor(() => first.view.lanKnown);
    expect(t.saved).toEqual([t.current]);
    first.close();

    // The next connection (a relaunch, a reconnect) dials it.
    const next = t.connect("auto");
    next.connect();
    await waitFor(() => live(next.view));
    expect(next.view.active).toBe("lan");
    expect(t.w.lan.dials).toEqual([`${t.current.addresses[0]}:${t.current.port}`]);
    next.close();
  });

  it("a stale endpoint: Local only reports the miss; Automatic goes through the Cloud and learns the new one", async () => {
    const t = setup({ addresses: ["192.168.1.99"], port: 1234 });
    const local = t.connect("local");
    local.connect();
    await waitFor(() => local.view.error?.code === "lan_unavailable");
    expect(local.view.lanKnown).toBe(true);
    expect(t.w.cloud.http).toEqual([]);
    local.close();

    const auto = t.connect("auto");
    auto.connect();
    await waitFor(() => live(auto.view));
    expect(auto.view.active).toBe("relay");
    await waitFor(() => t.saved.length === 1);
    expect(t.record.known).toEqual(t.current);
    auto.close();
  });

  it("forgets the endpoint when the host turns LAN access off", async () => {
    const t = setup("current");
    t.w.host.backend.handlers["host.lan_endpoint"] = () => ({ endpoint: null });
    const c = t.connect("auto");
    c.connect();
    await waitFor(() => live(c.view));
    expect(c.view.active).toBe("lan");
    await waitFor(() => !c.view.lanKnown);
    expect(t.saved).toEqual([null]);
    c.close();
  });

  it("keeps what it knows when the host can't say", async () => {
    const t = setup("current");
    delete t.w.host.backend.handlers["host.lan_endpoint"];
    const c = t.connect("auto");
    c.connect();
    await waitFor(() => live(c.view));
    await new Promise((r) => setTimeout(r, 20));
    expect(t.saved).toEqual([]);
    expect(c.view.lanKnown).toBe(true);
    c.close();
  });
});
