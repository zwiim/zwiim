import { ZwiimClientError, type ConnectionMode } from "@zwiim/sdk";
import { describe, expect, it } from "vitest";
import type { HostEntry } from "../src/account/hosts";
import type { HostConnectionView } from "../src/host/connection";
import { CurrentHost, type CurrentHostState } from "../src/host/current";
import { connectionText, hostProblem, hostRowState } from "../src/host/status";
import { MemorySecureStore } from "../src/platform/memory-store";
import { VaultPreferences } from "../src/platform/vault-store";
import { MapVault, waitFor, world } from "./support";

function setup(opts: Parameters<typeof world>[0] = {}) {
  const w = world(opts);
  const vault = new MapVault();
  const store = new MemorySecureStore();
  const opened: ConnectionMode[] = [];
  const launch = () =>
    new CurrentHost({
      store,
      prefs: new VaultPreferences(vault),
      open: async (hostId, mode) => {
        if (!(await store.pinnedHost(hostId))) throw new ZwiimClientError("device_refused", "Not paired");
        opened.push(mode);
        return w.connect({ mode });
      },
    });
  const entry: HostEntry = {
    host_id: w.host.hostId,
    name: "Studio Mac",
    static_pubkey: w.host.publicKey,
    online: true,
    last_seen_at: 1,
    paired: true,
  };
  return { w, vault, store, opened, launch, entry, prefs: new VaultPreferences(vault) };
}

const pin = (s: ReturnType<typeof setup>) => s.store.pinHost({ host_id: s.entry.host_id, host_static_pubkey: s.entry.static_pubkey });

describe("the current host", () => {
  it("connects in the host's kept mode, asks who it is, and is remembered", async () => {
    const s = setup({ onLan: false });
    await pin(s);
    await s.prefs.setConnectionMode(s.entry.host_id, "cloud");
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.hello !== null);
    expect(current.state.view).toMatchObject({ mode: "cloud", status: "live", active: "relay" });
    expect(current.state.hello).toMatchObject({ host_id: s.entry.host_id });
    expect(await s.prefs.lastHost()).toEqual({ host_id: s.entry.host_id, name: "Studio Mac" });
    await current.clear();
  });

  it("keeps the connection mode chosen for the host across a restart", async () => {
    const s = setup({ onLan: false });
    await pin(s);
    const first = s.launch();
    await first.open(s.entry);
    await waitFor(() => first.state.view?.status === "live");
    await first.setMode("local");
    expect(first.state.view?.mode).toBe("local");
    // No LAN path: Local only says so, and never falls back to the Cloud.
    await waitFor(() => first.state.view?.error?.code === "lan_unavailable");
    expect(hostProblem(first.state.view, null, "Studio Mac")).toEqual({
      kind: "banner",
      text: "Studio Mac isn't on this network. Switch to Automatic to connect through Zwiim Cloud.",
      action: "switch",
    });
    first.retry();

    // Relaunched: the same host opens in Local only again.
    const again = s.launch();
    await again.open(s.entry);
    expect(again.state.view?.mode).toBe("local");
    expect(s.opened).toEqual(["auto", "local"]);
    await again.setMode("auto");
    await waitFor(() => again.state.view?.status === "live");
    expect(await s.prefs.connectionModes()).toEqual({ [s.entry.host_id]: "auto" });
    await again.clear();
    await first.clear();
  });

  it("keeps one connection: the same host again reuses it, another replaces it", async () => {
    const s = setup();
    await pin(s);
    // Another host of the account (served by the same fake, reached by its own connection).
    const other = { ...s.entry, host_id: "host-2", name: "Office Mac" };
    await s.store.pinHost({ host_id: "host-2", host_static_pubkey: s.entry.static_pubkey });
    const current = s.launch();
    await current.select(s.entry);
    await waitFor(() => current.state.view?.status === "live");
    current.chooseWorkspace("w2");
    await current.select({ ...s.entry, online: false });
    expect(s.opened).toHaveLength(1);
    expect(current.state.workspaceId).toBe("w2");
    expect(current.state.host?.online).toBe(false);
    expect(current.state.view?.status).toBe("live");
    expect(s.w.lan.openLinks).toHaveLength(1);

    // The Host switcher: the bar names the new host at once, not yet connected.
    const switching = current.select(other);
    expect(current.state.host?.name).toBe("Office Mac");
    expect(current.state.view).toBeNull();
    // Workspaces are the host's own: the new one starts from its default.
    expect(current.state.workspaceId).toBeNull();
    await switching;
    expect(s.opened).toHaveLength(2);
    await waitFor(() => current.state.view?.status === "live");
    // The first connection is closed: one link open, the new one.
    await waitFor(() => s.w.lan.openLinks.length === 1);
    await current.clear();
  });

  it("forgets the pairing a host dropped, and asks to pair again", async () => {
    const s = setup({ trusted: false });
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await waitFor(() => current.state.view?.status === "unauthenticated");
    expect(current.state.view?.error?.code).toBe("device_refused");
    await waitFor(() => current.state.host?.paired === false);
    expect(await s.store.pinnedHost(s.entry.host_id)).toBeNull();
    expect(hostProblem(current.state.view, null, "Studio Mac")).toEqual({
      kind: "screen",
      title: "This phone was removed from Studio Mac",
      body: "Pair again to reconnect.",
    });
    await current.clear();
  });

  it("can't open a host this phone isn't paired with", async () => {
    const s = setup();
    const current = s.launch();
    await current.open(s.entry);
    expect(current.state.openError?.code).toBe("device_refused");
    expect(current.state.view).toBeNull();
    expect(hostProblem(null, current.state.openError, "Studio Mac")).toMatchObject({
      kind: "screen",
      title: "This phone isn't paired with Studio Mac",
    });
  });

  it("on sign-out, disconnects and forgets the last host, keeping pairings and modes", async () => {
    const s = setup();
    await pin(s);
    const current = s.launch();
    await current.open(s.entry);
    await current.setMode("cloud");
    await waitFor(() => current.state.view?.status === "live");
    const seen: CurrentHostState[] = [];
    current.subscribe(() => seen.push(current.state));
    await current.clear();
    expect(current.state).toEqual({ host: null, view: null, hello: null, openError: null, store: null, workspaceId: null });
    expect(await s.prefs.lastHost()).toBeNull();
    expect(await s.prefs.connectionModes()).toEqual({ [s.entry.host_id]: "cloud" });
    expect(await s.store.pinnedHost(s.entry.host_id)).not.toBeNull();
    // Closed: nothing more is reported.
    await new Promise((r) => setTimeout(r, 30));
    expect(seen).toHaveLength(1);
  });
});

