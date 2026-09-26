// The Host this phone is controlling: one connection at a time, shared by the
// Hosts list, the Host bar and every tab of the Host shell. Choosing another
// Host closes the first. The connection mode is kept per Host across
// launches, and the Host itself is remembered to open again on launch.

import {
  ZwiimClientError,
  type ConnectionMode,
  type HostHello,
  type HostStore,
  type OpName,
  type OpParams,
  type OpResult,
  type RequestAnswer,
} from "@zwiim/sdk";
import type { HostEntry } from "../account/hosts";
import type { Preferences, SecureStore } from "../platform/capabilities";
import { errorInfo, type ErrorInfo, type HostConnection, type HostConnectionView, type TerminalHold } from "./connection";

export interface CurrentHostState {
  host: HostEntry | null;
  /** The connection, once opened. */
  view: HostConnectionView | null;
  /** The host's `host.hello`, once live. */
  hello: HostHello | null;
  /** The connection could not be opened (this phone isn't paired with it). */
  openError: ErrorInfo | null;
  /** The host's workspaces and sessions, once the connection is open. */
  store: HostStore | null;
  /** The workspace Files and Terminal show, once chosen; kept while this host is. */
  workspaceId: string | null;
}

export interface CurrentHostDeps {
  store: SecureStore;
  prefs: Preferences;
  /** A connection to a paired host in `mode`, not yet connected. */
  open(hostId: string, mode: ConnectionMode): Promise<HostConnection>;
}

const EMPTY: CurrentHostState = { host: null, view: null, hello: null, openError: null, store: null, workspaceId: null };

export class CurrentHost {
  readonly #deps: CurrentHostDeps;
  #state: CurrentHostState = EMPTY;
  #connection: HostConnection | null = null;
  /** Bumped on every open, so a late result of an earlier one is dropped. */
  #run = 0;
  /** `host.hello` was asked on this live connection. */
  #asked = false;
  readonly #listeners = new Set<() => void>();

  constructor(deps: CurrentHostDeps) {
    this.#deps = deps;
  }

