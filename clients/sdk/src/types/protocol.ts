// Wire types for Zwiim Protocol V1 (`/zwiim/v1`). These mirror the host's
// projections in `runtime/host/src/protocol/{http,events,ws}.rs`; clients
// address everything by `workspace_id`, `session_id` and `terminal_id`.

export const PROTOCOL_PREFIX = "/zwiim/v1";
export const EVENTS_SUBPROTOCOL = "zwiim-events";
export const TOKEN_SUBPROTOCOL_PREFIX = "zwiim-token.";

// ─── errors ──────────────────────────────────────────────────────────────

export type HostErrorCode =
  | "unauthenticated"
  | "invalid_request"
  | "invalid_input"
  | "not_found"
  | "turn_in_progress"
  | "session_not_live"
  | "agent_not_installed"
  | "option_unavailable"
  | "host_owned"
  | "unknown_operation"
  | "method_not_allowed"
  | "already_exists"
  | "permission_denied"
  | "not_a_git_repository"
  | "workspace_busy"
  | "workspace_dirty"
  | "large_folder"
  | "repository_init_failed"
  | "workspace_removed"
  | "identity_unavailable"
  /** `repository.settings.*`: the Host's keychain, which keeps environment variables, would not open. */
  | "keychain_unavailable"
  | "cloud_unavailable"
  | "host_restarting"
  | "update_unavailable"
  | "not_mergeable"
  | "pr_changed"
  /** `git.pr_auto_merge`: the repository has "Allow auto-merge" off on GitHub. */
  | "auto_merge_disabled"
  /** `workspace.continue`: the pull request is not merged on GitHub. */
  | "pr_not_merged"
  /** `workspace.continue`: the branch moved past the merged head, so Continue would strand commits. */
  | "head_not_merged"
  | "gh_unavailable"
  | "gh_unauthenticated"
  /** GitHub could not be reached from the Host (not this client's connection). */
  | "github_unreachable"
  /** `workspace.resolve_conflicts`: the branch has no conflicts with its base (checked after a fetch). */
  | "no_conflicts"
  /** `session.start` / `session.send` / `terminal.spawn` / `workspace.run_script` in a root: sessions run in a worktree. */
  | "root_workspace"
  /** `workspace.run_script`: the repository has no such script. Details `{ script }`. */
  | "script_not_set"
  /** `workspace.add` with `publish`: GitHub did not take the repository. Details `{ created_on_github }`. */
  | "publish_failed"
  /** `attachments.upload`: the file is larger than 10 MB. */
  | "attachment_too_large"
  /** `session.start` / `session.send`: an `attachment` block names a file that is gone. */
  | "attachment_not_found"
  /** `attachments.upload`: nothing is left of the file name once cleaned. */
  | "invalid_name"
  /** `files.download`: the file is larger than 10 MB. */
  | "file_too_large"
  | "file_too_large_for_remote"
  /** `session.revert`: the turn has no saved state that is certainly its own. */
  | "checkpoint_unavailable"
  /** `session.retry` / `session.retry_in_new_chat`: the session has no saved can't-reach turn to retry. */
  | "nothing_to_retry"
  /** `agent.error` code: the agent could not reach its provider's server. */
  | "agent_unreachable"
  /** `host.instructions.set`: an instruction text is larger than 8 KiB. */
  | "too_large"
  | "internal";

export interface WireError {
  code: HostErrorCode | (string & {});
  message: string;
  /** Structured facts, only on codes that document them. */
  details?: Record<string, unknown>;
}

/** `workspace_busy` details: what still runs in the worktree. */
export interface WorkspaceBusyDetails {
  live_sessions: number;
  terminals: number;
  /** The sessions whose agents run there (an agent not bound to one yet is only counted). */
  session_ids: string[];
  /**
   * Set when the repository, not an agent, is busy: the operation it is in the
   * middle of (`merge`, `rebase`, `cherry_pick`, `revert`, `conflict`, `index_lock`).
   */
  operation?: string;
}

/** `host_restarting` details: the Host is draining to stop or restart. */
export interface HostRestartingDetails {
  /** When to try again; the connection itself reconnects on its own. */
  retry_after_ms: number;
}

/** `not_mergeable` details: why GitHub would not merge it now, in words for the user. */
export interface NotMergeableDetails {
  reason: string;
}

/** `workspace_dirty` details: uncommitted entries a forced removal discards. */
export interface WorkspaceDirtyDetails {
  changes: number;
}

/** `large_folder` details: what the folder's first commit would hold. `at_least`: counting stopped at a limit. */
export interface LargeFolderDetails {
  files: number;
  bytes: number;
  at_least: boolean;
}

/** `option_unavailable` details: the option and value the agent does not offer. */
export interface OptionUnavailableDetails {
  option: "model" | "effort" | "mode" | (string & {});
  value: string;
}

/** `host_owned` details: the account that already owns the host. */
export interface HostOwnedDetails {
  owner_email: string | null;
}

// ─── device ──────────────────────────────────────────────────────────────

/**
 * A one-time key-pairing invitation: the v2 QR payload. It carries no network
 * address; the device pins `host_static_pubkey`.
 */
export interface KeyPairingInvitation {
  v: 2;
  host_id: string;
  host_static_pubkey: string;
  code: string;
  expires_at: string;
}

/** A device the host trusts, or has revoked. */
export interface DeviceInfo {
  device_id: string;
  device_name: string;
  status: "trusted" | "revoked";
  /** X25519 static public key; `null` for a device paired with a bearer credential. */
  device_pubkey: string | null;
  paired_at: string;
  revoked_at: string | null;
  last_seen_at: string | null;
}

// ─── cloud ───────────────────────────────────────────────────────────────

/** The host's link to Zwiim Cloud. Public facts only; never a credential. */
export interface CloudStatus {
  enrolled: boolean;
  cloud_url: string | null;
  /** The account the host is enrolled into. */
  user_id: string | null;
  enrolled_at: string | null;
  connection: "not_enrolled" | "connecting" | "online" | "offline";
  connected_since: string | null;
  last_error: string | null;
}

// ─── keep awake ──────────────────────────────────────────────────────────

/** When the host keeps its Mac awake so phones can still reach it. */
export type KeepAwakeMode = "off" | "while_working" | "always";

/**
 * Whether the host's Mac stays awake, and why not when it won't. Defaults
 * to always, on AC power only; lets go on battery below 10%.
 */
export interface KeepAwakeStatus {
  /** False where the host cannot keep the computer awake (not a Mac). */
  supported: boolean;
  mode: KeepAwakeMode;
  /** Also keep awake on battery (still stops below 10%). */
  on_battery: boolean;
  lid_closed: { support: "no_lid" | "unavailable" | "needs_setup" | "available"; enabled: boolean };
  /** The Mac is being kept awake right now. */
  active: boolean;
  /** Why the Mac will sleep although the mode asks to keep it awake. */
  blocked_by: "on_battery" | "low_battery" | "thermal" | null;
  warnings: ("lid_close_will_sleep" | "on_battery_will_drain")[];
  power: { source: "ac" | "battery" | "unknown"; battery_percent: number | null };
}

// ─── lan ─────────────────────────────────────────────────────────────────

/**
 * The host's direct LAN access. Off until the operator turns it on; while
 * on, the host advertises its id, port and public key on the local network
 * and accepts the device tunnel on `port`.
 */
export interface LanStatus {
  enabled: boolean;
  listening: boolean;
  port: number | null;
  advertised: boolean;
  last_error: string | null;
}

/**
 * Where a paired device on the host's network can dial it directly
 * (`host.lan_endpoint`): the LAN port and the machine's addresses. Only a
 * hint: the device tunnel's handshake still proves the host holds its
 * pinned key.
 */
export interface HostLanEndpoint {
  addresses: string[];
  port: number;
}

// ─── host ────────────────────────────────────────────────────────────────

/** The account a host belongs to. Never a token. */
export interface HostOwner {
  user_id: string;
  email: string | null;
}

export interface HostHello {
  host_id: string;
  /** `zwiim/<major>`. */
  protocol: string;
  /** The Host's product version. Display only: compatibility uses `revision`. */
  version: string;
  /** Null until an account claims the host. */
  owner: HostOwner | null;
  /** Protocol revision the Host speaks. Absent on Hosts older than the handshake. */
  revision?: number;
  /** Oldest client revision the Host serves. */
  min_client_revision?: number;
  /** Optional features the Host offers. Unknown names are ignored. */
  capabilities?: string[];
  /** The Host's own verdict for the client that introduced itself; null if it did not. */
  compat?: "compatible" | "update_required" | null;
}