describe("how the connection reads", () => {
  const view = (over: Partial<HostConnectionView>): HostConnectionView => ({
    mode: "auto",
    status: "live",
    active: "relay",
    hostId: "h",
    error: null,
    events: 0,
    retryAt: null,
    lanKnown: true,
    ...over,
  });

  it("shows the path as a pill, never a color, and counts down a reconnect", () => {
    expect(connectionText(view({ active: "lan" }))).toEqual({ status: "Connected locally", pill: "⌂ local", dot: "done" });
    expect(connectionText(view({}))).toEqual({ status: "Connected through Zwiim Cloud", pill: "☁ cloud", dot: "done" });
    expect(connectionText(view({ status: "connecting", active: "connecting" })).pill).toBe("connecting…");
    const retrying = view({ status: "reconnecting", active: "reconnecting", retryAt: 10_000 });
    expect(connectionText(retrying, 7_200)).toEqual({ status: "Reconnecting…", pill: "reconnecting · 3s", dot: "work" });
    expect(connectionText(retrying, 10_000).pill).toBe("reconnecting");
    expect(connectionText(view({ status: "error", active: "offline" }))).toEqual({ status: "Offline", pill: "offline", dot: "idle" });
    expect(connectionText(null).pill).toBe("offline");
  });

  it("names every failure and its next step", () => {
    const failed = (code: string, over: Partial<HostConnectionView> = {}) =>
      hostProblem(view({ status: "reconnecting", active: "reconnecting", error: { code, message: "m" }, ...over }), null, "Mac");
    expect(failed("host_offline")).toEqual({ kind: "banner", text: "Mac is offline. Showing nothing until it's back.", action: "retry" });
    expect(failed("cloud_unavailable", { mode: "cloud" })).toMatchObject({ text: "Zwiim Cloud is unreachable. Local connections still work." });
    // No claim about the direct path while this phone doesn't know where to
    // dial, or in Automatic, where it was just tried and missed.
    expect(failed("cloud_unavailable", { mode: "cloud", lanKnown: false })).toMatchObject({ text: "Zwiim Cloud is unreachable." });
    expect(failed("cloud_unavailable")).toMatchObject({ text: "Zwiim Cloud is unreachable." });
    expect(failed("host_identity_mismatch", { status: "error" })).toMatchObject({ kind: "screen", title: "Couldn't verify Mac" });
    expect(failed("handshake_failed", { status: "error" })).toMatchObject({ kind: "screen", title: "Couldn't verify Mac" });
    expect(failed("unauthenticated", { status: "unauthenticated" })).toMatchObject({ action: "sign_in" });
    // Local only's LAN miss is a banner there; in Automatic the Cloud takes over.
    expect(failed("lan_unavailable", { mode: "local" })).toMatchObject({ text: expect.stringContaining("Mac isn't on this network"), action: "switch" });
    expect(failed("lan_unavailable", { mode: "local", lanKnown: false })).toMatchObject({
      text: "This phone doesn't know where Mac is on the network yet. Switch to Automatic to connect through Zwiim Cloud.",
      action: "switch",
    });
    expect(failed("lan_unavailable")).toBeNull();
    // A retryable failure shows in the pill; a stopped one asks for Retry.
    expect(failed("network")).toBeNull();
    expect(failed("network", { status: "error" })).toMatchObject({ text: "Couldn't connect to Mac. Try again.", action: "retry" });
    expect(hostProblem(view({}), null, "Mac")).toBeNull();
    expect(hostProblem(null, { code: "internal", message: "m" }, "Mac")).toMatchObject({ action: "retry" });
  });

  it("describes the current host's row by this phone's own connection", () => {
    const host: HostEntry = { host_id: "h", name: "Mac", static_pubkey: "pk", online: true, last_seen_at: 100, paired: true };
    const state = (over: Partial<CurrentHostState>): CurrentHostState => ({ host, view: null, hello: null, openError: null, store: null, workspaceId: null, ...over });
    expect(hostRowState(host, state({}), undefined, 200)).toEqual({ meta: "connecting…", dot: "work" });
    expect(hostRowState(host, state({ view: view({ active: "lan" }) }), undefined, 200)).toEqual({
      meta: "⌂ same network · paired",
      dot: "done",
    });
    const offline = { ...host, online: false };
    const retrying = view({ status: "reconnecting", active: "reconnecting" });
    expect(hostRowState(offline, state({ host: offline, view: retrying }), undefined, 7300)).toEqual({
      meta: "offline · last seen 2h ago",
      dot: "idle",
    });
    // Another host: the Cloud's presence.
    const other = { ...host, host_id: "o" };
    expect(hostRowState(other, state({ view: view({ active: "lan" }) }), "local", 200)).toEqual({
      meta: "online · paired",
      dot: "done",
    });
  });
});
