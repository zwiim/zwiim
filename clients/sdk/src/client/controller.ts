// HostController: connects a HostClient to a HostStore. Every piece of
// state it writes comes from a host response or frame. Workspaces, sessions
// and terminals follow the host state feed: each (re)connect starts from its
// snapshot instead of trusting what the store had.

import { createHostStore, type HostStore } from "../state/store";
import type { TerminalOutput } from "../state/terminal";
import { historyCovers, type SessionFrame } from "../state/session";
import type {
  AgentSummary,
  AgentType,
  CloneStart,
  GithubRepoPage,
  HostOwner,
  MergeMethod,
  OpParams,
  OptionsRequest,
  PrEntry,
  PrMerged,
  PrState,
  PublishTarget,
  SessionOptions,
  SessionRevertResult,
  WorkspaceDiff,
  WorkspaceInfo,
  WorkspaceInspection,
  WorktreeRemoved,
} from "../types/protocol";
import type { HostClient } from "./host-client";

/** Waits before re-reading a transcript that does not yet hold a finished turn. */
const HISTORY_SETTLE_DELAYS_MS = [250, 500, 1000, 2000];

/** `workspace.add` with `publish`: creating the GitHub repo and pushing can run long. */
const PUBLISH_TIMEOUT_MS = 330_000;

/** The Host's `gh` call has its own 15 s limit; leave room for it. */
const GITHUB_REPOS_TIMEOUT_MS = 20_000;

/** Frames after which `session.history` may include more turns. */
function transcriptMayHaveChanged(frame: SessionFrame): boolean {
  switch (frame.type) {
    case "snapshot":
      return true;
    case "detached":
      return frame.reason === "session_not_live";
    case "event":
      return frame.event.type === "agent.turn_complete";
    case "replay":
      return frame.events.some((e) => e.type === "agent.turn_complete");
    default:
      return false;
  }
}

/** Frames that carry the session's options as the host holds them now. */
function carriesOptions(frame: SessionFrame): boolean {
  switch (frame.type) {
    case "snapshot":
      return frame.snapshot.options !== null;
    case "event":
      return frame.event.type === "session.options";
    case "replay":
      return frame.events.some((e) => e.type === "session.options");
    default:
      return false;
  }
}

export interface AddOptions {
  /** Confirmed after a `large_folder` refusal (or the publish dialog's size line). */
  allowLarge?: boolean;
  /** Create this private GitHub repository from the folder. */
  publish?: PublishTarget;
}

/** Only the options set, so an older Host's strict params never see a new key. */
function addParams(opts: AddOptions): { allow_large?: true; publish?: PublishTarget } {
  return {
    ...(opts.allowLarge ? { allow_large: true as const } : {}),
    ...(opts.publish ? { publish: opts.publish } : {}),
  };
}

export class HostController {
  readonly store: HostStore;
  private readonly unsubscribe: () => void;
  private readonly openSessions = new Set<string>();
  private readonly historyInFlight = new Set<string>();
  private readonly historyDirty = new Set<string>();
  /** Sessions being or already deleted; their transcript and options are not re-read. */
  private readonly deleted = new Set<string>();
  /**
   * Per session, bumped whenever options arrive; a `session.options` read
   * that raced a newer report is dropped.
   */
  private readonly optionsRevision = new Map<string, number>();
  private readonly terminalListeners = new Set<(id: string, out: TerminalOutput) => void>();

