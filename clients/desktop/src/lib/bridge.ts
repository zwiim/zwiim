// The only native surface the UI uses: host discovery, host start, the
// folder picker and the optional account sign-in. Everything else goes
// through the SDK over HTTP/WS.

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
}

const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

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

export function defaultBridge(): NativeBridge {
  if (isTauri()) return tauriBridge();
  if (import.meta.env.DEV) return devBrowserBridge();
  throw new Error("Zwiim Desktop must run inside its native shell");
}