/** The `client` a `host.hello` body carries (see `helloClient`). */
export interface HostHelloParams {
  client?: {
    app: string;
    version: string;
    build?: string;
    platform?: string;
    revision: number;
    capabilities: readonly string[];
  };
}

// ─── workspace / files / git ─────────────────────────────────────────────

export type WorkspaceKind = "root" | "worktree" | (string & {});

export interface WorkspaceInfo {
  workspace_id: string;
  name: string;
  alias: string | null;
  kind: WorkspaceKind;
  parent_workspace_id: string | null;
  /**
   * A Git repository now (`.git` in the workspace's own folder). A plain
   * folder is still a full workspace (files, terminals, sessions); only a
   * repository has branches, changes and worktrees. False once removed.
   */
  is_git_repo: boolean;
  /** Checked-out branch, read live; null when detached, not a repository, or removed. */
  branch: string | null;
  /** Worktrees only: the root branch and commit it was created from. */
  base_branch: string | null;
  base_commit: string | null;
  /** A removed worktree, kept listed while it still holds sessions. */
  removed: boolean;
  /**
   * Roots only: "~" or "~/Projects/zwiim" when inside the host user's home;
   * null for worktrees and roots outside home. Presentation only.
   */
  display_path: string | null;
}

/** A folder in `workspace.browse`. */
export interface BrowseEntry {
  /** One path segment; never "." / ".." / dot-prefixed. */
  name: string;
  /** `<folder>/.git` exists. */
  is_git_repo: boolean;
  /** A live workspace already points here. */
  is_workspace: boolean;
  /** The host can list it. */
  is_readable: boolean;
}

/** The folders inside a folder under the host user's home. */
export interface BrowseListing {
  /** The folder's name segments under home; [] is home. */
  path: string[];
  /** "~" or "~/Projects": presentation only. */
  display_path: string;
  /** Child folders, sorted case-insensitively. */
  entries: BrowseEntry[];
  /** More than 1,000 folders; the first 1,000 are listed. */
  truncated: boolean;
}

/** A folder's Git state before adding it: no repository, a repository with no `origin`, or one with an `origin`. */
export type GitState = "none" | "no_origin" | "origin";

/** One GitHub account the Host's signed-in user can publish to. */
export interface GithubOwner {
  login: string;
  kind: "user" | "org";
}

/** Whether the Host can reach GitHub to publish, and as whom. */
export type GithubStatus =
  | { available: true; login: string; owners: GithubOwner[] }
  | { available: false; reason: "gh_unavailable" | "gh_unauthenticated" | "github_unreachable" };

/** `workspace.inspect`: what a folder is before adding it. */
export interface WorkspaceInspection {
  git: GitState;
  branch: string | null;
  suggested_name: string;
  first_commit: { files: number; bytes: number; at_least: boolean; large: boolean } | null;
  github: GithubStatus | null;
}

/** A private GitHub repository to create from a folder on `workspace.add`. */
export interface PublishTarget {
  owner: string;
  name: string;
}

/** One repository the Host's GitHub account can see (`github.repos`). */
export interface GithubRepo {
  owner: string;
  name: string;
  private: boolean;
  fork: boolean;
  archived: boolean;
  description: string | null;
  pushed_at: string | null;
  size_kb: number;
  default_branch: string | null;
  /** The root workspace already opened on this repository, if any. */
  workspace_id: string | null;
}

export interface GithubRepoPage {
  repos: GithubRepo[];
  next_page: number | null;
}

export type CloneState = "queued" | "cloning" | "adding" | "done" | "failed" | "cancelled";
export type ClonePhase = "receiving" | "resolving" | "checkout";

/**
 * A GitHub clone on the Host. States only move forward but may be skipped
 * (the feed coalesces); `done` arrives after its workspaces.
 */
export interface CloneJob {
  clone_id: string;
  owner: string;
  name: string;
  state: CloneState;
  phase: ClonePhase | null;
  percent: number | null;
  workspace_id: string | null;
  worktree_id: string | null;
  error: { code: string; message: string } | null;
}

/** A new clone job, or the root already opened on the repository. */
export type CloneStart = { clone_id: string } | { workspace_id: string };

export interface WorktreeRemoved {
  workspace_id: string;
  parent_workspace_id: string;
  /** Sessions that stay (read-only) on the removed worktree. */
  kept_sessions: number;
  /** The branch the worktree had (revision 17); null when it had none. */
  branch?: string | null;
  /** The branch was deleted with it (`git.delete_branch_on_remove`). */
  branch_deleted?: boolean;
  /** Why deleting was asked for but the branch was kept: unpushed commits, the base branch. */
  branch_kept_reason?: string | null;
  /** How the repository's archive script ended, when one ran (revision 19). */
  archive_script?: ScriptRun;
}

export interface WorkspaceRemoved {
  workspace_id: string;
  /** The worktrees archived with it, as `workspace.remove_worktree` does. */
  archived_worktrees: string[];
}

/** How a repository script the Host ran ended. */
export interface ScriptRun {
  /** Null when it was stopped, killed or never started. */
  exit_code: number | null;
  /** Stopped after its time limit (5 minutes for an archive script). */
  timed_out: boolean;
  /** The end of what it printed, or why it could not start. */
  output: string;
}

export interface WorkspaceDetail extends WorkspaceInfo {
  git_branch: string | null;
  default_agent: AgentType | null;
}

export type FileTreeNode =
  | { kind: "file"; name: string; path: string }
  | { kind: "dir"; name: string; path: string; children: FileTreeNode[]; symlink?: boolean };

export interface FileContent {
  path: string;
  content: string;
  etag: string;
  mtime_ms: number | null;
  readonly: boolean;
  line_ending: string;
}

/** `files.write`: the saved file's new `etag`, for the next save. */
export interface FileSaveResult {
  path: string;
  etag: string;
  mtime_ms: number | null;
  readonly: boolean;
  line_ending: string;
}

/** `files.download`: a workspace file's bytes. */
export interface FileDownload {
  name: string;
  mime_type: string;
  /** The whole file's size, also for a range. */
  size: number;
  /** Where a ranged read started; absent for a whole-file read. */
  offset?: number;
  /** Standard base64. */
  data: string;
}

/** `attachments.upload`: the stored file; send `attachment_id` in a prompt block. */
export interface UploadedAttachment {
  attachment_id: string;
  /** The cleaned file name the Host stored. */
  name: string;
  size: number;
  mime_type: string;
}

export interface GitStatusEntry {
  status: string;
  file: string;
}

export interface GitLogFileChange {
  path: string;
  status: string;
  additions: number;
  deletions: number;
}

export interface GitLogEntry {
  hash: string;
  full_hash: string;
  author: string;
  date: string;
  message: string;
  files: GitLogFileChange[];
  pushed: boolean | null;
}

/** Branches a new worktree can start from, and the one to suggest (the remote's default branch, else the checked-out one). */
export interface WorktreeBases {
  local: string[];
  remote: string[];
  default: string | null;
}

export interface GitLogResult {
  entries: GitLogEntry[];
  has_upstream: boolean;
}

export type DiffChange = "added" | "modified" | "deleted";

export interface DiffFile {
  path: string;
  change: DiffChange;
  /** Null for binary and oversized files. */
  additions: number | null;
  deletions: number | null;
  binary: boolean;
  too_large: boolean;
  untracked: boolean;
}

export interface WorkspaceDiff {
  /** What the diff is against; null for a repository without commits. */
  base_commit: string | null;
  files: DiffFile[];
  /** More files changed than the host lists. */
  truncated: boolean;
  /** Only with `file`: its patch, or null when binary/oversized. */
  patch?: string | null;
}

// ─── terminal ────────────────────────────────────────────────────────────

export interface TerminalInfo {
  terminal_id: string;
  title: string;
  /** The workspace the terminal runs in; null outside every workspace. */
  workspace_id: string | null;
}

export interface TerminalSnapshot {
  terminal_id: string;
  alive: boolean;
  data: string;
  /** Seq of the last chunk in `data`; 0 when unknown. */
  seq: number;
}

export type TerminalEvent =
  | { terminal_id: string; seq: number; type: "terminal.output"; payload: { data: string } }
  | { terminal_id: string; seq: number; type: "terminal.exit"; payload: Record<string, never> };

// ─── session ─────────────────────────────────────────────────────────────

export type AgentType = string;

// ─── agents and session options ──────────────────────────────────────────

/** One value of an agent-defined option (a model, an effort level). */
export interface OptionValue {
  value: string;
  label: string;
  description: string | null;
}

/** An agent-defined choice: what is in effect and what may be chosen. */
export interface ChoiceOption {
  current: string;
  values: OptionValue[];
}

/**
 * How the agent operates. `normal` asks before acting, `plan` plans first,
 * `unattended` runs tools without asking (only where the agent supports it).
 */
