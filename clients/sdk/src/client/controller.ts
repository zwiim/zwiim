// HostController: connects a HostClient to a HostStore. Every piece of
// state it writes comes from a host response or frame. Workspaces, sessions
// and terminals follow the host state feed: each (re)connect starts from its
// snapshot instead of trusting what the store had.

import type { BrowserStream, BrowserStreamHandlers } from "../browser/stream";
import { createHostStore, type HostStore } from "../state/store";
import type { TerminalOutput } from "../state/terminal";
import { backgroundMovedHistory, historyCovers, type SessionFrame, type SessionView } from "../state/session";
import type {
  AgentSummary,
  AgentType,
  BrowserEngine,
  BrowserTab,
  BrowserTabOwner,
  BrowserUiAnswer,
  BrowserViewport,
  CloneStart,
  ForkTarget,
  FollowUpOutcome,
  GithubAuth,
  GithubRepoPage,
  HostOwner,
  MergeMethod,
  OpParams,
  OpResult,
  OptionsRequest,
  PrAutoMerged,
  PrEntry,
  PrMerged,
  PrState,
  PublishTarget,
  RepositoryScript,
  ResolveStarted,
  ResolveStrategy,
  SessionEvent,
  SessionOptions,
  SessionRevertResult,
  WorkspaceDiff,
  WorkspaceGroup,
  WorkspaceInfo,
  WorkspaceInspection,
  WorkspaceLayout,
  WorkspaceRemoved,
  WorktreeRemoved,
} from "../types/protocol";
import { SCRIPT_TERMINAL_TITLES } from "../types/protocol";
import type { HostClient } from "./host-client";

/** Waits before re-reading a transcript that does not yet hold a finished turn. */
const HISTORY_SETTLE_DELAYS_MS = [250, 500, 1000, 2000];

/** `workspace.add` with `publish`: creating the GitHub repo and pushing can run long. */
const PUBLISH_TIMEOUT_MS = 330_000;

/** The Host's `gh` call has its own 15 s limit; leave room for it. */
const GITHUB_REPOS_TIMEOUT_MS = 20_000;

/** `github.sign_in` can take about 55 s at worst: gh's status (15 s), the code (10 s), then two more gh checks (15 s each). */
const GITHUB_SIGN_IN_TIMEOUT_MS = 60_000;

/**
 * Frames after which `session.history` may include more turns. `view` is the
 * session after the frame was applied.
 */
function transcriptMayHaveChanged(frame: SessionFrame, view: SessionView | undefined): boolean {
  switch (frame.type) {
    case "snapshot":
      // A reconnect mid-turn changes nothing on disk; the reducer compares
      // the Host's history generation with the one on screen.
      return view?.historyStale ?? true;
    case "detached":
      return frame.reason === "session_not_live";
    case "event":
      return frame.event.type === "agent.turn_complete" || movedHistory(frame.event, view);
    case "replay":
      return frame.events.some((e) => e.type === "agent.turn_complete" || movedHistory(e, view));
    default:
      return false;
  }
}

/**
 * A background event whose history generation differs from the history on
 * screen: turns landed outside a turn. Checked against the view after the
 * frame, so a read already done at that generation is not repeated.
 */