  constructor(
    readonly client: HostClient,
    store: HostStore = createHostStore(),
  ) {
    this.store = store;
    this.unsubscribe = client.subscribe({
      connection: (state) => {
        const was = this.store.getState().connection.status;
        this.store.dispatch({ type: "connection", state });
        if (state.status === "live" && was !== "live") void this.refreshAll();
      },
      hello: (hello) => this.store.dispatch({ type: "host", hello }),
      feed: (update) =>
        this.store.dispatch(
          update.type === "snapshot"
            ? { type: "host_snapshot", snapshot: update.snapshot }
            : { type: "host_event", event: update.event },
        ),
      session: (session_id, frame) => {
        this.store.dispatch({ type: "session_frame", session_id, frame });
        if (carriesOptions(frame)) this.bumpOptions(session_id);
        if (transcriptMayHaveChanged(frame)) void this.loadHistory(session_id);
      },
      terminal: (terminal_id, out) => {
        if (out.kind === "exit") {
          this.store.dispatch({ type: "terminal", terminal_id, patch: { alive: false } });
        } else if (out.kind === "reset") {
          this.store.dispatch({ type: "terminal", terminal_id, patch: { alive: true } });
        }
        for (const l of [...this.terminalListeners]) l(terminal_id, out);
      },
    });
  }

  dispose(): void {
    this.unsubscribe();
    this.client.stop();
  }

  // ─── lists ─────────────────────────────────────────────────────────────

  /**
   * Lists arrive with the feed snapshot; agents, open transcripts and their
   * options are re-read.
   */
  private async refreshAll(): Promise<void> {
    await Promise.allSettled([
      this.refreshAgents(),
      ...[...this.openSessions].flatMap((sid) => [this.loadHistory(sid), this.loadOptions(sid)]),
    ]);
  }

  /** Which agents this host can run. Detection only; nothing is installed. */
  async refreshAgents(): Promise<AgentSummary[]> {
    const agents = await this.client.request("agent.list", {});
    this.store.dispatch({ type: "agents", agents });
    return agents;
  }

  /**
   * Associate the host with a signed-in account. The first account wins; any
   * other is refused with `host_owned` and ownership never moves.
   */
  async claimHost(user_id: string, email?: string): Promise<HostOwner> {
    const { owner } = await this.client.request(
      "host.claim",
      email === undefined ? { user_id } : { user_id, email },
    );
    this.store.dispatch({ type: "host_owner", owner });
    return owner;
  }

  async refreshWorkspaces(): Promise<void> {
    const workspaces = await this.client.request("workspace.list", {});
    this.store.dispatch({ type: "workspaces", workspaces });
  }

  /**
   * Reload the workspace list and every listed workspace's sessions — the
   * whole tree, with the state behind its working counts.
   */
  async refreshWorkspaceTree(): Promise<void> {
    await this.refreshWorkspaces();
    const ids = (this.store.getState().workspaces ?? []).map((w) => w.workspace_id);
    await Promise.allSettled(ids.map((id) => this.refreshSessions(id)));
  }

  /** What a folder is before adding it: Git state, first-commit size, GitHub owners. Touches nothing. */
  async inspectWorkspace(path: string): Promise<WorkspaceInspection> {
    return this.client.request("workspace.inspect", { path });
  }

  /** `inspectWorkspace`, by folder names under the host user's home (from `workspace.browse`). */
  async inspectWorkspaceAt(home_path: string[]): Promise<WorkspaceInspection> {
    return this.client.request("workspace.inspect", { home_path });
  }

  /** Whether `owner/name` is free on GitHub for the Host's signed-in account. */
  async repoAvailable(owner: string, name: string): Promise<boolean> {
    return (await this.client.request("github.repo_available", { owner, name })).available;
  }

  /** One page (from 1) of the Host account's GitHub repositories. */
  async githubRepos(page?: number, refresh?: boolean): Promise<GithubRepoPage> {
    return this.client.request(
      "github.repos",
      { ...(page === undefined ? {} : { page }), ...(refresh ? { refresh } : {}) },
      { timeoutMs: GITHUB_REPOS_TIMEOUT_MS },
    );
  }

  /** Clone `owner/name` on the Host; progress arrives as `clones` state. */
  async cloneWorkspace(owner: string, name: string): Promise<CloneStart> {
    return this.client.request("workspace.clone", { owner, name });
  }