  get state(): CurrentHostState {
    return this.#state;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  /** Control `host`, keeping an existing connection to it. */
  async select(host: HostEntry): Promise<void> {
    if (this.#state.host?.host_id === host.host_id && this.#connection) {
      this.#set({ host });
      this.#connection.connect();
      return;
    }
    await this.open(host);
  }

  /** Control `host` over a new connection, read again from the store (after pairing). */
  async open(host: HostEntry): Promise<void> {
    this.#drop();
    const run = ++this.#run;
    this.#set({ ...EMPTY, host });
    await this.#deps.prefs.setLastHost({ host_id: host.host_id, name: host.name });
    const mode = (await this.#deps.prefs.connectionModes())[host.host_id] ?? "auto";
    let connection: HostConnection;
    try {
      connection = await this.#deps.open(host.host_id, mode);
    } catch (e) {
      if (run === this.#run) this.#set({ openError: errorInfo(e) });
      return;
    }
    if (run !== this.#run) return connection.close();
    this.#connection = connection;
    connection.subscribe((view) => this.#onView(run, view));
    this.#set({ view: connection.view, store: connection.store });
    connection.connect();
  }

  /** Use `mode` for the current host, now and on later launches. */
  async setMode(mode: ConnectionMode): Promise<void> {
    const host = this.#state.host;
    if (!host) return;
    await this.#deps.prefs.setConnectionMode(host.host_id, mode);
    this.#connection?.setMode(mode);
  }

  /** Try again now: reconnect, or reopen if opening failed. */
  retry(): void {
    if (this.#connection) {
      this.#connection.connect();
      this.#connection.resume();
    } else if (this.#state.host) void this.open(this.#state.host);
  }

  /**
   * Answer a session's pending request. The host then resolves it on the
   * session's stream, which updates the store; nothing is changed here first.
   */
  async respond(session_id: string, request_id: string, answer: RequestAnswer): Promise<void> {
    const connection = this.#connection;
    if (!connection) throw new ZwiimClientError("network", "Not connected to the host");
    await connection.request("session.respond", { session_id, request_id, ...answer });
  }

  /** A session is on screen: stream it and load its transcript until hidden. */
  showSession(session_id: string): void {
    this.#connection?.showSession(session_id);
  }

  hideSession(session_id: string): void {
    this.#connection?.hideSession(session_id);
  }

  /** Send a message to a session. The host echoes it on the session's stream. */
  async send(session_id: string, text: string, client_message_id: string): Promise<void> {
    const connection = this.#connection;
    if (!connection) throw new ZwiimClientError("network", "Not connected to the host");
    await connection.send(session_id, text, client_message_id);
  }

  /** Stop the session's current turn. The host ends it on the session's stream. */
  async cancel(session_id: string): Promise<void> {
    const connection = this.#connection;
    if (!connection) throw new ZwiimClientError("network", "Not connected to the host");
    await connection.request("session.cancel", { session_id });
  }

  /** A read of the host (its files, its Git state), over the open connection. */
  async request<O extends OpName>(op: O, params: OpParams<O>): Promise<OpResult<O>> {
    return this.#opened().request(op, params);
  }

  /** Show `workspace_id` in Files and Terminal. */
  chooseWorkspace(workspace_id: string): void {
    this.#set({ workspaceId: workspace_id });
  }

  /** A terminal is on screen: its emulator, streamed until released. None before a connection opens. */
  openTerminal(terminal_id: string): TerminalHold | null {
    return this.#connection?.openTerminal(terminal_id) ?? null;
  }

  /** Start a terminal in a workspace. The host lists it on its feed. */
  async spawnTerminal(workspace_id: string): Promise<string> {
    return this.#opened().spawnTerminal(workspace_id);
  }

  /** Type into a terminal. */
  async writeTerminal(terminal_id: string, data: string): Promise<void> {
    await this.#opened().writeTerminal(terminal_id, data);
  }

  /** End a terminal's process. */
  async killTerminal(terminal_id: string): Promise<void> {
    await this.#opened().killTerminal(terminal_id);
  }

  /** When this phone first saw a request still pending. */
  requestSeenAt = (request_id: string): number | null => this.#connection?.requestSeenAt(request_id) ?? null;

  /** The app is in the foreground again: skip the rest of the backoff. */
  resume(): void {
    this.#connection?.resume();
  }

  /** Stop controlling any host, and forget the last one (signing out). */
  async clear(): Promise<void> {
    this.#drop();
    this.#run++;
    this.#set(EMPTY);
    await this.#deps.prefs.setLastHost(null);
  }

  #onView(run: number, view: HostConnectionView): void {
    if (run !== this.#run) return;
    const host = this.#state.host!;
    if (view.status !== "live") {
      this.#asked = false;
      this.#set({ view, hello: null });
    } else {
      this.#set({ view });
      if (!this.#asked) {
        this.#asked = true;
        this.#connection?.request("host.hello", {}).then(
          (hello) => run === this.#run && this.#set({ hello }),
          () => {},
        );
      }
    }
    // The host no longer trusts this phone: its pairing is gone for good.
    if (view.error?.code === "device_refused" && host.paired) {
      this.#set({ host: { ...host, paired: false } });
      void this.#deps.store.forgetHost(host.host_id);
    }
  }

  #opened(): HostConnection {
    if (!this.#connection) throw new ZwiimClientError("network", "Not connected to the host");
    return this.#connection;
  }

  #drop(): void {
    this.#asked = false;
    this.#connection?.close();
    this.#connection = null;
  }

  #set(patch: Partial<CurrentHostState>): void {
    this.#state = { ...this.#state, ...patch };
    for (const l of [...this.#listeners]) l();
  }
}