function movedHistory(ev: SessionEvent, view: SessionView | undefined): boolean {
  return view !== undefined && backgroundMovedHistory(ev, view);
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
  /** The last write the Host answered, and the layout the feed held then. */
  private answeredLayout: { layout: WorkspaceLayout; over: WorkspaceLayout | null } | null = null;

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
      feed: (update) => {
        this.store.dispatch(
          update.type === "snapshot"
            ? { type: "host_snapshot", snapshot: update.snapshot }
            : { type: "host_event", event: update.event },
        );
        // Another device changed the open session (a plan continued in a new
        // chat, say): its saved plan and links are only in the history. Reads
        // coalesce in `loadHistory`; the read can raise one more change (a
        // first title or model), after which none.
        if (update.type === "event" && update.event.type === "session.changed") {
          void this.loadHistory(update.event.payload.session.session_id);
        }
      },
      session: (session_id, frame) => {
        this.store.dispatch({ type: "session_frame", session_id, frame });
        if (carriesOptions(frame)) this.bumpOptions(session_id);
        if (transcriptMayHaveChanged(frame, this.store.getState().sessions[session_id])) {
          void this.loadHistory(session_id);
        }
      },
      terminal: (terminal_id, out) => {
        if (out.kind === "exit") {
          this.store.dispatch({ type: "terminal", terminal_id, patch: { alive: false } });
        } else if (out.kind === "reset") {
          this.store.dispatch({ type: "terminal", terminal_id, patch: { alive: true } });
        }
        for (const l of [...this.terminalListeners]) l(terminal_id, out);
      },
      browser: (update) => this.store.dispatch({ type: "browser", update }),
    });
  }

  dispose(): void {
    this.unsubscribe();
    this.client.stop();
  }

  // ─── lists ─────────────────────────────────────────────────────────────

  /**
   * Lists arrive with the feed snapshot; agents and open sessions' options
   * are re-read, and so is a transcript that may have moved on. A session
   * that is re-attaching with a current transcript is not: its replay or
   * snapshot says whether the history changed while the client was away.
   */
  private async refreshAll(): Promise<void> {
    await Promise.allSettled([
      this.refreshAgents(),
      ...[...this.openSessions].flatMap((sid) => [
        ...(this.historyMayBeBehind(sid) ? [this.loadHistory(sid)] : []),
        this.loadOptions(sid),
      ]),
    ]);
  }

  /**
   * Whether a reconnect must re-read `session_id`'s transcript: it was never
   * read or marked stale, or the session is not re-attaching (its agent was
   * gone), so no replay or snapshot will tell.
   */
  private historyMayBeBehind(session_id: string): boolean {
    const view = this.store.getState().sessions[session_id];
    if (!view || view.historyStale) return true;
    return view.attach !== "live" && view.attach !== "attaching";
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

  /** How the Host's gh is signed in to GitHub. */
  async githubAuthStatus(): Promise<GithubAuth> {
    return this.client.request("github.auth_status", {}, { timeoutMs: GITHUB_REPOS_TIMEOUT_MS });
  }

  /** Start signing the Host's gh in to GitHub; `signing_in` carries the code to enter. */
  async githubSignIn(): Promise<GithubAuth> {
    return this.client.request("github.sign_in", {}, { timeoutMs: GITHUB_SIGN_IN_TIMEOUT_MS });
  }

  /** End the Host's GitHub sign-in in progress. */
  async cancelGithubSignIn(): Promise<GithubAuth> {
    return this.client.request("github.cancel_sign_in", {}, { timeoutMs: GITHUB_REPOS_TIMEOUT_MS });
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
   * Remove a repository from the Host: archive each of its worktrees as
   * `removeWorktree` does, then hide the root. Its folder is untouched, and
   * adding the folder again brings back the same workspace with its sessions.
   * Refused like `removeWorktree`, before anything is archived; the host feed
   * then drops the root and its worktrees.
   */
  async removeWorkspace(workspace_id: string, opts: { force?: boolean } = {}): Promise<WorkspaceRemoved> {
    return this.client.request("workspace.remove", {
      workspace_id,
      ...(opts.force ? { force: true } : {}),
    });
  }

  /** A new group, at the end of the top level. The feed then shows it. */
  async createWorkspaceGroup(name: string): Promise<WorkspaceGroup> {
    return this.client.request("workspace.group_create", { name });
  }

  async renameWorkspaceGroup(group_id: string, name: string): Promise<WorkspaceGroup> {
    return this.client.request("workspace.group_rename", { group_id, name });
  }

  /** Delete a group. Its repositories move to the top level, in its place. */
  async deleteWorkspaceGroup(group_id: string): Promise<void> {
    await this.client.request("workspace.group_delete", { group_id });
  }

  /**
   * Ask the Host to arrange its workspaces like `layout`. Answers nothing: a
   * response can land after a newer feed event, so only the feed moves
   * `state.layout`.
   */
  async setWorkspaceLayout(layout: WorkspaceLayout): Promise<void> {
    const entries = layout.entries.map((e) =>
      e.kind === "group"
        ? { kind: e.kind, group_id: e.group_id, workspaces: e.workspaces }
        : e,
    );
    await this.client.request("workspace.layout_set", { entries });
    this.answeredLayout = { layout, over: this.store.getState().layout };
  }

  /**
   * What the next move builds on. The feed's layout trails an answered write,
   * and the first layout after the answer has that write in it.
   */
  latestLayout(): WorkspaceLayout | null {
    const stored = this.store.getState().layout;
    return this.answeredLayout?.over === stored ? this.answeredLayout.layout : stored;
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
   * Ask session `session_id`'s agent to resolve the worktree's conflicts with
   * its base, in a turn the Host writes. Never retried. The feed then names
   * the session in `resolving` until the turn ends; merging stays the user's.
   */
  async resolveConflicts(
    workspace_id: string,
    session_id: string,
    strategy?: ResolveStrategy,
  ): Promise<ResolveStarted> {
    return this.client.request("workspace.resolve_conflicts", {
      workspace_id,
      session_id,
      ...(strategy ? { strategy } : {}),
    });
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

  /**
   * Start a new session from turn `turn_id` (one whose history turn lists
   * `target` in `fork_targets`), leaving this one as it is. Refused with
   * `turn_in_progress`, `workspace_busy`, `checkpoint_unavailable`,
   * `root_workspace` or `fork_unavailable`. Never retried.
   */
  async forkSession(session_id: string, turn_id: string, target: ForkTarget): Promise<OpResult<"session.fork">> {
    return this.client.forkSession({ session_id, turn_id, target });
  }

  /** Take a fork's waiting transcript back; the session's `session.changed` updates the view. Never retried. */
  async removeTranscript(session_id: string): Promise<void> {
    await this.client.removeTranscript({ session_id });
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

  /**
   * Turn GitHub's auto-merge for the workspace's pull request on, pinned to
   * the head the user saw, or off. Never retried. GitHub does the merge; the
   * feed's `workspace.pr_changed` brings it.
   */
  async setPrAutoMerge(workspace_id: string, pr: PrState, enabled: boolean, method?: MergeMethod): Promise<PrAutoMerged> {
    const params = enabled
      ? { workspace_id, number: pr.number, enabled, head_sha: pr.head_sha }
      : { workspace_id, number: pr.number, enabled };
    return this.client.request("git.pr_auto_merge", method === undefined ? params : { ...params, method });
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

  /** See `HostClient.sendFollowUp`. */
  async sendFollowUp(params: OpParams<"session.follow_up">): Promise<FollowUpOutcome> {
    return (await this.client.sendFollowUp(params)).outcome;
  }

  async editFollowUp(session_id: string, follow_up_id: string, text: string): Promise<void> {
    await this.client.request("session.follow_up_edit", { session_id, follow_up_id, text });
  }

  async removeFollowUp(session_id: string, follow_up_id: string): Promise<void> {
    await this.client.request("session.follow_up_remove", { session_id, follow_up_id });
  }

  /** See `HostClient.sendQueuedFollowUp`. */
  async sendQueuedFollowUp(session_id: string, follow_up_id: string): Promise<FollowUpOutcome> {
    return (await this.client.sendQueuedFollowUp({ session_id, follow_up_id })).outcome;
  }

  /** Send a session's failed can't-reach turn again; see `HostClient.retrySession`. */
  async retrySession(session_id: string): Promise<void> {
    await this.client.retrySession({ session_id });
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
        const { turns, history_generation, last_error, progress, continues, continued_in, forked_from, pending_transcript } = await this.client.request("session.history", { session_id });
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
        this.store.dispatch({
          type: "session_frame",
          session_id,
          frame: {
            type: "history",
            turns,
            generation: history_generation ?? null,
            lastError: last_error ?? null,
            savedPlan: progress ?? null,
            continues: continues ?? null,
            continuedIn: continued_in?.session_id ?? null,
            forkedFrom: forked_from ?? null,
            pendingTranscript: pending_transcript ?? null,
          },
        });
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

  /**
   * Run a worktree's repository script in a new terminal, which takes the
   * place of its live one of that script; the list then drops that one.
   */
  async runScript(workspace_id: string, script: RepositoryScript): Promise<string> {
    const { terminal_id } = await this.client.request("workspace.run_script", { workspace_id, script });
    const title = SCRIPT_TERMINAL_TITLES[script];
    this.store.dispatch({ type: "terminal", terminal_id, patch: { alive: true, workspace_id, title } });
    await this.refreshTerminals();
    // The ones it replaced, still shown if attached, go with them.
    for (const t of Object.values(this.store.getState().terminals)) {
      if (t.terminal_id !== terminal_id && t.workspace_id === workspace_id && t.title === title && !t.alive) {
        this.detachTerminal(t.terminal_id);
      }
    }
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

  // ─── browser ───────────────────────────────────────────────────────────

  /** Follow the Host browser's tabs and engine into `store.browser`. Idempotent. */
  watchBrowser(): void {
    this.client.watchBrowser();
  }

  /** Open a tab, on `url` or a blank page. The engine starts (or downloads) first if it must. */
  async openBrowserTab(owner: BrowserTabOwner, url?: string): Promise<BrowserTab> {
    const tab = await this.client.request("browser.tab.open", { owner, ...(url ? { url } : {}) });
    this.store.dispatch({ type: "browser", update: { type: "browser.tab.state", tab } });
    return tab;
  }

  async closeBrowserTab(tab_id: string): Promise<void> {
    await this.client.request("browser.tab.close", { tab_id });
  }

  /** Load `url` in the tab. */
  async navigateBrowserTab(tab_id: string, url: string): Promise<BrowserTab> {
    return this.browserTabStep(await this.client.request("browser.tab.navigate", { tab_id, url }));
  }

  async browserTabHistory(tab_id: string, action: "back" | "forward" | "reload" | "stop"): Promise<BrowserTab> {
    return this.browserTabStep(await this.client.request("browser.tab.navigate", { tab_id, action }));
  }

  /** Stop the engine and start it again, as after a crash loop. */
  async resetBrowserEngine(): Promise<BrowserEngine> {
    const { engine } = await this.client.request("browser.engine.reset", {});
    this.store.dispatch({ type: "browser", update: { type: "browser.engine.state", engine } });
    return engine;
  }

  /**
   * Start streaming a tab at `viewport`. Open the returned stream with
   * `openBrowserStream` within 30 s; close it with `closeBrowserView`.
   */
  async openBrowserView(tab_id: string, viewport: BrowserViewport): Promise<string> {
    const { stream_id } = await this.client.request("browser.view.open", { tab_id, viewport });
    return stream_id;
  }

  async closeBrowserView(stream_id: string): Promise<void> {
    await this.client.request("browser.view.close", { stream_id });
  }

  /** Find `query` in the tab and go to its next match (the one before with `backwards`). */
  async findInBrowserTab(
    tab_id: string,
    query: string,
    backwards = false,
  ): Promise<{ matches: number; current: number }> {
    return this.client.request("browser.tab.find", { tab_id, query, backwards });
  }

  /** Save the tab's page as a PDF in its downloads folder; the file's name. */
  async printBrowserTab(tab_id: string): Promise<string> {
    return (await this.client.request("browser.tab.print", { tab_id })).name;
  }

  /** Let the person point at an element of the tab's page; the outcome arrives in the browser state's `picks`. */
  async pickInBrowserTab(tab_id: string): Promise<void> {
    await this.client.request("browser.tab.pick", { tab_id });
  }

  /** Call the tab's element pick off. */
  async cancelBrowserPick(tab_id: string): Promise<void> {
    await this.client.request("browser.tab.pick_cancel", { tab_id });
  }

  /** Drop workspace files (`paths`) or uploaded ones (`attachment_ids`) on the page at `(x, y)`. */
  async dropOnBrowserTab(
    tab_id: string,
    x: number,
    y: number,
    workspace_id: string,
    files: { paths?: string[]; attachment_ids?: string[] },
  ): Promise<void> {
    await this.client.request("browser.tab.drop", { tab_id, x, y, workspace_id, ...files });
  }

  /** Session `session_id`'s strict site list; empty for none. */
  async browserSites(session_id: string): Promise<string[]> {
    return (await this.client.request("browser.sites.get", { session_id })).sites;
  }

  /** Replace session `session_id`'s strict site list (empty removes it); the list as kept. */
  async setBrowserSites(session_id: string, sites: string[]): Promise<string[]> {
    return (await this.client.request("browser.sites.set", { session_id, sites })).sites;
  }

  /** Answer what a tab waits on. Another client may have answered first: then `request_not_found`. */
  async answerBrowserUi(request_id: string, answer: BrowserUiAnswer): Promise<void> {
    await this.client.request("browser.ui.answer", { request_id, answer });
  }

  /** Watch a stream `openBrowserView` returned, and send it input. */
  openBrowserStream(stream_id: string, handlers: BrowserStreamHandlers): Promise<BrowserStream> {
    return this.client.openBrowserStream(stream_id, handlers);
  }

  private browserTabStep(tab: BrowserTab): BrowserTab {
    this.store.dispatch({ type: "browser", update: { type: "browser.tab.state", tab } });
    return tab;
  }
}
