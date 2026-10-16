// The Host this phone is controlling: one connection at a time, shared by the
// Hosts list, the Host bar and every tab of the Host shell. Choosing another
// Host closes the first. The connection mode and the worktree chosen in Files
// are kept per Host across launches, and the Host itself is remembered to
// open again on launch.

import {
  ZwiimClientError,
  type CloneStart,
  type ConnectionMode,
  type GithubRepoPage,
  type HostHello,
  type HostStore,
  type MergeMethod,
  type OpName,
  type OpParams,
  type OpResult,
  type OptionsRequest,
  type PrMerged,
  type PrState,
  type PromptBlock,
  type RequestAnswer,
  type SessionOptions,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import type { HostEntry } from "../account/hosts";
import type { Preferences, SecureStore } from "../platform/capabilities";
import { settleClones } from "./clone";
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
  /** The worktree Files shows (a root or one of its worktrees), once chosen; kept per host. */
  workspaceId: string | null;
  /** Failed clones whose card was dismissed on this phone. */
  dismissedClones: ReadonlySet<string>;
}

export interface CurrentHostDeps {
  store: SecureStore;
  prefs: Preferences;
  /** A connection to a paired host in `mode`, not yet connected. */
  open(hostId: string, mode: ConnectionMode): Promise<HostConnection>;
}

const EMPTY: CurrentHostState = {
  host: null,
  view: null,
  hello: null,
  openError: null,
  store: null,
  workspaceId: null,
  dismissedClones: new Set(),
};