export type SessionMode = "normal" | "plan" | "unattended";

export interface ModeOption {
  current: SessionMode;
  /** The agent's own name for the mode in effect. */
  label: string;
  available: SessionMode[];
}

/** What a session can choose; an option the agent does not offer is null. */
export interface SessionOptions {
  model: ChoiceOption | null;
  effort: ChoiceOption | null;
  mode: ModeOption | null;
  /**
   * Whether `session.revert` can rewind this session's chat as well as its
   * files. Present on `session.options` replies; absent on older Hosts and on
   * live option changes (keep the last value seen).
   */
  can_rewind_chat?: boolean;
}

/** A requested change; absent fields are left alone. */
export interface OptionsRequest {
  model?: string;
  effort?: string;
  mode?: SessionMode;
}

/** A slash command the agent offers. Run it by sending `/name args` as text. */
export interface SessionCommand {
  /** Without the leading `/`. */
  name: string;
  description: string;
  /** What to type after the name, when the command takes input. */
  hint: string | null;
}

export type AgentUnavailableReason =
  | "not_installed"
  | "unsupported_platform"
  | "disabled"
  /** Host-managed adapters (Claude Code, Codex) only. */
  | "installing"
  | "needs_network"
  | "setup_failed";

/** `supported`: Claude Code and Codex. `experimental`: every other agent, as-is. */
export type AgentSupport = "supported" | "experimental";

export type AdapterSetup =
  | { state: "idle" }
  | { state: "installing" }
  | { state: "needs_network"; message: string }
  | { state: "failed"; message: string };

/** A host-managed adapter: installed from the host build's lock, never from PATH. */
export interface AdapterStatus {
  /** The version that launches, if any. */
  version: string | null;
  /** The version this host build pins. */
  locked_version: string | null;
  installed: boolean;
  /** Answered the health probe; null until probed since the host started. */
  healthy: boolean | null;
  /** Vendor credentials were found (the agent itself is the final judge). */
  authenticated: boolean;
  /** Installed, healthy and authenticated. */
  usable: boolean;
  setup: AdapterSetup;
  /** `dev_path` only on a development host. */
  source: "managed" | "dev_path" | null;
}

export interface AgentSummary {
  agent: AgentType;
  name: string;
  /** Whether a session can be started with this agent on this host. */
  available: boolean;
  reason: AgentUnavailableReason | (string & {}) | null;
  support: AgentSupport | (string & {});
  /** Null for agents the host does not manage. */
  adapter: AdapterStatus | null;
  /** What a new session can choose; null before the agent's first session here. */
  options: SessionOptions | null;
  /**
   * Effort belongs to the model: the effort a new session can choose on each
   * model that has run here, by model value; null where that model offers
   * none. See `effortFor`.
   */
  effort_by_model: Record<string, ChoiceOption | null>;
  /** The slash commands it offered in its last session here; null before its first. */
  commands: SessionCommand[] | null;
  /** It is the agent new sessions start with (`defaults.agent`). Capability `settings`. */
  default?: boolean;
  /**
   * The Host's defaults this agent offers, already current in `options` and
   * `effort_by_model`; null where none applies. Capability `settings`.
   */
  defaults?: AgentDefaults;
}

/** What a new session of an agent starts with unless the client picks otherwise. */
export interface AgentDefaults {
  model: string | null;
  effort: string | null;
  mode: SessionMode | null;
}

/**
 * The Host's settings, shared by every device. Keys: `defaults.agent`,
 * `defaults.mode`, `defaults.claude_code.model` / `.effort`,
 * `defaults.codex.model` / `.effort`, and from revision 17 (capability
 * `settings.git`) `git.branch_prefix` (`github` | `custom`),
 * `git.branch_prefix_custom` and `git.delete_branch_on_remove` (a boolean),
 * and from revision 18 (capability `settings.prompts`) the user's own
 * instructions to agents for pull requests and branch names:
 * `prompts.pull_request` and `prompts.branch_name`, each up to
 * `MAX_PROMPT_BYTES` (their other instructions are `host.instructions`), and
 * from revision 20 (capability `settings.advanced`)
 * `terminal.scrollback`, the bytes of output each terminal keeps (within
 * `SCROLLBACK_RANGE`; unset keeps `DEFAULT_SCROLLBACK_BYTES`), and
 * `git.push_sets_upstream`, whether a plain `git push` in a worktree sets the
 * branch's upstream (unset is on). Null when unset.
 */
export type HostSettingKey =
  | "defaults.agent"
  | "defaults.mode"
  | "defaults.claude_code.model"
  | "defaults.claude_code.effort"
  | "defaults.codex.model"
  | "defaults.codex.effort"
  | "git.branch_prefix"
  | "git.branch_prefix_custom"
  | "git.delete_branch_on_remove"
  | "prompts.pull_request"
  | "prompts.branch_name"
  | "terminal.scrollback"
  | "git.push_sets_upstream";

/** Longest `prompts.*` setting the Host accepts, in UTF-8 bytes. */
export const MAX_PROMPT_BYTES = 8000;

/** What `terminal.scrollback` can be, in bytes. */
export const SCROLLBACK_RANGE = { min: 64 * 1024, max: 16 * 1024 * 1024 } as const;

/** What a terminal keeps while `terminal.scrollback` is unset, in bytes. */
export const DEFAULT_SCROLLBACK_BYTES = 1024 * 1024;

/** `host.folders`: where the Host keeps its data, worktrees and clones (revision 20). */
export interface HostFolders {
  data: string;
  worktrees: string;
  repositories: string;
}

export interface HostSettings {
  values: Partial<Record<HostSettingKey | (string & {}), string | number | boolean | null>>;
}

/**
 * One repository's own settings, from revision 19 (capability
 * `repository.settings`), shared by its worktrees; null when unset.
 *
 * 1. `scripts.setup`, run in a `Setup` terminal of each new worktree,
 *    `scripts.run`, run on asking (`workspace.run_script`) in a `Run` terminal,
 *    and `scripts.archive`, run before a worktree is removed: shell scripts up
 *    to `MAX_SCRIPT_BYTES`. They, and the worktree's agents and terminals,
 *    get `ZWIIM_PORT`, the first of ten ports of the worktree's own.
 * 2. `files.copy`: `.gitignore`-style patterns picking the Git-ignored files of
 *    the repository folder copied into each new worktree;
 *    `DEFAULT_FILES_TO_COPY` when unset, nothing when empty.
 * 3. `env`: variables every agent, terminal and script of the repository's
 *    worktrees gets. Names match `[A-Za-z_][A-Za-z0-9_]*` and don't start with
 *    `ZWIIM_`; values are one line. The Host keeps them in the OS keychain;
 *    updating fails with `keychain_unavailable` when it can't be used.
 * 4. `preview.urls`: where to open a worktree's servers (`workspace.previews`),
 *    up to `MAX_PREVIEW_URLS` `http(s)` URLs, each with an optional name. In a
 *    URL `$ZWIIM_PORT`, `${ZWIIM_PORT}` and `$((ZWIIM_PORT + n))` (n 0–9) become
 *    the worktree's ports; nothing else is expanded. When unset, the servers
 *    the worktree's terminals printed are opened.
 */
export type RepositorySettingKey =
  | "scripts.setup"
  | "scripts.run"
  | "scripts.archive"
  | "files.copy"
  | "env"
  | "preview.urls";

/** A repository script run in a terminal of its own, titled `Setup` / `Run`. */
export type RepositoryScript = "setup" | "run";

/** The title of the terminal a repository script runs in, by which clients know it. */
export const SCRIPT_TERMINAL_TITLES: Readonly<Record<RepositoryScript, string>> = { setup: "Setup", run: "Run" };

/** Longest repository script the Host accepts, in UTF-8 bytes. */
export const MAX_SCRIPT_BYTES = 16000;

/** The files a new worktree gets while `files.copy` is unset. */
export const DEFAULT_FILES_TO_COPY: readonly string[] = [".env*"];

/** Most `files.copy` patterns, and longest one in UTF-8 bytes. */
export const MAX_FILES_TO_COPY = 100;
export const MAX_FILE_PATTERN_BYTES = 256;

/** Longest `env` name, and most UTF-8 bytes of all names and values. */
export const MAX_ENV_NAME_BYTES = 128;
export const MAX_ENV_BYTES = 32000;

/** Most `preview.urls`, longest URL and longest name, in UTF-8 bytes. */
export const MAX_PREVIEW_URLS = 10;
export const MAX_PREVIEW_URL_BYTES = 2048;
export const MAX_PREVIEW_NAME_BYTES = 64;

/** A place to open a worktree's server: as set, or detected (no name). */
export interface Preview {
  name?: string | null;
  url: string;
}

