// The only native surface the UI uses: host discovery, host start, the
// folder picker, the optional account sign-in and, for hosts on other
// computers, this device's identity and the account's Cloud requests.
// Everything else goes through the SDK over HTTP/WS (or the device tunnel,
// through the Cloud relay or directly on the local network).

import type { ByteChannel, LanDial, LanDiscover } from "@zwiim/sdk";

export interface DiscoveredHost {
  host_id: string;
  url: string;
  token: string;
  pid: number;
  /** The process that wrote the discovery file is alive. */
  running: boolean;
}

/** The signed-in account. The session itself stays in the OS keychain. */
export interface Account {
  user_id: string;
  email: string | null;
}

export interface AccountStatus {
  /** This build can sign in. */
  available: boolean;
  account: Account | null;
  /** The Zwiim Cloud that reaches remote hosts; absent without one. */
  cloud_url?: string | null;
}

/** A Cloud response. The shell adds the account's token; the UI never sees it. */
export interface CloudReply {
  status: number;
  body: unknown;
}

/** This device's identity, from the OS keychain. The secret is kept in memory only. */
export interface DeviceKey {
  device_id: string;
  device_name: string;
  /** X25519 static secret, unpadded base64url. */
  secret: string;
}

/** A host this device paired with, pinned to the key its QR carried. */
export interface PairedHost {
  host_id: string;
  host_static_pubkey: string;
  name: string | null;
  /** Unix seconds. */
  paired_at: number;
}

/**
 * Hosts on this computer's network: mDNS discovery, and a byte pipe to an
 * endpoint discovery reported. The SDK checks each advertisement against
 * the pinned key and runs the tunnel's handshake over the pipe.
 */
export interface LanBridge {
  discover: LanDiscover;
  dial: LanDial;
}

/** Remote hosts: only in the desktop app, where the keychain is. */
export interface RemoteBridge {
  /** One of the device's Cloud calls (`GET /v1/hosts`, tickets, enrollments). */
  cloudRequest(method: "GET" | "POST", path: string): Promise<CloudReply>;
  deviceIdentity(): Promise<DeviceKey>;
  pairedHosts(): Promise<PairedHost[]>;
  pinHost(host: PairedHost): Promise<void>;
  forgetHost(hostId: string): Promise<void>;
  /** Absent where the local network cannot be reached directly. */
  lan?: LanBridge;
}

/** This app's own update (`tauri-plugin-updater`, driven from the shell). */
export type DesktopUpdateActivity =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "up_to_date" }
  | { state: "available"; version: string; notes: string | null }
  | { state: "downloading"; version: string; downloaded: number; total: number | null }
  | { state: "ready"; version: string }
  | { state: "failed"; message: string };

export interface DesktopUpdateStatus {
  version: string;
  /** False in a build made outside the release pipeline: it never updates. */
  supported: boolean;
  unsupported_reason: "unsigned_build" | null;
  /** Which releases this app updates to. Beta takes stable releases too. */
  channel: "stable" | "beta";
  activity: DesktopUpdateActivity;
}

export interface DesktopUpdates {
  status(): Promise<DesktopUpdateStatus>;
  /** Look for a newer Desktop; never downloads. */
  check(): Promise<DesktopUpdateStatus>;
  /** Download, verify and install; progress shows in `status`. */
  install(): Promise<DesktopUpdateStatus>;
  /** Relaunch into the installed update. The host keeps running. */
  restart(): Promise<void>;
  /** Follow `channel` from the next check, and remember it. */
  setChannel(channel: DesktopUpdateStatus["channel"]): Promise<DesktopUpdateStatus>;
}

export interface NativeBridge {
  discover(): Promise<DiscoveredHost | null>;
  startHost(): Promise<void>;
  /** Absolute directory chosen by the user, or null when cancelled. */
  pickFolder(): Promise<string | null>;
  /** The kept account, read locally: works offline. */
  accountStatus(): Promise<AccountStatus>;
  /** Sign in with Google in the system browser. */
  signIn(): Promise<Account>;
  cancelSignIn(): Promise<void>;
  /** Forget the account here. Never touches the host. */
  signOut(): Promise<void>;
  remote?: RemoteBridge;
  /** Only in the desktop app. */
  updates?: DesktopUpdates;
  /** Only in the desktop app: whether the host starts when the user logs in. */
  hostLogin?: HostLogin;
  /** Only in the desktop app on macOS: keeping a closed laptop awake. */
  lidHelper?: LidHelper;
}

