// The Desktop as a remote client: hosts listed from the account's Cloud,
// pairing from a host's QR text, and the same workbench driven through the
// relay tunnel or directly on the local network. The Cloud, the network and
// the remote host are the SDK's fakes, which speak the real Noise tunnel;
// this computer's host is a plain fake.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeKey, generateDeviceKeys, type DeviceInfo, type WorkspaceInfo } from "@zwiim/sdk";
import { App } from "@/App";
import type { NativeBridge, PairedHost } from "@/lib/bridge";
import { FakeLan } from "../../sdk/test/lan-fakes";
import { FakeCloud, FakeHost } from "../../sdk/test/relay-fakes";
import { FakeTransport } from "./fakes";

const ws = (id: string): WorkspaceInfo => ({
  workspace_id: id,
  name: `repo-${id}`,
  alias: null,
  kind: "root",
  parent_workspace_id: null,
  branch: "main",
  base_branch: null,
  base_commit: null,
  removed: false,
});

const me = { user_id: "u1", email: "me@example.com" };
const WAIT = { timeout: 4000 };
const RELAY = "Connected through Zwiim Cloud";
const LAN = "Connected locally";

/**
 * Everything outside the webview: the Cloud, the hosts, the keychain; with
 * `lan`, the local network the shell can look on and dial.
 */
function world({ lan: onLan = false } = {}) {
  const cloud = new FakeCloud();
  const lan = new FakeLan();
  const studio = new FakeHost("host-studio");
  studio.backend.handlers["workspace.list"] = () => [ws("w-remote")];
  studio.backend.handlers["workspace.tree"] = () => [];
  const attic = new FakeHost("host-attic");
  const own = new FakeHost("h-local");
  cloud.addHost(studio, "u1", "Studio");
  cloud.addHost(attic, "u1", "Attic");
  cloud.addHost(own, "u1", "This Mac");
  cloud.setOnline("host-attic", false);

  const local = new FakeTransport();
  local.handlers["host.hello"] = () => ({ host_id: "h-local", protocol: "zwiim/1", version: "0", owner: null });
  local.handlers["workspace.list"] = () => [ws("w-local")];
  local.handlers["workspace.tree"] = () => [];
  local.handlers["host.claim"] = (p) => ({ owner: { user_id: p.user_id, email: p.email ?? null } });

  const env = {
    cloud,
    lan,
    studio,
    attic,
    local,
    device: generateDeviceKeys(),
    /** The Supabase session the shell keeps; null when signed out. */
    token: "token-u1" as string | null,
    /** The keychain's pinned hosts. */
    pins: [] as PairedHost[],
    bridge: null as unknown as NativeBridge,
  };
  env.bridge = {
    discover: vi.fn(async () => ({ host_id: "h-local", url: "http://127.0.0.1:1", token: "t", pid: 7, running: true })),
    startHost: vi.fn(async () => {}),
    pickFolder: vi.fn(async () => null),
    accountStatus: vi.fn(async () => ({ available: true, account: env.token ? me : null, cloud_url: cloud.origin })),
    signIn: vi.fn(async () => {
      env.token = "token-u1";
      return me;
    }),
    cancelSignIn: vi.fn(async () => {}),
    signOut: vi.fn(async () => {
      env.token = null;
    }),
    remote: {
      // As the shell: the token is added here, never by the UI.
      cloudRequest: vi.fn(async (method: "GET" | "POST", path: string) => {
        if (!env.token) return { status: 401, body: { error: { code: "unauthenticated" } } };
        const res = await cloud.fetch(`${cloud.origin}${path}`, {
          method,
          headers: { authorization: `Bearer ${env.token}` },
        });
        return { status: res.status, body: JSON.parse(await res.text()) };
      }),
      deviceIdentity: vi.fn(async () => ({
        device_id: "desktop-test",
        device_name: "Test Mac",
        secret: encodeKey(env.device.secret),
      })),
      pairedHosts: vi.fn(async () => [...env.pins]),
      pinHost: vi.fn(async (h: PairedHost) => {
        env.pins = [...env.pins.filter((p) => p.host_id !== h.host_id), h];
      }),
      forgetHost: vi.fn(async (id: string) => {
        env.pins = env.pins.filter((p) => p.host_id !== id);
      }),
      lan: onLan ? { discover: lan.discover, dial: lan.dial } : undefined,
    },
  };
  return env;
}