export type RepositorySettingValue = string | string[] | Record<string, string> | Preview[] | null;

export interface RepositorySettings {
  values: {
    "scripts.setup"?: string | null;
    "scripts.run"?: string | null;
    "scripts.archive"?: string | null;
    "files.copy"?: string[] | null;
    env?: Record<string, string> | null;
    "preview.urls"?: Preview[] | null;
  } & Partial<Record<string, RepositorySettingValue>>;
}

/** Persisted session status. */
export type SessionStatus = "in_progress" | "pending_review" | "completed" | "cancelled";

/** Live agent status (snapshot `status`). */
export type AgentStatus = "connecting" | "connected" | "prompting" | "disconnected" | "error";

export interface SessionSummary {
  session_id: string;
  workspace_id: string | null;
  title: string | null;
  agent: AgentType | null;
  status: SessionStatus;
  live: boolean;
  created_at: string;
  updated_at: string;
  /**
   * Prompts queued behind the running turn, first to go out first; absent
   * when none are. Capability `session.follow_ups`.
   */
  follow_ups?: QueuedFollowUp[];
}

/** A prompt waiting for the session's running turn to end. */
export interface QueuedFollowUp {
  follow_up_id: string;
  /** What the user typed; the part an edit replaces. */
  text: string;
  /** How many images and files go with it. */
  attachments: number;
}

/**
 * `sent`: it went out as the next turn. `queued`: it waits in the queue.
 * `steered`: it went into the agent's running turn.
 */
export type FollowUpOutcome = "sent" | "queued" | "steered";

export type PromptBlock =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mime_type: string }
  /** A file uploaded with `attachments.upload` into the session's workspace. */
  | { type: "attachment"; attachment_id: string };

/**
 * A file attached to a user message. `path` is workspace-relative (open it with
 * `files.read` / `files.download`); `size` is absent when the file is gone.
 */
export interface AttachmentBlock {
  type: "attachment";
  attachment_id: string;
  name: string;
  mime_type: string;
  size?: number;
  path: string;
}

export type UserBlock =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mime_type: string }
  | AttachmentBlock;

/**
 * A tool call's progress, in the SDK's one spelling. The host may send ACP's
 * `in_progress` as `inprogress`; the SDK decodes both to `in_progress`.
 */
export type ToolCallStatus = "pending" | "in_progress" | "completed" | "failed" | (string & {});

/** An image an agent produced, as a file in the session's workspace. */
export interface AgentImage {
  name: string;
  mime_type: string;
  /** Workspace-relative; open or read it like any workspace file. */
  path: string;
}

export interface ToolCall {
  tool_call_id: string;
  title: string | null;
  kind?: string | null;
  status: ToolCallStatus | null;
  content: unknown;
  images?: AgentImage[];
  /**
   * The sub-agent tool call that made this call; absent on the main thread
   * (Host capability `agent.subagents`).
   */
  parent_id?: string;
  /**
   * Set when this call launches a sub-agent, with what is known of it so far
   * (`model`, `duration_ms`, `tokens` and `tool_uses` usually arrive with
   * the result). Read it
   * through `liveSubagentCards`.
   */
  subagent?: SubagentInfo;
}

/** A sub-agent as the tool call that launched it describes it. */
export interface SubagentInfo {
  /** The short task title the agent gave it. */
  description?: string;
  /** The model it ran on, by its plain id or the alias it was asked for. */
  model?: string;
  /** What it was asked to do. */
  prompt?: string;
  duration_ms?: number;
  /** Tokens it spent, once its result says (revision 7). */
  tokens?: number;
  /** Tool calls it made, once its result says (revision 7). */
  tool_uses?: number;
  /**
   * When it started, RFC 3339 UTC (revision 10): when the Host first saw the
   * launch, or on reload its transcript's first entry. Absent when unknown.
   */
  started_at?: string;
}

export interface PermissionOption {
  option_id: string;
  name: string;
  kind: string;
}

/** A proposed file change; paths are workspace-relative. */
export interface ToolDiff {
  path: string | null;
  old_text: string | null;
  new_text: string | null;
}

/** What a permission request would do. */
export interface PermissionTool {
  tool_call_id: string | null;
  kind: string | null;
  text: string | null;
  diffs: ToolDiff[];
  command: string | null;
}

export interface QuestionOption {
  label: string;
  description: string;
}

export interface QuestionSpec {
  question_id: string;
  header: string;
  question: string;
  multi_select: boolean;
  options: QuestionOption[];
}

export type AgentRequest =
  | {
      kind: "permission";
      request_id: string;
      title: string | null;
      tool: PermissionTool | null;
      /** A plan the agent asks to proceed with; its options are the agent's own. */
      plan_markdown: string | null;
      options: PermissionOption[];
    }
  | { kind: "question"; request_id: string; questions: QuestionSpec[] }
  | { kind: "plan_approval"; request_id: string; plan_markdown: string };

export type RequestKind = AgentRequest["kind"];

export type RequestAnswer =
  | { kind: "permission"; answer: { option_id: string } }
  | {
      kind: "question";
      answer: { answers: { question_id: string; labels: string[] }[]; declined?: boolean };
    }
  | {
      kind: "plan_approval";
      /** `hand_off` (Host capability `session.plan_hand_off`) continues the plan in a new session. */
      answer: { decision: "approve" | "request_changes" | "abandon" | "hand_off"; feedback?: string };
    };

export type LiveBlock =
  | { type: "text"; text: string }
  | { type: "thought"; text: string }
  | { type: "tool_call"; tool_call_id: string }
  | ({ type: "agent_image" } & AgentImage);

export interface LiveMessage {
  message_id: string | null;
  role: string;
  content: LiveBlock[];
  /**
   * Snapshots only: each sub-agent's own blocks, by the tool call that
   * launched it, in order of first appearance. Never part of `content`.
   * The session reducer moves them to `LiveTurn.subagents`.
   */
  subagents?: { parent_id: string; blocks: LiveBlock[] }[];
}

export interface PendingUserMessage {
  message_id: string;
  blocks: UserBlock[];
}

export interface SessionErrorInfo {
  message: string;
  code: string | number | null;
}

/**
 * The session's context window as the agent last reported it. A field the
 * agent did not report is null — never estimated.
 */
export interface Usage {
  /** Tokens in the context window. */
  used: number | null;
  /** The context window's size, in tokens. */
  size: number | null;
}

/** Token counts the agent reported for the last finished turn. */
export interface TurnUsage {
  total_tokens: number;
  input_tokens: number;
  output_tokens: number;
  /** Null: not reported by the agent. */
  thought_tokens: number | null;
  cached_read_tokens: number | null;
  cached_write_tokens: number | null;
}

/** One step of the agent's plan. */
export type StepStatus = "pending" | "in_progress" | "completed";

export interface ProgressStep {
  title: string;
  status: StepStatus;
}

/** The steps an agent is working through (`agent.progress`, revision 26). */
export interface Progress {
  steps: ProgressStep[];
  completed: number;
  total: number;
}

/** The newest plan a session saved, as `session.history` sends it (revision 26). */
export interface SavedPlan extends Progress {
  plan_id: string;
  agent: AgentType;
  /** How the plan's turn ended; `running` while it still runs. */
  state: "running" | "completed" | "interrupted" | "cancelled";
  /** A new session can finish it (`session.continue_plan`). */
  resumable: boolean;
}

/** The plan a session was started to finish, and where it came from (revision 26). */
export interface PlanContinues {
  session_id: string;
  plan_id: string;
  /** Absent from links saved before the Host recorded it. */
  agent?: AgentType | null;
  completed?: number;
  total?: number;
  title?: string | null;
}

export interface SessionSnapshot {
  status: AgentStatus;
  live_message: LiveMessage | null;
  tool_calls: ToolCall[];
  pending_request: AgentRequest | null;
  pending_user_message: PendingUserMessage | null;
  last_error: SessionErrorInfo | null;
  /** The agent is reconnecting after losing its server. Absent from Hosts before revision 14 (read as false). */
  reconnecting?: boolean;
  usage: Usage | null;
  /** Null until the agent reports a finished turn's tokens. */
  turn_usage?: TurnUsage | null;
  /** Null until the agent has advertised its options. */
  options: SessionOptions | null;
  /** Empty until the agent has announced its commands. */
  commands: SessionCommand[];
  /** Background work still running. Absent from Hosts before revision 5. */
  background?: SessionBackground;
  /**
   * Changes whenever the session's history does (a finished turn, a chat
   * rewind). Compare with `SessionHistory.history_generation` of the history
   * on screen: equal means that history is current. Absent from Hosts before
   * revision 5.
   */
  history_generation?: number;
  /** The turn's current steps; null when it has none. Absent from Hosts before revision 26. */
  progress?: Progress | null;
}