  /** `true` when the clone was stopped; `false` once it is past cloning. */
  async cancelClone(cloneId: string): Promise<boolean> {
    const r = await this.client.request("workspace.clone_cancel", { clone_id: cloneId });
    return r.cancelled;
  }

  async addWorkspace(path: string, opts: AddOptions = {}): Promise<WorkspaceInfo> {
    const workspace = await this.client.request(
      "workspace.add",
      { path, ...addParams(opts) },
      opts.publish ? { timeoutMs: PUBLISH_TIMEOUT_MS } : undefined,
    );
    this.store.dispatch({ type: "workspace_added", workspace });
    return workspace;
  }

  /** Add the folder named by `home_path` (folder names under the host user's home, from `workspace.browse`). */
  async addWorkspaceAt(home_path: string[], opts: AddOptions = {}): Promise<WorkspaceInfo> {
    const workspace = await this.client.request(
      "workspace.add",
      { home_path, ...addParams(opts) },
      opts.publish ? { timeoutMs: PUBLISH_TIMEOUT_MS } : undefined,
    );
    this.store.dispatch({ type: "workspace_added", workspace });
    return workspace;
  }

  /** `inspectWorkspace`, for a root already listed (§3.8): a plain folder left by part 1's sweep (too large to initialize), or an unborn repository. */
  async inspectRoot(workspace_id: string): Promise<WorkspaceInspection> {
    return this.client.request("workspace.inspect", { workspace_id });
  }

  /** `addWorkspace`, for a root already listed (§3.8): revives the same row and returns it. */
  async addRoot(workspace_id: string, opts: AddOptions = {}): Promise<WorkspaceInfo> {
    const workspace = await this.client.request(
      "workspace.add",
      { workspace_id, ...addParams(opts) },
      opts.publish ? { timeoutMs: PUBLISH_TIMEOUT_MS } : undefined,
    );
    this.store.dispatch({ type: "workspace_added", workspace });
    return workspace;
  }

  /**
   * Create a worktree of a root workspace, starting from branch `base` (one
   * of `git.branches`) or else the root's HEAD; the host picks branch and path.
   */
  async createWorktree(root_workspace_id: string, opts: { base?: string } = {}): Promise<WorkspaceInfo> {
    const workspace = await this.client.request("workspace.create_worktree", {
      workspace_id: root_workspace_id,
      ...(opts.base ? { base: opts.base } : {}),
    });
    this.store.dispatch({ type: "workspace_added", workspace });
    if (!(workspace.workspace_id in this.store.getState().sessionsByWorkspace)) {
      this.store.dispatch({ type: "sessions", workspace_id: workspace.workspace_id, sessions: [] });
    }
    return workspace;
  }

  /**
   * Remove a worktree. The host refuses with `workspace_busy` while agents run
   * in it, and with `workspace_dirty` for uncommitted changes unless `force`;
   * it kills the worktree's terminals itself. The host feed then updates the tree: a removed worktree
   * that keeps sessions stays listed as `removed`.
   */
  async removeWorktree(workspace_id: string, opts: { force?: boolean } = {}): Promise<WorktreeRemoved> {
    const removed = await this.client.request("workspace.remove_worktree", {
      workspace_id,
      ...(opts.force ? { force: true } : {}),
    });
    return removed;
  }

  /**
   * After pull request `number` merged, move the worktree in place to a new
   * branch from the PR's base; its sessions and terminals stay. Never
   * retried. The answer shows the new branch at once; the feed then clears the
   * PR and the diff.
   */
  async continueWorkspace(workspace_id: string, number: number): Promise<WorkspaceInfo> {
    const workspace = await this.client.request("workspace.continue", { workspace_id, number });
    this.store.dispatch({ type: "workspace_added", workspace });
    return workspace;
  }