type World = ReturnType<typeof world>;

function mount(env: World) {
  return render(<App bridge={env.bridge} bootOptions={{ makeTransport: () => env.local, pollMs: 1 }} />);
}

const sidebar = () => within(screen.getByTestId("sidebar"));
const currentHost = () => screen.getByTestId("current-host").textContent;
const badge = () => within(screen.getByTestId("connection-status")).getAllByText(/./)[0]!.textContent;

async function openPicker() {
  fireEvent.click(screen.getByTitle("Choose the host this window shows"));
  return within(await screen.findByRole("listbox", { name: "Hosts" }));
}

async function pasteCode(text: string) {
  const dialog = within(await screen.findByRole("dialog"));
  fireEvent.change(dialog.getByLabelText("Pairing code"), { target: { value: text } });
  fireEvent.click(dialog.getByText("Pair"));
}

/** Pair with Studio from the picker and wait until it is live on `path`. */
async function pairStudio(env: World, path = RELAY) {
  const list = await openPicker();
  fireEvent.click(await list.findByText("Studio"));
  await pasteCode(JSON.stringify(env.studio.invite()));
  await waitFor(() => expect(currentHost()).toBe("Studio"), WAIT);
  await screen.findByText("repo-w-remote", {}, WAIT);
  await waitFor(() => expect(badge()).toBe(path), WAIT);
}

const chooseMode = (mode: string) =>
  fireEvent.change(screen.getByLabelText("Connection mode"), { target: { value: mode } });