/** One task the agent launched to run in the background. */
export interface BackgroundTask {
  /** Opaque; stable for the task while it runs. */
  task_id: string;
  /** Human text for the task. */
  label: string;
  /** `running` | `paused`. */
  state: string;
  /** The tool call that launched it, when the agent linked them. */
  tool_call_id?: string;
  /**
   * A background sub-agent's tokens so far, read from its own transcript.
   * Absent until there is a reading, and from Hosts before revision 11
   * (capability `background.tokens`).
   */
  tokens?: number;
}

/** The session's background work. */
export interface SessionBackground {
  /** Tasks launched and not yet finished. */
  outstanding: number;
  /** What is known about each (may be fewer than `outstanding`). */
  tasks: BackgroundTask[];
  /**
   * RFC 3339: since when the agent has had nothing left to do but wait for
   * its background work. Null while a turn runs or nothing is outstanding.
   */
  waiting_since: string | null;
}

/** `session.background`: the background work, plus where history stands. */
export interface SessionBackgroundEvent extends SessionBackground {
  /**
   * The session's history generation when the event was sent. It moves when
   * turns land outside a turn (background work finishing after the turn
   * ended), which no `agent.turn_complete` announces. Absent from older Hosts.
   */
  history_generation?: number;
}

interface EventBase<T extends string, P> {
  session_id: string;
  seq: number;
  type: T;
  payload: P;
}

export type SessionEvent =
  | EventBase<"agent.message_delta", { text: string }>
  | EventBase<"agent.thought_delta", { text: string }>
  /** A sub-agent's own text and thinking, by the tool call that launched it. */
  | EventBase<"agent.subagent_message_delta", { parent_id: string; text: string }>
  | EventBase<"agent.subagent_thought_delta", { parent_id: string; text: string }>
  | EventBase<"agent.message_image", AgentImage>
  | EventBase<"agent.user_message", PendingUserMessage>
  /** A message the agent took into its running turn (revision 22). */
  | EventBase<"agent.steered", PendingUserMessage>
  | EventBase<"agent.tool_call", ToolCall>
  | EventBase<"agent.tool_call_update", Omit<ToolCall, "kind">>
  | EventBase<"agent.request", AgentRequest>
  | EventBase<"agent.request_resolved", { kind: RequestKind; request_id: string }>
  | EventBase<"agent.turn_complete", { stop_reason: string | null }>
  | EventBase<"agent.error", SessionErrorInfo>
  /** The agent's step list changed (revision 26). */
  | EventBase<"agent.progress", Progress>
  | EventBase<"session.status", { status: SessionStatus }>
  /** The agent started or stopped reconnecting after losing its server (revision 14). */
  | EventBase<"session.reconnecting", { reconnecting: boolean }>
  | EventBase<"session.options", { options: SessionOptions }>
  | EventBase<"session.commands", { commands: SessionCommand[] }>
  | EventBase<"session.usage", Usage>
  | EventBase<"session.turn_usage", TurnUsage>
  /** The background work changed (revision 5). */
  | EventBase<"session.background", SessionBackgroundEvent>;

export type HistoryBlock =
  | { type: "text"; text: string }
  | { type: "thought"; text: string }
  | { type: "image"; data: string; mime_type: string }
  | AttachmentBlock
  | ({ type: "agent_image"; revised_prompt?: string } & AgentImage)
  | {
      type: "image_generation";
      revised_prompt: string | null;
      image: { data: string; mime_type: string } | null;
    }
  | {
      type: "tool_call";
      tool_call_id: string | null;
      title: string;
      input_preview: string | null;
      status: ToolCallStatus | null;
      /**
       * Set when this call launched a sub-agent; `blocks` is everything the
       * sub-agent did, when its transcript was found.
       */
      subagent?: SubagentInfo & { blocks?: HistoryBlock[] };
    }
  | {
      type: "tool_result";
      tool_call_id: string | null;
      output_preview: string | null;
      is_error: boolean;
      images?: AgentImage[];
      /**
       * Revision 6: set when the call launched a sub-agent that ran in the
       * background. How the task ended, or `null` while the transcript does
       * not say (still running, or the session stopped first). Then
       * `output_preview` is the sub-agent's report.
       */
      background?: "completed" | "failed" | "stopped" | null;
    };

export interface HistoryTurn {
  turn_id: string;
  role: string;
  /**
   * User turns only: `session.revert` can put back the files this turn
   * started with. Absent on agent turns and from older Hosts.
   */
  revertible?: boolean;
  blocks: HistoryBlock[];
  timestamp: string;
  completed_at: string | null;
  duration_ms: number | null;
  model: string | null;
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_creation_input_tokens: number;
    cache_read_input_tokens: number;
  } | null;
}

export interface SessionRevertResult {
  files_restored: boolean;
  /**
   * The chat was taken back to before the turn. `false` when `chat` was not
   * asked for, the agent cannot rewind (see `SessionOptions.can_rewind_chat`)
   * or, with `files`, the rewind failed after the files were restored.
   */
  chat_rewound: boolean;
  /** The checked-out commit moved since the turn started; it was left as it is. */
  head_moved: boolean;
}

export interface SessionHistory {
  session_id: string;
  turns: HistoryTurn[];
  /**
   * See `SessionSnapshot.history_generation`; null when no agent is attached.
   * Absent from older Hosts.
   */
  history_generation?: number | null;
  /**
   * The can't-reach error saved on a session with no live agent. Null when
   * there is none. Absent from Hosts before revision 14.
   */
  last_error?: { message: string; code: "agent_unreachable" } | null;
  /** The newest saved plan. Absent from Hosts before revision 26 (capability `agent.progress`). */
  progress?: SavedPlan | null;
  /** The plan this session was started to finish. Absent before revision 26. */
  continues?: PlanContinues | null;
  /** The new session that took this session's plan over. Absent before revision 26. */
  continued_in?: { session_id: string } | null;
}

// ─── pull requests ───────────────────────────────────────────────────────

export type PrLifecycle = "open" | "draft" | "merged" | "closed";
export type CheckState = "passed" | "failed" | "pending" | "skipped";
export type MergeMethod = "merge" | "squash" | "rebase";
/** GitHub's `mergeStateStatus`. */
export type MergeStatus =
  | "clean"
  | "unstable"
  | "has_hooks"
  | "blocked"
  | "behind"
  | "dirty"
  | "draft"
  | "unknown";

export interface PrCheckItem {
  name: string;
  workflow: string | null;
  state: CheckState;
  /** The check's page on GitHub. */
  link: string | null;
}

export interface PrChecksSummary {
  passed: number;
  failed: number;
  pending: number;
  skipped: number;
  items: PrCheckItem[];
}

export interface PrMergeInfo {
  /** GitHub would merge it now. */
  allowed: boolean;
  status: MergeStatus;
  /** Why not, in words for the user; null when allowed or settled. */
  reason: string | null;
  /** The method a merge uses unless told otherwise. */
  method: MergeMethod | null;
  methods: MergeMethod[];
  /** GitHub merges it by itself once its requirements are met; null when off. Revision 22+. */
  auto?: PrAutoMerge | null;
  /** Auto-merge can be turned on: open and waiting only on required checks or review. Revision 22+. */
  auto_available?: boolean;
}

/** An auto-merge GitHub has queued for a pull request. */
export interface PrAutoMerge {
  /** How GitHub will merge it. */
  method: MergeMethod | null;
}

/** A workspace branch's pull request as the Host last saw it on GitHub. */
export interface PrState {
  number: number;
  url: string;
  title: string;
  state: PrLifecycle;
  base: string;
  head: string;
  /** The head commit a merge is pinned to. */
  head_sha: string;
  merged_at: string | null;
  checks: PrChecksSummary;
  merge: PrMergeInfo;
}

/** Why the Host cannot see pull requests: `gh_unavailable` | `gh_unauthenticated`. */
export interface PrError {
  code: string;
  message: string;
}

/** A workspace's pull request: `pr` null when its branch has none. */
export interface PrEntry {
  pr: PrState | null;
  error: PrError | null;
}

/**
 * Where a workspace's branch stands against its base (`origin/main`, or a
 * root's upstream). `conflicts`: merging the base in would conflict in
 * `files`. `in_progress`: a merge or rebase stopped on `files`; `operation`
 * is `unmerged` when entries are left with none running.
 */
export type ConflictCheck = { base: string; base_sha: string; head_sha: string } & (
  | { state: "clean" }
  | { state: "conflicts"; files: string[] }
  | { state: "in_progress"; operation: string; files: string[] }
);

