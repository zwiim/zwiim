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
  | "cloud_unavailable"
  | "host_restarting"
  | "update_unavailable"
  | "not_mergeable"
  | "pr_changed"
  /** `workspace.continue`: the pull request is not merged on GitHub. */
  | "pr_not_merged"
  /** `workspace.continue`: the branch moved past the merged head, so Continue would strand commits. */
  | "head_not_merged"
  | "gh_unavailable"
  | "gh_unauthenticated"
  /** GitHub could not be reached from the Host (not this client's connection). */
  | "github_unreachable"
  /** `session.start` / `session.send` / `terminal.spawn` in a root: sessions run in a worktree. */
  | "root_workspace"
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

/**
 * Whether the host's Mac stays awake while an agent works. On by default;
 * lets go on battery below 10%.
 */
export interface KeepAwakeStatus {
  /** False where the host cannot keep the computer awake (not a Mac). */
  supported: boolean;
  enabled: boolean;
  /** The Mac is being kept awake right now. */
  active: boolean;
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
}

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

export interface SessionSnapshot {
  status: AgentStatus;
  live_message: LiveMessage | null;
  tool_calls: ToolCall[];
  pending_request: AgentRequest | null;
  pending_user_message: PendingUserMessage | null;
  last_error: SessionErrorInfo | null;
  usage: Usage | null;
  /** Null until the agent reports a finished turn's tokens. */
  turn_usage?: TurnUsage | null;
  /** Null until the agent has advertised its options. */
  options: SessionOptions | null;
  /** Empty until the agent has announced its commands. */
  commands: SessionCommand[];
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
  | EventBase<"agent.message_image", AgentImage>
  | EventBase<"agent.user_message", PendingUserMessage>
  | EventBase<"agent.tool_call", ToolCall>
  | EventBase<"agent.tool_call_update", Omit<ToolCall, "kind">>
  | EventBase<"agent.request", AgentRequest>
  | EventBase<"agent.request_resolved", { kind: RequestKind; request_id: string }>
  | EventBase<"agent.turn_complete", { stop_reason: string | null }>
  | EventBase<"agent.error", SessionErrorInfo>
  | EventBase<"session.status", { status: SessionStatus }>
  | EventBase<"session.options", { options: SessionOptions }>
  | EventBase<"session.commands", { commands: SessionCommand[] }>
  | EventBase<"session.usage", Usage>
  | EventBase<"session.turn_usage", TurnUsage>;

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
    }
  | {
      type: "tool_result";
      tool_call_id: string | null;
      output_preview: string | null;
      is_error: boolean;
      images?: AgentImage[];
    };

export interface HistoryTurn {
  turn_id: string;
  role: string;
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

export interface SessionHistory {
  session_id: string;
  turns: HistoryTurn[];
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

export interface PrMerged {
  number: number;
  method: MergeMethod;
  merged_at: string | null;
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
  | { type: "clone.changed"; payload: { clone: CloneJob } }
  | { type: "clone.removed"; payload: { clone_id: string } }
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
  /** GitHub clones in flight or recently finished (older Hosts omit it). */
  clones?: CloneJob[];
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
  /** `base`: a branch from `git.branches`; the root's HEAD when absent. */
  "workspace.create_worktree": [{ workspace_id: string; base?: string }, WorkspaceInfo];
  "workspace.remove_worktree": [{ workspace_id: string; force?: boolean }, WorktreeRemoved];
  /**
   * After its pull request `number` merged, move the worktree in place to a
   * new branch from the PR's base (capability `workspace.continue`).
   */
  "workspace.continue": [{ workspace_id: string; number: number }, WorkspaceInfo];
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
  "host.keep_awake.set": [{ enabled: boolean }, KeepAwakeStatus];
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
  "session.set_options",
  "workspace.add",
  "workspace.create_folder",
  "workspace.create_worktree",
  "attachments.upload",
  "files.write",
  "workspace.remove_worktree",
  "workspace.continue",
  "terminal.spawn",
  "terminal.write",
  "terminal.resize",
  "terminal.kill",
  "session.start",
  "session.send",
  "session.cancel",
  "session.stop",
  "session.delete",
  "session.respond",
  "device.pair",
  "device.revoke",
  "cloud.enroll",
  "workspace.clone",
  "workspace.clone_cancel",
]);