beforeEach(() => {
  window.localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

function setup(options: { lan?: boolean } = {}) {
  const env = world(options);
  vi.stubGlobal("WebSocket", env.cloud.WebSocket);
  mount(env);
  return env;
}

describe("hosts", () => {
  it("lists the account's hosts; this computer's own is always reached locally", async () => {
    setup();
    await screen.findByText("repo-w-local");
    expect(currentHost()).toBe("This computer");
    const list = await openPicker();
    const studio = (await list.findByText("Studio")).closest("button")!;
    expect(within(studio).getByText("Pair…")).toBeTruthy();
    expect(within(studio).getByLabelText("online")).toBeTruthy();
    const attic = list.getByText("Attic").closest("button")!;
    expect(within(attic).getByLabelText("offline")).toBeTruthy();
    // The account's entry for this computer's host is "This computer".
    expect(list.queryByText("This Mac")).toBeNull();
    expect(list.getAllByRole("option")).toHaveLength(3);
  });

  it("asks to sign in before listing remote hosts", async () => {
    const env = world();
    env.token = null;
    vi.stubGlobal("WebSocket", env.cloud.WebSocket);
    mount(env);
    await screen.findByText("repo-w-local");
    const list = await openPicker();
    expect(await list.findByText("Sign in to reach your other hosts.")).toBeTruthy();
    expect(list.queryByText("Studio")).toBeNull();
    expect(env.cloud.http).toEqual([]);
  });

  it("does not offer a paired host the signed-in account does not own", async () => {
    const env = world();
    const foreign = new FakeHost("host-foreign");
    env.cloud.addHost(foreign, "u2", "Theirs");
    env.pins = [{ host_id: "host-foreign", host_static_pubkey: foreign.publicKey, name: "Theirs", paired_at: 1 }];
    vi.stubGlobal("WebSocket", env.cloud.WebSocket);
    mount(env);
    await screen.findByText("repo-w-local");
    const list = await openPicker();
    const row = (await list.findByText("Theirs")).closest("button")!;
    expect(row.hasAttribute("disabled")).toBe(true);
    expect(within(row).getByText("Not in this account")).toBeTruthy();
  });
});

describe("pairing", () => {
  it("pairs from the QR text, pins the host's key and connects through the relay", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    expect(env.pins).toEqual([
      expect.objectContaining({ host_id: "host-studio", host_static_pubkey: env.studio.publicKey, name: "Studio" }),
    ]);
    expect(env.studio.paired.get(env.device.publicKey)).toEqual({ device_id: "desktop-test", device_name: "Test Mac" });
    // This computer's host is gone from view; nothing of it is mixed in.
    expect(screen.queryByText("repo-w-local")).toBeNull();
    // The device's secret is nowhere in web storage.
    const secret = encodeKey(env.device.secret);
    for (let i = 0; i < window.localStorage.length; i++) {
      const k = window.localStorage.key?.(i);
      if (k) expect(window.localStorage.getItem(k)).not.toContain(secret);
    }
    expect(JSON.stringify(window.localStorage)).not.toContain(secret);
  });

  it("refuses expired, forged, mismatched and reused codes, pinning nothing", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    const other = generateDeviceKeys().publicKey;
    const cases: [string, RegExp][] = [
      [JSON.stringify(env.studio.invite(-1000)), /expired/],
      [JSON.stringify({ ...env.studio.invite(), host_static_pubkey: other }), /does not match the host/],
      [JSON.stringify({ ...env.studio.invite(), code: "forged" }), /refused this pairing code/],
      [JSON.stringify(env.attic.invite()), /different host/],
      ["https://10.0.0.2:3080/pair?token=abc", /not a Zwiim pairing code/],
    ];
    for (const [text, message] of cases) {
      const list = await openPicker();
      fireEvent.click(await list.findByText("Studio"));
      await pasteCode(text);
      expect(await screen.findByText(message, {}, WAIT)).toBeTruthy();
      fireEvent.click(within(screen.getByRole("dialog")).getByText("Cancel"));
    }
    expect(env.pins).toEqual([]);
    expect(env.studio.trusted.size).toBe(0);

    // A code works once.
    const invite = JSON.stringify(env.studio.invite());
    const list = await openPicker();
    fireEvent.click(await list.findByText("Studio"));
    await pasteCode(invite);
    await waitFor(() => expect(currentHost()).toBe("Studio"), WAIT);
    const again = await openPicker();
    fireEvent.click(again.getByText("Pair with a host…"));
    await pasteCode(invite);
    expect(await screen.findByText(/refused this pairing code/, {}, WAIT)).toBeTruthy();
  });

  it("keeps the pairing and the chosen host across a restart", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    const handshakes = env.studio.handshakes;
    cleanup();
    mount(env);
    await screen.findByText("repo-w-remote", {}, WAIT);
    expect(currentHost()).toBe("Studio");
    expect(env.studio.paired.size).toBe(1);
    expect(env.studio.handshakes).toBeGreaterThan(handshakes);
  });

  it("falls back to this computer when a remembered host is no longer paired", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    cleanup();
    env.pins = [];
    mount(env);
    await screen.findByText("repo-w-local", {}, WAIT);
    expect(currentHost()).toBe("This computer");
  });
});