/** How a resolve turn brings the base in. */
export type ResolveStrategy = "merge" | "rebase";

/** The conflicts a resolve turn was sent for. */
export interface ResolveStarted {
  base: string;
  files: string[];
}

export interface PrMerged {
  number: number;
  method: MergeMethod;
  merged_at: string | null;
}

/** `on`: GitHub merges it once its requirements are met. `merged`: it was mergeable, so it was merged now. */
export type AutoMergeOutcome = "on" | "merged" | "off";

export interface PrAutoMerged {
  number: number;
  outcome: AutoMergeOutcome;
  /** The method GitHub merges, or merged, with; null when off. */
  method: MergeMethod | null;
}

// ─── host state feed ─────────────────────────────────────────────────────

/**
 * Host-wide state changes, in feed order. Upserts carry the whole object;
 * `workspace.diff_changed` only says the workspace's diff may differ now.
 */
export type HostFeedEvent = { seq: number } & (
  | { type: "workspace.changed"; payload: { workspace: WorkspaceInfo } }
  | { type: "workspace.removed"; payload: { workspace_id: string } }
  | { type: "workspace.diff_changed"; payload: { workspace_id: string } }
  | { type: "session.changed"; payload: { session: SessionSummary } }
  | { type: "session.removed"; payload: { session_id: string; workspace_id: string } }
  | { type: "terminal.changed"; payload: { terminal: TerminalInfo } }
  | { type: "terminal.removed"; payload: { terminal_id: string } }
  | { type: "workspace.pr_changed"; payload: { workspace_id: string } & PrEntry }
  | { type: "workspace.conflicts_changed"; payload: { workspace_id: string; conflicts: ConflictCheck | null } }
  | { type: "workspace.resolving_changed"; payload: { workspace_id: string; session_id: string | null } }
  | { type: "clone.changed"; payload: { clone: CloneJob } }
  | { type: "clone.removed"; payload: { clone_id: string } }
  | { type: "host.keep_awake_changed"; payload: KeepAwakeStatus }
);

/** The feed's baseline: what `workspace.list`, `session.list` and `terminal.list` return. */
export interface HostSnapshot {
  /** The host process generation; a new one means start over. */
  epoch: string;
  /** The last event already reflected here; the next event is `seq + 1`. */
  seq: number;
  workspaces: WorkspaceInfo[];
  sessions: SessionSummary[];
  terminals: TerminalInfo[];
  /** Workspaces whose branch has a pull request (or whose PR lookup failed). */
  prs?: Record<string, PrEntry>;
  /** Workspaces with a base to compare with, and where they stand (older Hosts omit it). */
  conflicts?: Record<string, ConflictCheck>;
  /** Workspaces whose conflicts a session's turn is resolving, to that session (older Hosts omit it). */
  resolving?: Record<string, string>;
  /** GitHub clones in flight or recently finished (older Hosts omit it). */
  clones?: CloneJob[];
  /** What keeping the Mac awake is doing; null until the host first looked (older Hosts omit it). */
  keep_awake?: KeepAwakeStatus | null;
}

// ─── event socket ────────────────────────────────────────────────────────

export type ClientMessage =
  | {
      op: "session.subscribe";
      subscription_id: string;
      session_id: string;
      since_seq?: number;
      epoch?: string;
    }
  | { op: "session.detach"; subscription_id: string }
  | { op: "terminal.subscribe"; terminal_id: string }
  | { op: "terminal.unsubscribe"; terminal_id: string }
  | { op: "host.subscribe" }
  | { op: "ping" };

export type DetachReason = "session_not_live" | "lagged" | "server_shutdown";

export type ServerFrame =
  | {
      type: "snapshot";
      subscription_id: string;
      session_id: string;
      epoch: string;
      snapshot: SessionSnapshot;
      event_seq: number;
    }
  | {
      type: "replay";
      subscription_id: string;
      session_id: string;
      epoch: string;
      events: SessionEvent[];
      high_water_seq: number;
    }
  | { type: "event"; subscription_id: string; event: SessionEvent }
  | { type: "detached"; subscription_id: string; reason: DetachReason }
  | { type: "terminal"; event: TerminalEvent }
  | ({ type: "host.snapshot" } & HostSnapshot)
  | { type: "host.event"; epoch: string; event: HostFeedEvent }
  | { type: "pong" }
  | { type: "error"; subscription_id: string | null; error: WireError };

// ─── host updates ────────────────────────────────────────────────────────

/** Why a Host cannot update itself. */
export type HostUpdateUnsupportedReason =
  | "unsigned_build"
  | "invalid_release_key"
  | "unsupported_platform"
  | "unsupervised"
  | (string & {});

export type HostUpdateActivity =
  | { state: "idle" }
  | { state: "checking" }
  | { state: "downloading"; version: string }
  | { state: "failed"; reason: string; message: string };

/** How the last update ended. */
export interface HostUpdateResult {
  outcome: "applied" | "rolled_back";
  version: string;
  from: string;
  reason: string | null;
  at: string;
}

/** `host.update.status`: the Host's own account of its updates. */
export interface HostUpdateStatus {
  version: string;
  channel: "stable" | "beta";
  supported: boolean;
  unsupported_reason: HostUpdateUnsupportedReason | null;
  activity: HostUpdateActivity;
  /** A newer verified release. */
  available: { version: string; released_at: string; notes: string } | null;
  /** Verified and installed next to the running version; a restart applies it. */
  staged: { version: string; source: "download" | "bundle" } | null;
  restart_required: boolean;
  /** A restart was asked for and waits until the Host is idle. Absent from older Hosts. */
  restart_pending?: boolean;
  /** What keeps the Host from restarting now. Absent from Hosts before revision 27. */
  blockers?: HostUpdateApply["blockers"];
  /** The version on trial after an update, until it proves healthy. */
  trial: { version: string; from: string } | null;
  /** The version a failed update returns to. */
  previous: string | null;
  last_result: HostUpdateResult | null;
  last_checked: string | null;
  platform_signature: "enforced" | "not_enforced";
  /** The Host came with the Desktop app, whose update brings the next one. */
  bundled_with_desktop: boolean;
}

export type HostRestartMode = "when_idle" | "now";

/** The two texts a user can give their agents at one scope; null when unset. */
export interface InstructionTexts {
  /** Added to every session's instructions. */
  general: string | null;
  /** Added to the turn that asks an agent to resolve conflicts. */
  resolve_conflicts: string | null;
}

/** `host.instructions`: Host-wide texts, and texts per root workspace keyed by `workspace_id`. */
export interface UserInstructions {
  host: InstructionTexts;
  workspaces: Record<string, InstructionTexts>;
}

/** `host.instructions.set`: replaces both texts at one scope; absent or blank clears. At most 8 KiB each. */
export interface InstructionsChange {
  scope: "host" | "workspace";
  /** Required for `workspace`, and must name a root workspace; absent for `host`. */
  workspace_id?: string;
  general?: string | null;
  resolve_conflicts?: string | null;
}

/** `host.diagnostics`: facts for support. Never credentials, keys or paths. */
export interface HostDiagnostics {
  version: string;
  target: string | null;
  channel: "stable" | "beta";
  protocol: { major: number; revision: number; min_client_revision: number; capabilities: string[] };
  release_signing: { release_key: boolean | null; platform_signature: "enforced" | "not_enforced" };
  adapters: Record<
    string,
    {
      version: string | null;
      locked_version: string | null;
      installed: boolean;
      healthy: boolean | null;
      authenticated: boolean;
      setup: AdapterSetup;
      source: string | null;
    }
  >;
  update: Pick<
    HostUpdateStatus,
    | "supported"
    | "unsupported_reason"
    | "activity"
    | "staged"
    | "trial"
    | "previous"
    | "last_result"
    | "last_checked"
    | "bundled_with_desktop"
  > & { available: string | null };
  database: {
    schema: { state: "fresh" | "current" | "pending" | "newer"; migrations?: string[] };
    migrations_known: number;
    applied_at_start: number;
    snapshot_at_start: boolean;
    snapshots: number;
  };
}

/** `host.update.apply`: the switch happens on the restart it starts or schedules. */
export interface HostUpdateApply {
  version: string;
  state: "restarting" | "restart_pending";
  blockers: {
    live_turns: number;
    pending_requests: number;
    terminals: number;
    /** Sessions starting a prompt. Absent before revision 27. */
    starting?: number;
    session_ids: string[];
  };
}

/** How an agent is signed in on the Host. */
export type AgentAuthMethod = "account" | "api_key" | "cloud";