  /**
   * Stop a session's agent: its turn is cancelled, then the agent and its
   * background work end. The session and its transcript stay; the feed reports
   * it not live, and the next send starts the agent again. Never retried: a
   * dropped connection leaves the outcome `unknown_outcome`.
   */
  async stopSession(session_id: string): Promise<void> {
    await this.client.request("session.stop", { session_id });
  }

  /**
   * Delete a session. The host refuses with `turn_in_progress` while its agent
   * is working; `session.removed` on the feed drops it from every client.
   */
  async deleteSession(session_id: string): Promise<void> {
    // Stopping the agent detaches the session before it is gone; do not re-read it.
    this.deleted.add(session_id);
    try {
      await this.client.request("session.delete", { session_id });
    } catch (e) {
      this.deleted.delete(session_id);
      void this.loadHistory(session_id);
      throw e;
    }
  }

  /**
   * Take session `session_id` back to before user turn `turn_id` (one whose
   * history turn is `revertible`). `files` restores the files and staged
   * changes the turn started with; `chat` takes the conversation back to the
   * agent reply before it (Hosts with `session.revert.chat`, agents whose
   * options say `can_rewind_chat`). Refused with `turn_in_progress`,
   * `workspace_busy` or `checkpoint_unavailable`. Never retried.
   */
  async revertSession(
    session_id: string,
    turn_id: string,
    what: { files?: boolean; chat?: boolean },
  ): Promise<SessionRevertResult> {
    return this.client.revertSession({
      session_id,
      turn_id,
      files: what.files ?? false,
      chat: what.chat ?? false,
    });
  }

  /** Stage and commit `files` (repo-relative). Never retried. */
  async commit(workspace_id: string, message: string, files: string[]): Promise<number> {
    const { committed_files } = await this.client.request("git.commit", { workspace_id, message, files });
    return committed_files;
  }

  /** The workspace's read-only diff; with `file`, that file's patch too. */
  async loadDiff(workspace_id: string, file?: string): Promise<WorkspaceDiff> {
    return this.client.request("git.diff", file === undefined ? { workspace_id } : { workspace_id, file });
  }

  /**
   * Merge the workspace's pull request now, pinned to the head the user saw.
   * Never retried. The feed's `workspace.pr_changed` brings the merged state.
   */
  async mergePr(workspace_id: string, pr: PrState, method?: MergeMethod): Promise<PrMerged> {
    const params = { workspace_id, number: pr.number, head_sha: pr.head_sha };
    return this.client.request("git.pr_merge", method === undefined ? params : { ...params, method });
  }

  /** Ask the Host to look the pull request up on GitHub now. */
  async refreshPr(workspace_id: string): Promise<PrEntry> {
    return this.client.request("git.pr", { workspace_id, refresh: true });
  }

  async refreshSessions(workspace_id: string): Promise<void> {
    const sessions = await this.client.request("session.list", { workspace_id });
    this.store.dispatch({ type: "sessions", workspace_id, sessions });
  }

  async refreshTerminals(): Promise<void> {
    const terminals = await this.client.request("terminal.list", {});
    this.store.dispatch({ type: "terminals", terminals });
  }

  // ─── sessions ──────────────────────────────────────────────────────────

  /** Show a session: load its transcript and stream its live state. */
  openSession(session_id: string): void {
    this.openSessions.add(session_id);
    this.client.attachSession(session_id);
    void this.loadHistory(session_id);
    void this.loadOptions(session_id);
  }

  closeSession(session_id: string): void {
    this.openSessions.delete(session_id);
    this.client.detachSession(session_id);
    this.optionsRevision.delete(session_id);
    this.store.dispatch({ type: "session_forget", session_id });
  }

  /**
   * Change model, effort or mode. Only between turns (`turn_in_progress`
   * otherwise); a value the agent does not offer is `option_unavailable` and
   * nothing changes. Applies to an idle session's next agent too.
   */
  async setSessionOptions(session_id: string, request: OptionsRequest): Promise<SessionOptions> {
    const { options } = await this.client.request("session.set_options", { session_id, ...request });
    this.bumpOptions(session_id);
    this.store.dispatch({ type: "session_frame", session_id, frame: { type: "options", options } });
    return options;
  }