describe("remote host", () => {
  it("runs the same workbench through the tunnel", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    env.studio.backend.handlers["session.list"] = () => [
      { session_id: "s1", workspace_id: "w-remote", agent: "claude_code", title: "Fix the build", status: "idle" },
    ];
    await pairStudio(env);
    const ops = () => env.studio.backend.requests.map((r) => r.op);
    // The tree, and the session's history, were read through the relay.
    await waitFor(() => expect(ops()).toContain("workspace.tree"), WAIT);
    fireEvent.click(await screen.findByText("Fix the build"));
    await waitFor(() => expect(ops()).toContain("session.history"), WAIT);
    fireEvent.click(screen.getByText("Git"));
    await waitFor(() => expect(ops()).toEqual(expect.arrayContaining(["git.status", "git.log"])), WAIT);
    // Live events arrive once.
    env.studio.backend.feedEvent("workspace.changed", { workspace: ws("w-live") });
    expect(await sidebar().findAllByText("repo-w-live", {}, WAIT)).toHaveLength(1);
    // This computer's host saw none of it.
    expect(env.local.calls.map((c) => c.op)).not.toContain("git.status");
    // Nothing the host wrote was readable on the relay.
    const relayed = new TextDecoder("latin1").decode(Uint8Array.from(env.cloud.relayed.flatMap((b) => [...b])));
    expect(relayed).not.toContain("repo-w-remote");
  });

  it("switches hosts without restarting or mixing their state", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    const list = await openPicker();
    fireEvent.click(list.getByText("This computer"));
    await screen.findByText("repo-w-local", {}, WAIT);
    expect(screen.queryByText("repo-w-remote")).toBeNull();
    // The remote tunnel was closed with the old connection.
    await waitFor(() => expect(env.studio.live).toHaveLength(0), WAIT);
    const back = await openPicker();
    fireEvent.click(await back.findByText("Studio"));
    await screen.findByText("repo-w-remote", {}, WAIT);
    expect(screen.queryByText("repo-w-local")).toBeNull();
  });

  it("reconnects after a relay drop, and refreshes after a host restart", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    env.cloud.cut(1006);
    await waitFor(() => expect(badge()).not.toBe(RELAY), WAIT);
    await waitFor(() => expect(badge()).toBe(RELAY), WAIT);
    expect(sidebar().getAllByText("repo-w-remote")).toHaveLength(1);

    // A restart: a new epoch, and the state is read again.
    env.studio.backend.handlers["workspace.list"] = () => [ws("w-remote"), ws("w-after")];
    env.studio.restart("e2");
    await screen.findByText("repo-w-after", {}, { timeout: 6000 });
    expect(sidebar().getAllByText("repo-w-remote")).toHaveLength(1);
  }, 10_000);

  it("shows an offline host as such and reconnects when it is back", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    env.cloud.setOnline("host-studio", false);
    await waitFor(() => expect(badge()).toBe("Host offline"), WAIT);
    expect(screen.getByTestId("offline-banner")).toBeTruthy();
    env.cloud.setOnline("host-studio", true);
    await waitFor(() => expect(badge()).toBe(RELAY), { timeout: 8000 });
  }, 12_000);

  it("keeps this computer's host working while the Cloud is down", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    env.cloud.down = true;
    env.cloud.cut(1006);
    await waitFor(() => expect(badge()).toBe("Cloud unavailable"), WAIT);
    const list = await openPicker();
    fireEvent.click(list.getByText("This computer"));
    await screen.findByText("repo-w-local", {}, WAIT);
    await waitFor(() => expect(badge()).toBe("Connected"), WAIT);
    // The pairing survives the outage.
    expect(env.pins).toHaveLength(1);
  });

  it("asks to pair again when the host revokes this device", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    env.studio.revoke(env.device.publicKey);
    await waitFor(() => expect(badge()).toBe("Device refused"), WAIT);
    await waitFor(() => expect(env.pins).toEqual([]), WAIT);
    // It stays refused: no retry loop.
    const handshakes = env.studio.handshakes;
    await new Promise((r) => setTimeout(r, 700));
    expect(env.studio.handshakes).toBe(handshakes);

    fireEvent.click(screen.getByText("Pair again"));
    await pasteCode(JSON.stringify(env.studio.invite()));
    await waitFor(() => expect(badge()).toBe(RELAY), WAIT);
    expect(env.pins).toHaveLength(1);
  });
});

