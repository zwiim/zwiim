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
  | "workspace_removed"
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
}

/** `workspace_dirty` details: uncommitted entries a forced removal discards. */
export interface WorkspaceDirtyDetails {
  changes: number;
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

// ─── host ────────────────────────────────────────────────────────────────

/** The account a host belongs to. Never a token. */
export interface HostOwner {
  user_id: string;
  email: string | null;
}

export interface HostHello {
  host_id: string;
  protocol: string;
  version: string;
  /** Null until an account claims the host. */
  owner: HostOwner | null;
}

// ─── workspace / files / git ─────────────────────────────────────────────

export type WorkspaceKind = "root" | "worktree" | (string & {});

export interface WorkspaceInfo {
  workspace_id: string;
  name: string;
  alias: string | null;
  kind: WorkspaceKind;
  parent_workspace_id: string | null;
  /** Checked-out branch, read live; null when detached or removed. */
  branch: string | null;
  /** Worktrees only: the root branch and commit it was created from. */
  base_branch: string | null;
  base_commit: string | null;
  /** A removed worktree, kept listed while it still holds sessions. */
  removed: boolean;
}

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

export type AgentUnavailableReason = "not_installed" | "unsupported_platform" | "disabled";

export interface AgentSummary {
  agent: AgentType;
  name: string;
  /** Whether a session can be started with this agent on this host. */
  available: boolean;
  reason: AgentUnavailableReason | (string & {}) | null;
  /** What a new session can choose; null before the agent's first session here. */
  options: SessionOptions | null;
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
  | { type: "image"; data: string; mime_type: string };

export type UserBlock = PromptBlock;

export interface ToolCall {
  tool_call_id: string;
  title: string | null;
  kind?: string | null;
  status: string | null;
  content: unknown;
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
      answer: { decision: "approve" | "request_changes" | "abandon"; feedback?: string };
    };

export type LiveBlock =
  | { type: "text"; text: string }
  | { type: "thought"; text: string }
  | { type: "tool_call"; tool_call_id: string };

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

export interface Usage {
  used: number;
  size: number;
}

export interface SessionSnapshot {
  status: AgentStatus;
  live_message: LiveMessage | null;
  tool_calls: ToolCall[];
  pending_request: AgentRequest | null;
  pending_user_message: PendingUserMessage | null;
  last_error: SessionErrorInfo | null;
  usage: Usage | null;
  /** Null until the agent has advertised its options. */
  options: SessionOptions | null;
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
  | EventBase<"agent.user_message", PendingUserMessage>
  | EventBase<"agent.tool_call", ToolCall>
  | EventBase<"agent.tool_call_update", Omit<ToolCall, "kind">>
  | EventBase<"agent.request", AgentRequest>
  | EventBase<"agent.request_resolved", { kind: RequestKind; request_id: string }>
  | EventBase<"agent.turn_complete", { stop_reason: string | null }>
  | EventBase<"agent.error", SessionErrorInfo>
  | EventBase<"session.status", { status: SessionStatus }>
  | EventBase<"session.options", { options: SessionOptions }>;

export type HistoryBlock =
  | { type: "text"; text: string }
  | { type: "thought"; text: string }
  | { type: "image"; data: string; mime_type: string }
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
      status: string | null;
    }
  | {
      type: "tool_result";
      tool_call_id: string | null;
      output_preview: string | null;
      is_error: boolean;
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

// ─── operations ──────────────────────────────────────────────────────────

/** Request/response shape of every V1 operation the SDK exposes. */
export interface Operations {
  "host.hello": [Record<string, never>, HostHello];
  "host.claim": [{ user_id: string; email?: string }, { owner: HostOwner }];
  "agent.list": [Record<string, never>, AgentSummary[]];
  "workspace.list": [Record<string, never>, WorkspaceInfo[]];
  "workspace.add": [{ path: string }, WorkspaceInfo];
  "workspace.open": [{ workspace_id: string }, WorkspaceDetail];
  "workspace.tree": [
    { workspace_id: string; path?: string; max_depth?: number },
    FileTreeNode[],
  ];
  "files.read": [{ workspace_id: string; path: string }, FileContent];
  "git.status": [{ workspace_id: string; show_all_untracked?: boolean }, GitStatusEntry[]];
  "git.log": [{ workspace_id: string; limit?: number; skip?: number }, GitLogResult];
  "git.commit": [
    { workspace_id: string; message: string; files: string[] },
    { committed_files: number },
  ];
  "git.diff": [{ workspace_id: string; file?: string }, WorkspaceDiff];
  "workspace.create_worktree": [{ workspace_id: string }, WorkspaceInfo];
  "workspace.remove_worktree": [{ workspace_id: string; force?: boolean }, WorktreeRemoved];
  "terminal.spawn": [{ workspace_id: string; shell?: string }, { terminal_id: string }];
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
  "session.options": [{ session_id: string }, { session_id: string; options: SessionOptions }];
  "session.set_options": [
    { session_id: string } & OptionsRequest,
    { session_id: string; options: SessionOptions },
  ];
  "session.respond": [
    { session_id: string; request_id: string } & RequestAnswer,
    Record<string, never>,
  ];
  "session.list": [{ workspace_id: string }, SessionSummary[]];
  "session.history": [{ session_id: string }, SessionHistory];
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
  "git.commit",
  "session.set_options",
  "workspace.add",
  "workspace.create_worktree",
  "workspace.remove_worktree",
  "terminal.spawn",
  "terminal.write",
  "terminal.resize",
  "terminal.kill",
  "session.start",
  "session.send",
  "session.cancel",
  "session.respond",
]);
