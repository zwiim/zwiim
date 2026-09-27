// What the app needs from the phone, as interfaces: the SDK and the screens
// depend on these, never on a native module directly, so each platform's
// implementation can be swapped (or faked in tests) behind them.

import type { ConnectionMode, DeviceKeys, HostLanEndpoint, LanDial, LanDiscover } from "@zwiim/sdk";

/** A host this device is paired with: its key, pinned at pairing. */
export interface PinnedHost {
  host_id: string;
  host_static_pubkey: string;
  /** Where the host last said it can be dialed on its network; pairing again forgets it. */
  lan_endpoint?: HostLanEndpoint | null;
}

/** This device's identity: its id and tunnel keypair. */
export interface DeviceIdentity {
  device_id: string;
  keys: DeviceKeys;
}

/** The signed-in account, and the token that renews its access token. */
export interface AccountSession {
  user_id: string;
  email: string | null;
  refresh_token: string;
}

/**
 * Where this device's secrets live: its private key, the host keys it
 * pinned at pairing, and the account's session. On a phone, the iOS
 * Keychain / Android Keystore. Never AsyncStorage, plain files, app state or
 * logs: callers read a secret when they need it and do not keep it.
 */
export interface SecureStore {
  deviceIdentity(): Promise<DeviceIdentity | null>;
  setDeviceIdentity(identity: DeviceIdentity): Promise<void>;
  pinnedHost(hostId: string): Promise<PinnedHost | null>;
  pinHost(host: PinnedHost): Promise<void>;
  /** Keep where a pinned host can be dialed on its network; nothing for a host not pinned. */
  setHostLanEndpoint(hostId: string, endpoint: HostLanEndpoint | null): Promise<void>;
  /** Drop a host's pinned key: the host no longer trusts this device. */
  forgetHost(hostId: string): Promise<void>;
  /** The account's current access token: the Cloud's bearer token. */
  accountToken(): Promise<string | null>;
  setAccountToken(token: string | null): Promise<void>;
  /** Who is signed in; none for a test token alone. */
  accountSession(): Promise<AccountSession | null>;
  setAccountSession(session: AccountSession | null): Promise<void>;
}

/** The host this phone opened last, to open again on launch. */
export interface LastHost {
  host_id: string;
  name: string | null;
}

/**
 * Choices kept across launches. Not secrets, but kept in the same platform
 * storage as them, so the app writes nothing anywhere else.
 */
export interface Preferences {
  /** The connection mode chosen for each host; absent means Automatic. */
  connectionModes(): Promise<Record<string, ConnectionMode>>;
  setConnectionMode(hostId: string, mode: ConnectionMode): Promise<void>;
  lastHost(): Promise<LastHost | null>;
  setLastHost(host: LastHost | null): Promise<void>;
  /** The worktree chosen in Files on each host; absent means its default. */
  workspaces(): Promise<Record<string, string>>;
  setWorkspace(hostId: string, workspace_id: string | null): Promise<void>;
}

/** Finding hosts on the local network and dialing them: the SDK's LAN inputs. */
export interface LanAccess {
  discover: LanDiscover;
  dial: LanDial;
}

/** Whether the phone has a network, as it changes. */
export interface NetworkReachability {
  subscribe(listener: (online: boolean) => void): () => void;
}