describe("local network", () => {
  it("connects directly when the host is on this network, without the Cloud", async () => {
    const env = setup({ lan: true });
    env.lan.advertise(env.studio);
    await screen.findByText("repo-w-local");
    await pairStudio(env, LAN);
    // Pairing went through the Cloud; the connection did not.
    expect(env.cloud.openRelays).toHaveLength(0);
    expect(env.lan.openLinks).toHaveLength(1);
    const relayed = env.cloud.relayed.length;
    fireEvent.click(screen.getByText("Git"));
    await waitFor(() => expect(env.studio.backend.requests.map((r) => r.op)).toContain("git.status"), WAIT);
    expect(env.cloud.relayed.length).toBe(relayed);
    expect((screen.getByLabelText("Connection mode") as HTMLSelectElement).value).toBe("auto");
  });

  it("uses the Cloud when the host is not on this network, without showing it offline", async () => {
    const env = setup({ lan: true });
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    expect(env.lan.discoveries).toBeGreaterThan(0);
    expect(env.lan.dials).toEqual([]);
  });

  it("local only never contacts the Cloud, and says when the host is not reachable", async () => {
    const env = setup({ lan: true });
    const listener = env.lan.advertise(env.studio);
    await screen.findByText("repo-w-local");
    await pairStudio(env, LAN);
    const http = env.cloud.http.length;
    const relays = env.cloud.relays.length;
    chooseMode("local");
    await waitFor(() => expect(env.lan.links.length).toBe(2), WAIT);
    await waitFor(() => expect(badge()).toBe(LAN), WAIT);
    // The host leaves this network.
    listener.up = false;
    env.lan.cut();
    await waitFor(() => expect(badge()).toBe("Host not reachable locally"), { timeout: 8000 });
    expect(env.cloud.http.length).toBe(http);
    expect(env.cloud.relays.length).toBe(relays);
    // Back on the network: reconnected, still without the Cloud.
    listener.up = true;
    await waitFor(() => expect(badge()).toBe(LAN), { timeout: 8000 });
    expect(env.cloud.relays.length).toBe(relays);
  }, 20_000);

  it("cloud only never looks on the network, and the mode is kept across a restart", async () => {
    const env = setup({ lan: true });
    env.lan.advertise(env.studio);
    await screen.findByText("repo-w-local");
    await pairStudio(env, LAN);
    chooseMode("cloud");
    await waitFor(() => expect(badge()).toBe(RELAY), WAIT);
    const discoveries = env.lan.discoveries;
    cleanup();
    mount(env);
    await screen.findByText("repo-w-remote", {}, WAIT);
    await waitFor(() => expect(badge()).toBe(RELAY), WAIT);
    expect((screen.getByLabelText("Connection mode") as HTMLSelectElement).value).toBe("cloud");
    expect(env.lan.discoveries).toBe(discoveries);
    expect(env.lan.openLinks).toHaveLength(0);
  });

  it("reports another key on the network as a host identity mismatch, not falling back", async () => {
    const env = setup({ lan: true });
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    cleanup();
    // Something on this network claims to be Studio, with another key.
    env.lan.extra.push({
      instance: "host-studio",
      port: 4000,
      addresses: ["192.168.1.66"],
      txt: { v: "1", id: "host-studio", pk: env.attic.publicKey },
    });
    const relays = env.cloud.relays.length;
    mount(env);
    await waitFor(() => expect(badge()).toBe("Host identity mismatch"), WAIT);
    expect(env.lan.dials).toEqual([]);
    expect(env.cloud.relays.length).toBe(relays);
    expect(env.pins).toHaveLength(1);
  });

  it("reaches a paired host on this network while signed out", async () => {
    const env = world({ lan: true });
    env.token = null;
    env.pins = [{ host_id: "host-studio", host_static_pubkey: env.studio.publicKey, name: "Studio", paired_at: 1 }];
    env.studio.trusted.add(env.device.publicKey);
    env.lan.advertise(env.studio);
    vi.stubGlobal("WebSocket", env.cloud.WebSocket);
    mount(env);
    await screen.findByText("repo-w-local");
    const list = await openPicker();
    const row = (await list.findByText("Studio")).closest("button")!;
    expect(row.hasAttribute("disabled")).toBe(false);
    fireEvent.click(row);
    await screen.findByText("repo-w-remote", {}, WAIT);
    await waitFor(() => expect(badge()).toBe(LAN), WAIT);
    expect(env.cloud.http).toEqual([]);
    expect(env.cloud.relays).toHaveLength(0);
  });

  it("signing out keeps a direct connection", async () => {
    const env = setup({ lan: true });
    env.lan.advertise(env.studio);
    await screen.findByText("repo-w-local");
    await pairStudio(env, LAN);
    fireEvent.click(screen.getByText("Sign out"));
    await waitFor(() => expect(env.token).toBeNull(), WAIT);
    await new Promise((r) => setTimeout(r, 50));
    expect(currentHost()).toBe("Studio");
    expect(badge()).toBe(LAN);
  });

  it("an unknown device is refused on the network as through the Cloud", async () => {
    const env = world({ lan: true });
    env.pins = [{ host_id: "host-studio", host_static_pubkey: env.studio.publicKey, name: "Studio", paired_at: 1 }];
    env.lan.advertise(env.studio);
    vi.stubGlobal("WebSocket", env.cloud.WebSocket);
    window.localStorage.setItem("zwiim.desktop.host", JSON.stringify({ kind: "remote", hostId: "host-studio" }));
    mount(env);
    await waitFor(() => expect(badge()).toBe("Device refused"), WAIT);
    expect(env.cloud.relays).toHaveLength(0);
  });
});