/** `agent.auth_status`: an agent's sign-in on the Host. Never a key or a path. */
export interface AgentAuthStatus {
  agent: AgentType;
  connected: boolean;
  /** `null` when not signed in. */
  method: AgentAuthMethod | null;
  /** An API key is saved on the Host; unused while an account is signed in. */
  saved_key: boolean;
  /** `running` while the provider's sign-in page is open on the Host's Mac. */
  sign_in: "idle" | "running" | "failed";
  /** The signed-in account's, when the method is `account`. */
  email: string | null;
  plan: string | null;
}

/** How the Host's `gh` is signed in to GitHub. `unavailable`: gh is not installed. */
export type GithubAuthState = "signed_out" | "signing_in" | "signed_in" | "unavailable";

/** `github.auth_status`: the Host's GitHub sign-in through gh's device flow. Never a token. */
export interface GithubAuth {
  state: GithubAuthState;
  /** The signed-in account, when `signed_in`. */
  login?: string;
  /** The one-time code to enter, while `signing_in`. */
  code?: string;
  /** Where to enter it (always `https://github.com/...`), while `signing_in`. */
  url?: string;
  /** Why the last attempt failed, reported once. */
  error?: string;
}

// ─── operations ──────────────────────────────────────────────────────────

/** Request/response shape of every V1 operation the SDK exposes. */
export interface Operations {
  "host.hello": [HostHelloParams, HostHello];
  /** `null` while the host's LAN access is off. */
  "host.lan_endpoint": [Record<string, never>, { endpoint: HostLanEndpoint | null }];
  "host.claim": [{ user_id: string; email?: string }, { owner: HostOwner }];
  "host.update.status": [Record<string, never>, HostUpdateStatus];
  /** Look for a newer release now; a Host that can update also downloads and stages it. */
  "host.update.check": [Record<string, never>, HostUpdateStatus];
  /** `update_unavailable` when nothing is staged or the Host cannot switch versions. */
  "host.update.apply": [{ mode?: HostRestartMode }, HostUpdateApply];
  /**
   * Which releases the Host follows from its next check; beta takes stable
   * releases too. Local host authority only. Idempotent. Offered with the
   * `host.update.channel` capability.
   */
  "host.update.set_channel": [{ channel: HostUpdateStatus["channel"] }, HostUpdateStatus];
  "host.diagnostics": [Record<string, never>, HostDiagnostics];
  /** Where the Host keeps its data, worktrees and clones. Capability `host.folders`. */
  "host.folders": [Record<string, never>, HostFolders];
  "agent.list": [Record<string, never>, AgentSummary[]];
  /** Install or repair a host-managed adapter in the background (local authority). */
  "agent.setup": [{ agent: AgentType }, AdapterStatus];
  /** Claude Code's and Codex's sign-ins on the Host, read fresh. */
  "agent.auth_status": [Record<string, never>, AgentAuthStatus[]];
  /** Open the provider's sign-in page on the Host's Mac; asking again starts over (local authority). */
  "agent.sign_in": [{ agent: AgentType }, AgentAuthStatus];
  /** Keep an API key in the Host's Keychain; never returned (local authority). */
  "agent.save_key": [{ agent: AgentType; key: string }, AgentAuthStatus];
  "agent.remove_key": [{ agent: AgentType }, AgentAuthStatus];
  /** Delete the saved key, sign the account out, end the agent's running sessions (local authority). */
  "agent.sign_out": [{ agent: AgentType }, AgentAuthStatus];
  "workspace.list": [Record<string, never>, WorkspaceInfo[]];
  /** `path` needs local host authority; paired devices use `home_path` (folder names under home). */
  "workspace.add": [
    ({ path: string } | { home_path: string[] } | { workspace_id: string }) & {
      allow_large?: boolean;
      publish?: PublishTarget;
    },
    WorkspaceInfo,
  ];
  /** What a folder is before adding it: Git state, first-commit size, GitHub owners. Touches nothing. */
  "workspace.inspect": [{ path: string } | { home_path: string[] } | { workspace_id: string }, WorkspaceInspection];
  /** Whether `owner/name` is free on GitHub for the Host's signed-in account. */
  "github.repo_available": [PublishTarget, { available: boolean }];
  /** One page (from 1) of the Host account's GitHub repositories. */
  "github.repos": [{ page?: number; refresh?: boolean }, GithubRepoPage];
  /** How the Host's gh is signed in to GitHub; the code while a sign-in runs. Capability `github.auth`. */
  "github.auth_status": [Record<string, never>, GithubAuth];
  /** Start signing the Host's gh in (or return the sign-in running); waits briefly for the code. */
  "github.sign_in": [Record<string, never>, GithubAuth];
  /** End the sign-in in progress, if any. */
  "github.cancel_sign_in": [Record<string, never>, GithubAuth];
  /** Clone `owner/name` into a new workspace, or the root already opened on it. */
  "workspace.clone": [PublishTarget, CloneStart];
  "workspace.clone_cancel": [{ clone_id: string }, { cancelled: boolean }];
  "workspace.browse": [{ path: string[] }, BrowseListing];
  /** Make one folder `name` inside the folder `path` (folder names under home); `already_exists` if taken. */
  "workspace.create_folder": [{ path: string[]; name: string }, BrowseEntry];
  "workspace.open": [{ workspace_id: string }, WorkspaceDetail];
  "workspace.tree": [
    { workspace_id: string; path?: string; max_depth?: number },
    FileTreeNode[],
  ];
  "files.read": [{ workspace_id: string; path: string }, FileContent];
  /**
   * Replace a text file's content (Hosts with `files.write`). With `etag` from
   * the last read, a file changed since is refused with `file_changed`.
   */
  "files.write": [{ workspace_id: string; path: string; content: string; etag?: string }, FileSaveResult];
  /**
   * A workspace file as base64. Whole (up to 10 MB, `file_too_large` past it),
   * or with `offset`/`length` a range of at most 8 MiB (Hosts with
   * `file_ranges`). Over the Cloud relay files past 10 MB are refused with
   * `file_too_large_for_remote`.
   */
  "files.download": [{ workspace_id: string; path: string; offset?: number; length?: number }, FileDownload];
  /** Store a file (base64, up to 10 MB) in the workspace for an `attachment` prompt block. */
  "attachments.upload": [{ workspace_id: string; name: string; data: string }, UploadedAttachment];
  "git.status": [{ workspace_id: string; show_all_untracked?: boolean }, GitStatusEntry[]];
  "git.log": [{ workspace_id: string; limit?: number; skip?: number }, GitLogResult];
  "git.commit": [
    { workspace_id: string; message: string; files: string[] },
    { committed_files: number },
  ];
  "git.diff": [{ workspace_id: string; file?: string }, WorkspaceDiff];
  "git.branches": [{ workspace_id: string }, WorktreeBases];
  /** The Host's view of the branch's pull request; `refresh` asks GitHub now. */
  "git.pr": [{ workspace_id: string; refresh?: boolean }, PrEntry];
  /**
   * Merge now, pinned to `head_sha`: `pr_changed` if the PR moved,
   * `not_mergeable` (details.reason) if GitHub would not merge it now.
   */
  "git.pr_merge": [
    { workspace_id: string; number: number; head_sha: string; method?: MergeMethod },
    PrMerged,
  ];
  /**
   * Turn GitHub's auto-merge on (pinned to `head_sha`) or off. `merged` when
   * GitHub would merge it now; `not_mergeable` when behind, conflicted or a
   * draft; `auto_merge_disabled` when the repository does not allow it.
   * Revision 22+ (`git.pr_auto_merge`).
   */
  "git.pr_auto_merge": [
    { workspace_id: string; number: number; enabled: boolean; head_sha?: string; method?: MergeMethod },
    PrAutoMerged,
  ];
  /** `base`: a branch from `git.branches`; the root's HEAD when absent. */
  "workspace.create_worktree": [{ workspace_id: string; base?: string }, WorkspaceInfo];
  "workspace.remove_worktree": [{ workspace_id: string; force?: boolean }, WorktreeRemoved];
  "workspace.remove": [{ workspace_id: string; force?: boolean }, WorkspaceRemoved];
  /**
   * After its pull request `number` merged, move the worktree in place to a
   * new branch from the PR's base (capability `workspace.continue`).
   */
  "workspace.continue": [{ workspace_id: string; number: number }, WorkspaceInfo];
  /**
   * Send session `session_id` the Host's own turn asking its agent to resolve
   * the worktree's conflicts with its base (capability
   * `workspace.resolve_conflicts`). Answers with the conflicts it was sent for.
   */
  "workspace.resolve_conflicts": [
    { workspace_id: string; session_id: string; strategy?: ResolveStrategy },
    ResolveStarted,
  ];
  /**
   * `agent` runs that agent's CLI in place of a shell (Claude Code and Codex;
   * capability `terminal.agent`). `command`, a slash command, runs in it;
   * one the CLI does not show back (it is asking something first) is not
   * entered.
   */
  "terminal.spawn": [
    { workspace_id: string; shell?: string; agent?: AgentType; command?: string },
    { terminal_id: string },
  ];
  "terminal.write": [{ terminal_id: string; data: string }, Record<string, never>];
  "terminal.resize": [{ terminal_id: string; cols: number; rows: number }, Record<string, never>];
  "terminal.kill": [{ terminal_id: string }, Record<string, never>];
  "terminal.list": [Record<string, never>, TerminalInfo[]];
  "terminal.snapshot": [{ terminal_id: string }, TerminalSnapshot];
  "session.start": [
    {
      workspace_id: string;
      agent: AgentType;
      blocks: PromptBlock[];
      client_message_id?: string;
      options?: OptionsRequest;
    },
    { session_id: string },
  ];
  "session.send": [
    { session_id: string; blocks: PromptBlock[]; client_message_id?: string },
    { session_id: string },
  ];
  "session.cancel": [{ session_id: string }, Record<string, never>];
  "session.stop": [{ session_id: string }, Record<string, never>];
  "session.delete": [{ session_id: string }, Record<string, never>];
  "session.options": [{ session_id: string }, { session_id: string; options: SessionOptions }];
  "session.set_options": [
    { session_id: string } & OptionsRequest,
    { session_id: string; options: SessionOptions },
  ];
  "session.respond": [
    { session_id: string; request_id: string } & RequestAnswer,
    /** `session_id`: the new session a plan was handed off to. */
    { session_id?: string },
  ];
  "session.list": [{ workspace_id: string }, SessionSummary[]];
  "session.history": [{ session_id: string }, SessionHistory];
  /**
   * Run the turn that could not reach its server again in the same chat.
   * Capability `session.retry`. `nothing_to_retry` when no such turn is saved.
   */
  "session.retry": [{ session_id: string }, { session_id: string }];
  /**
   * Retry in a new chat that starts with a summary of the earlier one.
   * `session_id` is the NEW session. Capability `session.retry`.
   */
  "session.retry_in_new_chat": [{ session_id: string }, { session_id: string }];
  /**
   * Start a new session, with `agent` (the session's own when absent), that
   * finishes this session's newest plan. `session_id` is the NEW session.
   * Capability `session.continue_plan`. `nothing_to_continue` when there is
   * no plan to carry on.
   */
  "session.continue_plan": [{ session_id: string; agent?: AgentType }, { session_id: string; plan_id: string }];
  /**
   * Send now when the agent is idle and nothing is queued, else queue behind
   * what is. The Host sends the next one when a turn ends normally; a stopped
   * or failed turn holds the queue. Capability `session.follow_ups`.
   *
   * `steer` puts it into the running turn instead when the agent can take a
   * message mid-turn; otherwise it goes the usual way. Capability
   * `session.steer`.
   */
  "session.follow_up": [
    { session_id: string; blocks: PromptBlock[]; client_message_id?: string; steer?: boolean },
    { outcome: FollowUpOutcome },
  ];
  /** Replace a queued follow-up's text. `not_found` once it has gone out. */
  "session.follow_up_edit": [{ session_id: string; follow_up_id: string; text: string }, Record<string, never>];
  /** Drop a queued follow-up; one already gone is not an error. */
  "session.follow_up_remove": [{ session_id: string; follow_up_id: string }, Record<string, never>];
  /** Send a queued follow-up now when idle (resuming a held queue), else move it first. */
  "session.follow_up_send": [{ session_id: string; follow_up_id: string }, { outcome: FollowUpOutcome }];
  /** The Host's settings. Capability `settings`. */
  "settings.get": [Record<string, never>, HostSettings];
  /**
   * Set settings (a value) or clear them (null); others are left alone.
   * `invalid_input` for an unknown key or a value it cannot hold, and then
   * nothing is written. Capability `settings`.
   */
  "settings.update": [HostSettings, HostSettings];
  /**
   * The settings of a workspace's repository: its own for a repository, the
   * one it was made from for a worktree. Capability `repository.settings`.
   */
  "repository.settings.get": [{ workspace_id: string }, RepositorySettings];
  /**
   * Set (a value) or clear (null) settings of a workspace's repository, as
   * `settings.update` does. Capability `repository.settings`.
   */
  "repository.settings.update": [{ workspace_id: string } & RepositorySettings, RepositorySettings];
  /**
   * Run a worktree's repository script now, in a new terminal titled `Setup`
   * or `Run` that replaces its live one of that title. `script_not_set` when
   * the repository has none; `root_workspace` in a repository folder.
   * Capability `workspace.run_script`.
   */
  "workspace.run_script": [{ workspace_id: string; script: RepositoryScript }, { terminal_id: string }];
  /**
   * Where to open a worktree's servers, the default first: the repository's
   * `preview.urls` with the worktree's ports in them, else the servers its
   * terminals printed that still answer, newest first. URLs are the Host's
   * own (`localhost`), for a client on its machine. `root_workspace` in a
   * repository folder. Capability `workspace.previews`.
   */
  "workspace.previews": [{ workspace_id: string }, { previews: Preview[] }];
  /**
   * Take a session back to before user turn `turn_id`. `files` restores the
   * workspace files and staged changes the turn started with (HEAD and
   * ignored files are never touched). `chat` takes the conversation back to
   * the agent reply before the turn, keeping the `session_id` (capability
   * `session.revert.chat`; a first turn has no reply before it and is
   * `checkpoint_unavailable`). With both, files are restored first. Capability
   * `session.revert`.
   */
  "session.revert": [
    { session_id: string; turn_id: string; files: boolean; chat: boolean },
    SessionRevertResult,
  ];
  /** Local host authority only, like `device.list` / `device.revoke` and `host.claim`. */
  "device.pair_invite": [Record<string, never>, KeyPairingInvitation];
  /** Unauthenticated: the one-time `code` authorizes it. */
  "device.pair": [
    { code: string; device_id: string; device_name?: string; device_pubkey: string },
    { host_id: string; device_id: string },
  ];
  "device.list": [Record<string, never>, { devices: DeviceInfo[] }];
  "device.revoke": [{ device_id: string }, Record<string, never>];
  /** Local host authority only: enroll this host with a Zwiim Cloud. */
  "cloud.enroll": [
    { cloud_url: string; enrollment_token: string; name?: string },
    CloudStatus,
  ];
  /** Local host authority only. */
  "cloud.status": [Record<string, never>, CloudStatus];
  /** Local host authority only. */
  "lan.status": [Record<string, never>, LanStatus];
  /** Local host authority only. Idempotent: sets, never toggles. */
  "lan.set": [{ enabled: boolean }, LanStatus];
  /** Local host authority only. */
  "host.keep_awake.status": [Record<string, never>, KeepAwakeStatus];
  /** Local host authority only. Idempotent: sets, never toggles. */
  "host.keep_awake.set": [Partial<{ mode: KeepAwakeMode; on_battery: boolean; lid_closed: boolean }>, KeepAwakeStatus];
  /** The user's instructions for their agents. Applies to new sessions, and to open ones after a few idle minutes. */
  "host.instructions": [Record<string, never>, UserInstructions];
  /** Idempotent: sets one scope's texts, never appends. */
  "host.instructions.set": [InstructionsChange, UserInstructions];
}