/** Where registering the power helper left it. */
export type LidHelperSetup = "enabled" | "requires_approval" | "not_registered" | "not_found" | "unknown";

export interface LidHelper {
  /** Register the power helper; may open System Settings for approval. */
  setup(): Promise<LidHelperSetup>;
}

export interface HostLoginStatus {
  /** False where there is no installed host to start (development builds). */
  supported: boolean;
  enabled: boolean;
}

export interface HostLogin {
  status(): Promise<HostLoginStatus>;
  /** Affects the next login only; the running host is left alone. */
  set(enabled: boolean): Promise<HostLoginStatus>;
}

/** Bytes as standard base64, and back: how the shell's pipe carries them. */
function toBase64(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

function fromBase64(text: string): Uint8Array {
  const bin = atob(text);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

type LinkEvent = { kind: "data"; data: string } | { kind: "closed" };

/** The shell's mDNS discovery and TCP pipe (`lan_*` commands). */
export function tauriLan(): LanBridge {
  return {
    async discover(hostId, { timeoutMs }) {
      const { invoke } = await import("@tauri-apps/api/core");
      return invoke<unknown[]>("lan_discover", { hostId, timeoutMs });
    },
    async dial({ address, port }, { timeoutMs }) {
      const { Channel, invoke } = await import("@tauri-apps/api/core");
      type Handlers = Parameters<ByteChannel["listen"]>[0];
      let handlers: Handlers | null = null;
      // What arrived before the tunnel listened.
      const early: LinkEvent[] = [];
      let ended = false;
      const deliver = (ev: LinkEvent) => {
        if (ended) return;
        if (!handlers) return void early.push(ev);
        if (ev.kind === "data") return handlers.onData(fromBase64(ev.data));
        ended = true;
        handlers.onClose({ code: 1006, reason: "" });
      };
      const onEvent = new Channel<LinkEvent>();
      onEvent.onmessage = deliver;
      const id = await invoke<number>("lan_connect", { address, port, timeoutMs, onEvent });
      // One send at a time, in order: the tunnel's frames must not reorder.
      let sending: Promise<unknown> = Promise.resolve();
      return {
        send(bytes) {
          if (ended) return;
          const data = toBase64(bytes);
          sending = sending.then(() => invoke("lan_send", { id, data })).catch(() => {});
        },
        close() {
          sending = sending.then(() => invoke("lan_close", { id })).catch(() => {});
        },
        listen(h) {
          handlers = h;
          for (const ev of early.splice(0)) deliver(ev);
        },
      };
    },
  };
}

export const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function tauriBridge(): NativeBridge {
  return {
    async discover() {
      const { invoke } = await import("@tauri-apps/api/core");
      return invoke<DiscoveredHost | null>("discover_host");
    },
    async startHost() {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("start_host");
    },
    async pickFolder() {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const picked = await open({ directory: true, multiple: false });
      return typeof picked === "string" ? picked : null;
    },
    async accountStatus() {
      const { invoke } = await import("@tauri-apps/api/core");
      return invoke<AccountStatus>("account_status");
    },
    async signIn() {
      const { invoke } = await import("@tauri-apps/api/core");
      return invoke<Account>("account_sign_in");
    },
    async cancelSignIn() {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("account_cancel_sign_in");
    },
    async signOut() {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("account_sign_out");
    },
    updates: {
      async status() {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<DesktopUpdateStatus>("desktop_update_status");
      },
      async check() {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<DesktopUpdateStatus>("desktop_update_check");
      },
      async install() {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<DesktopUpdateStatus>("desktop_update_install");
      },
      async restart() {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("desktop_update_restart");
      },
      async setChannel(channel) {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<DesktopUpdateStatus>("desktop_update_set_channel", { channel });
      },
    },
    hostLogin: {
      async status() {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<HostLoginStatus>("host_login_status");
      },
      async set(enabled) {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<HostLoginStatus>("host_login_set", { enabled });
      },
    },
    lidHelper: {
      async setup() {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<LidHelperSetup>("lid_helper_setup");
      },
    },
    remote: {
      async cloudRequest(method, path) {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<CloudReply>("cloud_request", { method, path });
      },
      async deviceIdentity() {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<DeviceKey>("device_identity");
      },
      async pairedHosts() {
        const { invoke } = await import("@tauri-apps/api/core");
        return invoke<PairedHost[]>("device_paired_hosts");
      },
      async pinHost(host) {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("device_pin_host", { host });
      },
      async forgetHost(hostId) {
        const { invoke } = await import("@tauri-apps/api/core");
        await invoke("device_forget_host", { hostId });
      },
      lan: tauriLan(),
    },
  };
}

/**
 * Development-only bridge for running the UI in a browser against a host
 * started by hand: `http://localhost:1420/?url=http://127.0.0.1:3080&token=…`.
 * With `&dev_account=1`, sign-in asks for "user_id email" instead of Google.
 */
function devBrowserBridge(): NativeBridge {
  const params = new URLSearchParams(window.location.search);
  const url = params.get("url");
  const token = params.get("token");
  const DEV_ACCOUNT = "zwiim.dev.account";
  const devAccount = (): Account | null => {
    const raw = window.localStorage.getItem(DEV_ACCOUNT);
    return raw ? (JSON.parse(raw) as Account) : null;
  };
  return {
    async discover() {
      if (!url || !token) return null;
      return { host_id: params.get("host_id") ?? "", url, token, pid: 0, running: true };
    },
    async startHost() {
      throw new Error("Start the host manually when running outside the desktop app");
    },
    async pickFolder() {
      return window.prompt("Absolute folder path on the host") || null;
    },
    async accountStatus() {
      return { available: params.get("dev_account") === "1", account: devAccount() };
    },
    async signIn() {
      const [user_id, email] = (window.prompt("Dev sign-in: user_id email") ?? "").trim().split(/\s+/);
      if (!user_id) throw new Error("Sign-in cancelled");
      const account = { user_id, email: email ?? null };
      window.localStorage.setItem(DEV_ACCOUNT, JSON.stringify(account));
      return account;
    },
    async cancelSignIn() {},
    async signOut() {
      window.localStorage.removeItem(DEV_ACCOUNT);
    },
  };
}

/** Web and mail links; anything else a conversation links to stays shut. */
export const isOpenableLink = (href: string | undefined): href is string =>
  !!href && /^(https?:\/\/|mailto:)/i.test(href) && !/[\s\u0000-\u001f]/.test(href);

/**
 * Open a link from a conversation in the default browser. The app's own
 * window never navigates away, and a native window never opens a new one.
 */
export async function openLink(href: string): Promise<void> {
  if (!isOpenableLink(href)) return;
  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("open_link", { url: href });
  } else {
    window.open(href, "_blank", "noopener,noreferrer");
  }
}

/**
 * Open a workspace file in the default app for its type, written piece by
 * piece so a large file never sits whole in memory. Outside the native app
 * the browser downloads it instead.
 */
export async function openFileBytes(name: string, mime_type: string, chunks: AsyncIterable<Uint8Array>): Promise<void> {
  if (isTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    const token = await invoke<string>("download_begin", { name });
    for await (const bytes of chunks) await invoke("download_append", bytes, { headers: { "x-download": token } });
    await invoke("download_open", { token });
    return;
  }
  const parts: Uint8Array[] = [];
  for await (const bytes of chunks) parts.push(bytes);
  const url = URL.createObjectURL(new Blob(parts as BlobPart[], { type: mime_type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

export function defaultBridge(): NativeBridge {
  if (isTauri()) return tauriBridge();
  if (import.meta.env.DEV) return devBrowserBridge();
  throw new Error("Zwiim Desktop must run inside its native shell");
}