describe("account", () => {
  it("asks to sign in again when the session lapses, then reconnects", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    // The refresh was refused: the shell has no session any more.
    env.token = null;
    env.cloud.cut(1006);
    await waitFor(() => expect(badge()).toBe("Sign-in required"), WAIT);
    fireEvent.click(within(screen.getByTestId("connection-status")).getByText("Sign in"));
    await waitFor(() => expect(badge()).toBe(RELAY), WAIT);
    expect(env.bridge.signIn).toHaveBeenCalledTimes(1);
  });

  it("signing out shows this computer's host and leaves every host running", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    const before = env.local.calls.length;
    fireEvent.click(screen.getByText("Sign out"));
    await screen.findByText("repo-w-local", {}, WAIT);
    expect(currentHost()).toBe("This computer");
    // Nothing was asked of either host but reads; the pairing is kept.
    const writes = ["host.claim", "session.cancel", "terminal.kill", "workspace.remove_worktree"];
    expect(env.studio.backend.requests.map((r) => r.op).filter((op) => writes.includes(op))).toEqual([]);
    const since = env.local.calls.slice(before).map((c) => c.op);
    expect(since.filter((op) => writes.includes(op))).toEqual([]);
    expect(env.pins).toHaveLength(1);
  });

  it("never claims a remote host", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    await pairStudio(env);
    await new Promise((r) => setTimeout(r, 50));
    expect(env.studio.backend.requests.map((r) => r.op)).not.toContain("host.claim");
  });
});

