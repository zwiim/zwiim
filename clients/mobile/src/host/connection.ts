// The app's connection to one paired host: the SDK's `HostClient` over its
// `ConnectionManager`, which alone chooses between the direct LAN path and
// the Cloud relay by the mode. Screens read a `HostConnectionView` and call
// these methods; they never pick a transport. The host's state (workspaces,
// sessions, their live views) is the SDK's `HostStore`, kept by its
// `HostController`; it outlives a change of mode and is rebuilt from the
// host on every connect.

import {
  activeConnection,
  ConnectionManager,
  createHostStore,
  HostClient,
  HostController,
  isClientError,
  ZwiimClientError,
  type ActiveConnection,
  type BackoffOptions,
  type CloudClient,
  type ConnectionMode,
  type ConnectionStatus,
  type HostLanEndpoint,
  type HostStore,
  type OpName,
  type OpParams,
  type OpResult,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import type { LanAccess, NetworkReachability } from "../platform/capabilities";
import { SessionFollower } from "../sessions/follow";
import { TerminalScreen } from "../terminal/screen";

export interface HostConnectionConfig {
  hostId: string;
  /** Pinned at pairing. */
  hostStaticPubkey: string;
  /** This device's X25519 secret, read from the secure store for this connection. */
  deviceSecret: Uint8Array;
  /** The relay path; none without an account. */
  cloud: CloudClient | null;
  /** The direct path; none where the phone cannot reach the LAN. */
  lan: LanAccess | null;
  /**
   * Where the host last said it can be dialed on its network, and where to
   * keep what it says next: asked on every connect. Absent: the direct
   * path's endpoint comes from `lan` alone.
   */
  lanEndpoint?: LanEndpointRecord;
  mode?: ConnectionMode;
  reachability?: NetworkReachability | null;
  /** Tests: faster timings. */
  tuning?: { backoff?: BackoffOptions; timeoutMs?: number; lanRetry?: BackoffOptions };
}

/** The host's LAN endpoint as this phone last heard it (`ReportedEndpoint`). */
export interface LanEndpointRecord {
  readonly known: HostLanEndpoint | null;
  report(endpoint: HostLanEndpoint | null): Promise<void>;
}

/** A failure, as the app shows it. */
export interface ErrorInfo {
  code: string;
  message: string;
}

/** What a screen shows about the connection. */
export interface HostConnectionView {
  mode: ConnectionMode;
  status: ConnectionStatus;
  /** `lan` / `relay` while live. */
  active: ActiveConnection;
  hostId: string | null;
  error: ErrorInfo | null;
  /** Host feed updates received (snapshots and events). */
  events: number;
  /** While reconnecting: when the next attempt starts (ms since the epoch). */
  retryAt: number | null;
  /** Whether this phone knows where to dial the host directly. */
  lanKnown: boolean;
}

/** A view's hold on a terminal's emulator; let go when the view goes. */
export interface TerminalHold {
  screen: TerminalScreen;
  release(): void;
}

/** Any failure as a code and message; codes are the SDK's. */
export function errorInfo(e: unknown): ErrorInfo {
  if (isClientError(e)) return { code: e.code, message: e.message };
  return { code: "internal", message: e instanceof Error ? e.message : String(e) };
}

export class HostConnection {
  readonly #config: HostConnectionConfig;
  #mode: ConnectionMode;
  #manager: ConnectionManager | null = null;
  #client: HostClient | null = null;
  #controller: HostController | null = null;
  #follower: SessionFollower | null = null;
  readonly #store: HostStore = createHostStore();
  readonly #requestsSeen = new Map<string, number>();
  readonly #shown = new Set<string>();
  /** Terminals on screen: each one's emulator and how many views hold it. */
  readonly #terminals = new Map<string, { screen: TerminalScreen; holds: number }>();
  /** Per terminal, the last input sent: input goes to the host in order. */
  readonly #input = new Map<string, Promise<unknown>>();
  #unsubscribe: (() => void)[] = [];
  #events = 0;
  #retryAt: number | null = null;
  #view: HostConnectionView;
  readonly #listeners = new Set<(view: HostConnectionView) => void>();
  readonly #offReachability: (() => void) | null;
  #closed = false;

  constructor(config: HostConnectionConfig) {
    this.#config = config;
    this.#mode = config.mode ?? "auto";
    this.#view = this.#compute();
    // Back online: skip the rest of the reconnect backoff.
    this.#offReachability = config.reachability?.subscribe((online) => online && this.resume()) ?? null;
  }

  get view(): HostConnectionView {
    return this.#view;
  }

  /** The host's state as this connection last heard it. */
  get store(): HostStore {
    return this.#store;
  }

  /** When this phone first saw a request that is still pending. */
  requestSeenAt(request_id: string): number | null {
    return this.#requestsSeen.get(request_id) ?? null;
  }

  subscribe(listener: (view: HostConnectionView) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  /** Use `mode` from now on; an open connection is reopened by it. */
  setMode(mode: ConnectionMode): void {
    if (mode === this.#mode || this.#closed) return;
    const wasOn = !!this.#client && this.#client.state.status !== "disconnected";
    this.#teardown();
    this.#mode = mode;
    this.#update();
    if (wasOn) this.connect();
  }

  /** Connect (or retry after a terminal failure). No-op while connecting or live. */
  connect(): void {
    if (this.#closed) return;
    this.#ensure().start();
  }

  /** Close the connection. The host and its sessions are unaffected. */
  disconnect(): void {
    this.#client?.stop();
  }

  /** The app came back to the foreground, or the network came back. */
  resume(): void {
    this.#client?.retryNow();
  }

  /** A session is on screen: stream it and load its transcript, on this and later connections. */
  showSession(session_id: string): void {
    if (this.#follower) this.#follower.show(session_id);
    else this.#shown.add(session_id);
  }

  hideSession(session_id: string): void {
    if (this.#follower) this.#follower.hide(session_id);
    else this.#shown.delete(session_id);
  }

  /**
   * A terminal is on screen: stream it into its emulator, on this and later
   * connections, until every view holding it has let go.
   */
  openTerminal(terminal_id: string): TerminalHold {
    let held = this.#terminals.get(terminal_id);
    if (held) held.holds++;
    else {
      held = { screen: new TerminalScreen(), holds: 1 };
      this.#terminals.set(terminal_id, held);
      this.#controller?.attachTerminal(terminal_id);
    }
    let released = false;
    return {
      screen: held.screen,
      release: () => {
        if (released) return;
        released = true;
        this.#release(terminal_id);
      },
    };
  }

  /** Add the folder named by its folder names under the host's home (`workspace.add`); never retried. */
  async addWorkspaceAt(home_path: string[]): Promise<WorkspaceInfo> {
    return this.#live().addWorkspaceAt(home_path);
  }

  /** Create a worktree of a root workspace from its current branch; never retried. */
  async createWorktree(root_workspace_id: string): Promise<WorkspaceInfo> {
    return this.#live().createWorktree(root_workspace_id);
  }

  /** Start a terminal in a workspace (`terminal.spawn`); never retried. */
  async spawnTerminal(workspace_id: string): Promise<string> {
    return this.#live().spawnTerminal(workspace_id);
  }

  /**
   * Type into a terminal. Input is sent in order, one write after another.
   * Once a write fails, the input queued behind it fails too, unsent: it
   * may not have reached the shell, and later keys must not run without it.
   */
  async writeTerminal(terminal_id: string, data: string): Promise<void> {
    const controller = this.#live();
    const prev = this.#input.get(terminal_id) ?? Promise.resolve();
    const next = prev.then(() => controller.client.request("terminal.write", { terminal_id, data })).then(() => {});
    this.#input.set(terminal_id, next);
    // Drained: new input starts a fresh queue.
    const drained = () => {
      if (this.#input.get(terminal_id) === next) this.#input.delete(terminal_id);
    };
    next.then(drained, drained);
    return next;
  }

  /** End a terminal's process; the host then reports it exited. */
  async killTerminal(terminal_id: string): Promise<void> {
    await this.#live().client.request("terminal.kill", { terminal_id });
  }

  /**
   * Start a session (`session.start`); never retried. The controller opens
   * it at once, so its first prompt and the agent's reply stream in.
   */
  async startSession(params: OpParams<"session.start">): Promise<string> {
    return this.#live().startSession(params);
  }

  /** Read the host's agents again (`agent.list`) into the store. */
  async refreshAgents(): Promise<void> {
    await this.#live().refreshAgents();
  }

  /** Send a prompt to a session (`session.send`); never retried. */
  async send(session_id: string, text: string, client_message_id: string): Promise<void> {
    await this.#live().sendMessage({ session_id, blocks: [{ type: "text", text }], client_message_id });
  }

  /** A request to the host, over the open connection only. */
  async request<O extends OpName>(op: O, params: OpParams<O>): Promise<OpResult<O>> {
    const client = this.#client;
    if (!client || client.state.status !== "live") {
      throw new ZwiimClientError("network", "Not connected to the host");
    }
    return client.request(op, params);
  }

  /** Disconnect and stop reporting. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#teardown();
    this.#offReachability?.();
    this.#listeners.clear();
    for (const { screen } of this.#terminals.values()) screen.dispose();
    this.#terminals.clear();
  }

  #release(terminal_id: string): void {
    const held = this.#terminals.get(terminal_id);
    if (!held || --held.holds > 0) return;
    this.#terminals.delete(terminal_id);
    this.#input.delete(terminal_id);
    this.#controller?.detachTerminal(terminal_id);
    held.screen.dispose();
  }

  #live(): HostController {
    const controller = this.#controller;
    if (!controller || controller.client.state.status !== "live") {
      throw new ZwiimClientError("network", "Not connected to the host");
    }
    return controller;
  }

  #ensure(): HostClient {
    if (this.#client) return this.#client;
    const { tuning } = this.#config;
    const manager = new ConnectionManager({
      mode: this.#mode,
      hostId: this.#config.hostId,
      hostStaticPubkey: this.#config.hostStaticPubkey,
      deviceSecret: this.#config.deviceSecret,
      lan: this.#config.lan,
      cloud: this.#config.cloud,
      timeoutMs: tuning?.timeoutMs,
      lanRetry: tuning?.lanRetry,
    });
    const client = new HostClient({ transport: manager, expectedHostId: this.#config.hostId, backoff: tuning?.backoff });
    this.#unsubscribe = [
      client.subscribe({
        connection: (state) => {
          this.#retryAt = state.retryInMs == null ? null : Date.now() + state.retryInMs;
          const connected = state.status === "live" && this.#view.status !== "live";
          this.#update();
          if (connected) this.#learnLanEndpoint(client);
        },
        feed: () => {
          this.#events++;
          this.#update();
        },
      }),
      manager.onPath(() => this.#update()),
    ];
    const controller = new HostController(client, this.#store);
    this.#unsubscribe.push(controller.onTerminalOutput((id, out) => this.#terminals.get(id)?.screen.apply(out)));
    for (const id of this.#terminals.keys()) controller.attachTerminal(id);
    this.#manager = manager;
    this.#client = client;
    this.#controller = controller;
    this.#follower = new SessionFollower(controller, this.#store, this.#requestsSeen, this.#shown);
    return client;
  }

  /**
   * Ask the host where to dial it directly next time. Best effort: a host
   * without the operation, or a dropped connection, leaves what is known.
   */
  #learnLanEndpoint(client: HostClient): void {
    const record = this.#config.lanEndpoint;
    if (!record) return;
    client
      .request("host.lan_endpoint", {})
      .then(async ({ endpoint }) => {
        if (this.#client !== client) return;
        await record.report(endpoint ?? null);
        this.#update();
      })
      .catch(() => {});
  }

  #teardown(): void {
    for (const off of this.#unsubscribe.splice(0)) off();
    this.#follower?.dispose();
    this.#controller?.dispose();
    this.#follower = null;
    this.#controller = null;
    this.#client?.stop();
    this.#manager?.close();
    this.#client = null;
    this.#manager = null;
    this.#retryAt = null;
  }

  #compute(): HostConnectionView {
    const state = this.#client?.state;
    const status = state?.status ?? "disconnected";
    return {
      mode: this.#mode,
      status,
      active: activeConnection(status, this.#manager?.path ?? null),
      hostId: state?.hostId ?? null,
      error: state?.error ?? null,
      events: this.#events,
      retryAt: this.#client ? this.#retryAt : null,
      lanKnown: !!this.#config.lan && (this.#config.lanEndpoint ? this.#config.lanEndpoint.known !== null : true),
    };
  }

  #update(): void {
    this.#view = this.#compute();
    for (const l of [...this.#listeners]) l(this.#view);
  }
}