  private bumpOptions(session_id: string): void {
    this.optionsRevision.set(session_id, (this.optionsRevision.get(session_id) ?? 0) + 1);
  }

  private async loadOptions(session_id: string): Promise<void> {
    if (!this.openSessions.has(session_id) || this.deleted.has(session_id)) return;
    const revision = this.optionsRevision.get(session_id) ?? 0;
    let options: SessionOptions;
    try {
      ({ options } = await this.client.request("session.options", { session_id }));
    } catch {
      return; // re-read on reconnect
    }
    if (!this.openSessions.has(session_id)) return;
    if ((this.optionsRevision.get(session_id) ?? 0) !== revision) return;
    this.store.dispatch({ type: "session_frame", session_id, frame: { type: "options", options } });
  }

  async startSession(params: OpParams<"session.start">): Promise<string> {
    const { session_id } = await this.client.request("session.start", params);
    this.openSession(session_id);
    return session_id;
  }

  async sendMessage(params: OpParams<"session.send">): Promise<void> {
    await this.client.sendMessage(params);
  }

  private async loadHistory(session_id: string): Promise<void> {
    if (!this.openSessions.has(session_id) || this.deleted.has(session_id)) return;
    if (this.historyInFlight.has(session_id)) {
      this.historyDirty.add(session_id);
      return;
    }
    this.historyInFlight.add(session_id);
    let settleAttempt = 0;
    try {
      do {
        this.historyDirty.delete(session_id);
        const { turns } = await this.client.request("session.history", { session_id });
        if (!this.openSessions.has(session_id)) return;
        const settled = this.store.getState().sessions[session_id]?.settled ?? null;
        const delay = HISTORY_SETTLE_DELAYS_MS[settleAttempt];
        if (settled && delay !== undefined && !historyCovers(turns, settled)) {
          // Keep the finished turn on screen until the agent's transcript has it;
          // after the last retry the host's history wins regardless.
          settleAttempt++;
          await new Promise((r) => setTimeout(r, delay));
          this.historyDirty.add(session_id);
          continue;
        }
        this.store.dispatch({ type: "session_frame", session_id, frame: { type: "history", turns } });
      } while (this.historyDirty.has(session_id));
    } catch {
      // Retried on the next stale mark or reconnect.
    } finally {
      this.historyInFlight.delete(session_id);
    }
  }

  // ─── terminals ─────────────────────────────────────────────────────────

  /** A shell, or with `agent` that agent's CLI running `command`. */
  async spawnTerminal(workspace_id: string, agent?: { agent: AgentType; command?: string }): Promise<string> {
    const { terminal_id } = await this.client.request("terminal.spawn", { workspace_id, ...agent });
    this.store.dispatch({ type: "terminal", terminal_id, patch: { alive: true, workspace_id } });
    return terminal_id;
  }

  attachTerminal(terminal_id: string): void {
    this.store.dispatch({ type: "terminal", terminal_id, patch: { attached: true } });
    this.client.attachTerminal(terminal_id);
  }

  /** Stop streaming a terminal; an ended one is then forgotten, as the host has. */
  detachTerminal(terminal_id: string): void {
    this.client.detachTerminal(terminal_id);
    const meta = this.store.getState().terminals[terminal_id];
    this.store.dispatch(
      meta && !meta.alive
        ? { type: "terminal_forget", terminal_id }
        : { type: "terminal", terminal_id, patch: { attached: false } },
    );
  }

  onTerminalOutput(listener: (terminal_id: string, out: TerminalOutput) => void): () => void {
    this.terminalListeners.add(listener);
    return () => this.terminalListeners.delete(listener);
  }
}