describe("devices", () => {
  function withDevices(env: World) {
    let devices: DeviceInfo[] = [
      {
        device_id: "phone-1",
        device_name: "Phone",
        status: "trusted",
        device_pubkey: "k",
        paired_at: "2026-09-26T10:00:00Z",
        revoked_at: null,
        last_seen_at: null,
      },
    ];
    env.local.handlers["device.list"] = () => ({ devices });
    env.local.handlers["device.revoke"] = (p) => {
      devices = devices.map((d) =>
        d.device_id === p.device_id ? { ...d, status: "revoked", revoked_at: "2026-09-26T11:00:00Z" } : d,
      );
      return {};
    };
    env.local.handlers["device.pair_invite"] = () => env.studio.invite();
    env.local.handlers["cloud.status"] = () => ({
      enrolled: false,
      cloud_url: null,
      user_id: null,
      enrolled_at: null,
      connection: "not_enrolled",
      connected_since: null,
      last_error: null,
    });
    env.local.handlers["cloud.enroll"] = (p) => ({
      enrolled: true,
      cloud_url: p.cloud_url,
      user_id: "u1",
      enrolled_at: "now",
      connection: "connecting",
      connected_since: null,
      last_error: null,
    });
  }

  it("lists and revokes this host's devices, and shows a pairing QR with nothing reusable in it", async () => {
    const env = world();
    withDevices(env);
    vi.stubGlobal("WebSocket", env.cloud.WebSocket);
    mount(env);
    await screen.findByText("repo-w-local");
    fireEvent.click(screen.getByText("Devices"));
    const dialog = within(await screen.findByRole("dialog", { name: "Devices" }));
    expect(await dialog.findByText("Phone")).toBeTruthy();
    fireEvent.click(dialog.getByText("Revoke"));
    expect(await dialog.findByText(/^Revoked /)).toBeTruthy();
    expect(env.local.calls.filter((c) => c.op === "device.revoke").map((c) => c.params)).toEqual([
      { device_id: "phone-1" },
    ]);

    fireEvent.click(dialog.getByText("Pair a device"));
    expect(await dialog.findByRole("img", { name: "Pairing QR code" })).toBeTruthy();
    const payload = JSON.parse(dialog.getByTestId("pairing-text").textContent!);
    expect(Object.keys(payload).sort()).toEqual(["code", "expires_at", "host_id", "host_static_pubkey", "v"]);
    expect(dialog.getByTestId("pairing-text").textContent).not.toMatch(/https?:|\d+\.\d+\.\d+\.\d+|token/);
  });

  it("enables remote access with a one-time enrollment token from the account", async () => {
    const env = world();
    withDevices(env);
    vi.stubGlobal("WebSocket", env.cloud.WebSocket);
    mount(env);
    await screen.findByText("repo-w-local");
    fireEvent.click(screen.getByText("Devices"));
    const dialog = within(await screen.findByRole("dialog", { name: "Devices" }));
    fireEvent.click(await dialog.findByText("Enable"));
    expect(await dialog.findByText("Connecting")).toBeTruthy();
    expect(env.local.calls.filter((c) => c.op === "cloud.enroll").map((c) => c.params)).toEqual([
      { cloud_url: env.cloud.origin, enrollment_token: "enroll-1" },
    ]);
  });

  it("moves remote access to the Cloud this app uses when the host's differs", async () => {
    const env = world();
    withDevices(env);
    env.local.handlers["cloud.status"] = () => ({
      enrolled: true,
      cloud_url: "https://old-tunnel.example",
      user_id: "u1",
      enrolled_at: "then",
      connection: "offline",
      connected_since: null,
      last_error: "the Cloud is unreachable",
    });
    vi.stubGlobal("WebSocket", env.cloud.WebSocket);
    mount(env);
    await screen.findByText("repo-w-local");
    fireEvent.click(screen.getByText("Devices"));
    const dialog = within(await screen.findByRole("dialog", { name: "Devices" }));
    expect(await dialog.findByText(/Enabled on https:\/\/old-tunnel\.example/)).toBeTruthy();
    fireEvent.click(dialog.getByText("Move to this Cloud"));
    expect(await dialog.findByText("Connecting")).toBeTruthy();
    expect(dialog.queryByText("Move to this Cloud")).toBeNull();
    expect(env.local.calls.filter((c) => c.op === "cloud.enroll").map((c) => c.params)).toEqual([
      { cloud_url: env.cloud.origin, enrollment_token: "enroll-1" },
    ]);
  });

  it("turns local network access on and off on this computer's host", async () => {
    const env = world();
    withDevices(env);
    let lan = { enabled: false, listening: false, port: null as number | null, advertised: false, last_error: null };
    env.local.handlers["lan.status"] = () => lan;
    env.local.handlers["lan.set"] = (p) => {
      lan = p.enabled
        ? { enabled: true, listening: true, port: 4100, advertised: true, last_error: null }
        : { enabled: false, listening: false, port: null, advertised: false, last_error: null };
      return lan;
    };
    vi.stubGlobal("WebSocket", env.cloud.WebSocket);
    mount(env);
    await screen.findByText("repo-w-local");
    fireEvent.click(screen.getByText("Devices"));
    const dialog = within(await screen.findByRole("dialog", { name: "Devices" }));
    const section = within(await dialog.findByTestId("lan-access"));
    expect(section.getByText("Off")).toBeTruthy();
    fireEvent.click(section.getByText("Turn on"));
    expect(await section.findByText("On · port 4100")).toBeTruthy();
    fireEvent.click(section.getByText("Turn off"));
    expect(await section.findByText("Off")).toBeTruthy();
    expect(env.local.calls.filter((c) => c.op === "lan.set").map((c) => c.params)).toEqual([
      { enabled: true },
      { enabled: false },
    ]);
  });

  it("is not offered for a remote host", async () => {
    const env = setup();
    await screen.findByText("repo-w-local");
    expect(screen.getByText("Devices")).toBeTruthy();
    await pairStudio(env);
    expect(screen.queryByText("Devices")).toBeNull();
  });
});