export type OpName = keyof Operations;
export type OpParams<O extends OpName> = Operations[O][0];
export type OpResult<O extends OpName> = Operations[O][1];

/**
 * Operations whose effect is not safe to repeat. A dropped connection during
 * one of these leaves the outcome unknown; the SDK never retries them.
 */
export const MUTATING_OPS: ReadonlySet<OpName> = new Set<OpName>([
  "host.claim",
  "host.update.apply",
  "git.commit",
  "git.pr_merge",
  "git.pr_auto_merge",
  "session.set_options",
  "workspace.add",
  "workspace.create_folder",
  "workspace.create_worktree",
  "attachments.upload",
  "files.write",
  "workspace.remove_worktree",
  "workspace.remove",
  "workspace.continue",
  "workspace.resolve_conflicts",
  "terminal.spawn",
  "workspace.run_script",
  "terminal.write",
  "terminal.resize",
  "terminal.kill",
  "session.start",
  "session.send",
  "session.cancel",
  "session.stop",
  "session.delete",
  "session.revert",
  "session.retry",
  "session.retry_in_new_chat",
  "session.continue_plan",
  "session.follow_up",
  "session.follow_up_send",
  "session.respond",
  "device.pair",
  "device.revoke",
  "cloud.enroll",
  "workspace.clone",
  "workspace.clone_cancel",
  "github.sign_in",
  "github.cancel_sign_in",
]);