export class CurrentHost {
  readonly #deps: CurrentHostDeps;
  #state: CurrentHostState = EMPTY;
  #connection: HostConnection | null = null;
  /** Bumped on every open, so a late result of an earlier one is dropped. */
  #run = 0;
  /** `host.hello` was asked on this live connection. */
  #asked = false;
  readonly #listeners = new Set<() => void>();
  /** The session last on screen in each worktree of this host (in memory, as Desktop keeps it). */
  readonly #lastSession = new Map<string, string>();
  /** What's typed and not yet sent in each session of this host (in memory; never sent to the host). */
  readonly #drafts = new Map<string, string>();
  /** Clones started here with `choose`: the first one done becomes the selection. */
  readonly #chosen = new Set<string>();
  #unwatch: (() => void) | null = null;

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
    const sameHost = this.#state.host?.host_id === host.host_id;
    if (!sameHost) {
      this.#lastSession.clear();
      this.#drafts.clear();
      this.#chosen.clear();
    }
    this.#drop();
    const run = ++this.#run;
    this.#set({ ...EMPTY, host, dismissedClones: sameHost ? this.#state.dismissedClones : EMPTY.dismissedClones });
    await this.#deps.prefs.setLastHost({ host_id: host.host_id, name: host.name });
    const [modes, chosen] = await Promise.all([this.#deps.prefs.connectionModes(), this.#deps.prefs.workspaces()]);
    const mode = modes[host.host_id] ?? "auto";
    if (run === this.#run && this.#state.workspaceId === null) this.#set({ workspaceId: chosen[host.host_id] ?? null });
    let connection: HostConnection;
    try {
      connection = await this.#deps.open(host.host_id, mode);
    } catch (e) {
      if (run === this.#run) this.#set({ openError: errorInfo(e) });
      return;
    }
    if (run !== this.#run) return connection.close();
    this.#connection = connection;
    this.#unwatch = connection.store.subscribe(() => this.#settleClones());
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
   * Resolves to the new session a plan was handed off to, else null.
   */
  async respond(session_id: string, request_id: string, answer: RequestAnswer): Promise<string | null> {
    const connection = this.#connection;
    if (!connection) throw new ZwiimClientError("network", "Not connected to the host");
    const result = await connection.request("session.respond", { session_id, request_id, ...answer });
    return result.session_id ?? null;
  }

  /** A session is on screen: stream it and load its transcript until hidden. */
  showSession(session_id: string): void {
    this.#connection?.showSession(session_id);
  }

  hideSession(session_id: string): void {
    this.#connection?.hideSession(session_id);
  }

  /** `session_id` is the one on screen in `workspace_id`: switching back to that worktree opens it. */
  rememberSession(workspace_id: string, session_id: string): void {
    this.#lastSession.set(workspace_id, session_id);
  }

  /** The session last on screen in a worktree, if any. */
  lastSession(workspace_id: string): string | null {
    return this.#lastSession.get(workspace_id) ?? null;
  }

  /** What's typed in a session's composer and not yet sent; "" when nothing is. */
  draft(session_id: string): string {
    return this.#drafts.get(session_id) ?? "";
  }

  /** Keep what's typed in one session's composer, apart from every other session's. */
  setDraft(session_id: string, text: string): void {
    if (text) this.#drafts.set(session_id, text);
    else this.#drafts.delete(session_id);
  }

  /**
   * Send a message to a session, with any files already uploaded. The host
   * echoes it on the session's stream. Once the host takes it, that session's
   * draft is cleared (unless more was typed since); a failed send keeps it.
   */
  async send(session_id: string, text: string, client_message_id: string, files: PromptBlock[] = []): Promise<void> {
    const connection = this.#connection;
    if (!connection) throw new ZwiimClientError("network", "Not connected to the host");
    await connection.send(session_id, text, client_message_id, files);
    if (this.draft(session_id).trim() === text) this.setDraft(session_id, "");
  }

  /**
   * Start a session where `params.workspace_id` says, never elsewhere. The
   * worktree chosen in Files stays as it is; nothing is listed until the
   * host reports the session on its feed.
   */
  async startSession(params: OpParams<"session.start">): Promise<string> {
    return this.#opened().startSession(params);
  }

  /** Read which agents the host can run again. */
  async refreshAgents(): Promise<void> {
    await this.#opened().refreshAgents();
  }

  /** Stop the session's current turn. The host ends it on the session's stream. */
  async cancel(session_id: string): Promise<void> {
    const connection = this.#connection;
    if (!connection) throw new ZwiimClientError("network", "Not connected to the host");
    await connection.request("session.cancel", { session_id });
  }

  /**
   * Stop a session's agent and its background work. The session and its
   * transcript stay; the host reports it not live, and the next send starts
   * the agent again. Never retried.
   */
  async stopSession(session_id: string): Promise<void> {
    await this.#opened().stopSession(session_id);
  }

  /**
   * Merge a workspace's pull request now, pinned to the head the phone shows:
   * the Host refuses a PR that moved since (`pr_changed`) or that GitHub would
   * not merge now (`not_mergeable`). The feed then reports it merged. Never retried.
   */
  async mergePr(workspace_id: string, pr: PrState, method?: MergeMethod): Promise<PrMerged> {
    return this.#opened().mergePr(workspace_id, pr, method);
  }

  /**
   * After pull request `number` merged, move the worktree in place to a new
   * branch from the PR's base; its sessions and terminals stay, and Files stays
   * on it. The Host refuses while an agent works there (`workspace_busy`), with
   * uncommitted changes (`workspace_dirty`), or unless GitHub says it merged
   * this branch's head. Never retried.
   */
  async continueWorkspace(workspace_id: string, number: number): Promise<WorkspaceInfo> {
    return this.#opened().continueWorkspace(workspace_id, number);
  }

  /**
   * Change a session's model, effort or mode between turns. The host applies
   * it to the agent (or keeps it for the agent's next start) and reports the
   * options now in effect, which replace the ones shown. Never retried.
   */
  async setSessionOptions(session_id: string, request: OptionsRequest): Promise<SessionOptions> {
    return this.#opened().setSessionOptions(session_id, request);
  }

  /** A read of the host (its files, its Git state), over the open connection. */
  async request<O extends OpName>(op: O, params: OpParams<O>, opts?: { timeoutMs?: number }): Promise<OpResult<O>> {
    return this.#opened().request(op, params, opts);
  }

  /** Show `workspace_id` in Files, now and on later launches. */
  chooseWorkspace(workspace_id: string): void {
    this.#set({ workspaceId: workspace_id });
    const host = this.#state.host;
    if (host) void this.#deps.prefs.setWorkspace(host.host_id, workspace_id).catch(() => {});
  }

  /**
   * Add a folder under the host's home as a workspace, and show it in Files.
   * `choose: false` (New session) adds it without touching Files.
   * `allowLarge`: the user confirmed a `large_folder` refusal.
   */
  async addWorkspaceAt(
    home_path: string[],
    { choose = true, allowLarge = false }: { choose?: boolean; allowLarge?: boolean } = {},
  ): Promise<WorkspaceInfo> {
    const workspace = await this.#opened().addWorkspaceAt(home_path, { allowLarge });
    if (choose) this.chooseWorkspace(workspace.workspace_id);
    return workspace;
  }

  /**
   * Create a worktree of a root workspace from its current branch, and show it
   * in Files. `choose: false` (New session) creates it without touching Files.
   */
  async createWorktree(root_workspace_id: string, { choose = true }: { choose?: boolean } = {}): Promise<WorkspaceInfo> {
    const workspace = await this.#opened().createWorktree(root_workspace_id);
    if (choose) this.chooseWorkspace(workspace.workspace_id);
    return workspace;
  }

  /** One page of the Host's GitHub repositories; `refresh` skips the Host's cache. */
  async githubRepos(page?: number, refresh?: boolean): Promise<GithubRepoPage> {
    return this.#opened().githubRepos(page, refresh);
  }

  /**
   * Clone a GitHub repository on the Host. With `choose` (the default), its
   * worktree becomes the selection once the clone is done. A repository
   * already added replies with its root and starts nothing.
   */
  async startClone(owner: string, name: string, { choose = true }: { choose?: boolean } = {}): Promise<CloneStart> {
    const host_id = this.#state.host?.host_id;
    const started = await this.#opened().cloneWorkspace(owner, name);
    if (choose && "clone_id" in started && this.#state.host?.host_id === host_id) {
      this.#chosen.add(started.clone_id);
      // It may already be done: the feed can beat the reply.
      this.#settleClones();
    }
    return started;
  }

  /** Stop a clone; `false` once it is past cloning (it then finishes). */
  async cancelClone(clone_id: string): Promise<boolean> {
    return this.#opened().cancelClone(clone_id);
  }

  /** Hide a failed clone's card on this phone. */
  dismissClone(clone_id: string): void {
    if (this.#state.dismissedClones.has(clone_id)) return;
    this.#set({ dismissedClones: new Set([...this.#state.dismissedClones, clone_id]) });
  }

  /**
   * Remove a worktree. The host refuses while agents or terminals run there
   * (`workspace_busy`, even with `force`) and, without `force`, while it has
   * uncommitted changes. Files moves to the worktree's workspace if it showed it.
   */
  async removeWorktree(workspace_id: string, opts: { force?: boolean } = {}): Promise<void> {
    const parent = this.#state.store?.getState().workspaces?.find((w) => w.workspace_id === workspace_id)?.parent_workspace_id;
    await this.#opened().removeWorktree(workspace_id, opts);
    if (this.#state.workspaceId === workspace_id && parent) this.chooseWorkspace(parent);
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
    this.#chosen.clear();
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
      void this.#deps.prefs.setWorkspace(host.host_id, null).catch(() => {});
    }
  }

  #opened(): HostConnection {
    if (!this.#connection) throw new ZwiimClientError("network", "Not connected to the host");
    return this.#connection;
  }

  /** Select the worktree of a chosen clone once it is done. Reads the connection's store: it can change before `#set`. */
  #settleClones(): void {
    if (this.#chosen.size === 0 || !this.#connection) return;
    const { done, left } = settleClones(this.#connection.store.getState().clones, [...this.#chosen]);
    this.#chosen.clear();
    for (const id of left) this.#chosen.add(id);
    for (const job of done) {
      const id = job.worktree_id ?? job.workspace_id;
      if (id) this.chooseWorkspace(id);
    }
  }

  #drop(): void {
    this.#asked = false;
    this.#unwatch?.();
    this.#unwatch = null;
    this.#connection?.close();
    this.#connection = null;
  }

  #set(patch: Partial<CurrentHostState>): void {
    this.#state = { ...this.#state, ...patch };
    for (const l of [...this.#listeners]) l();
  }
}
