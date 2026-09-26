// The Sessions tab as data (DESIGN.md, Screen 4 and Status semantics): every
// session of the host in one of three sections, NEEDS YOU, WORKING and
// RECENT, with its state, context and the request it waits on. Derived from
// the SDK's host state alone; nothing here is stored.

import {
  isClientError,
  type AgentRequest,
  type HostState,
  type PermissionOption,
  type QuestionSpec,
  type RequestAnswer,
  type SessionSummary,
  type SessionView,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import type { DotState } from "../ui/components";

export type SessionState =
  | "needs_you"
  | "working"
  | "error"
  | "disconnected"
  | "ready"
  | "finished"
  | "completed"
  | "cancelled"
  | "idle";

const LOOK: Record<SessionState, { dot: DotState; label: string }> = {
  needs_you: { dot: "need", label: "waiting for you" },
  working: { dot: "work", label: "working" },
  error: { dot: "err", label: "error" },
  disconnected: { dot: "off", label: "agent disconnected" },
  ready: { dot: "idle", label: "ready" },
  finished: { dot: "done", label: "turn finished" },
  completed: { dot: "done", label: "completed" },
  cancelled: { dot: "idle", label: "cancelled" },
  idle: { dot: "idle", label: "idle" },
};

export function sessionLook(state: SessionState): { dot: DotState; label: string } {
  return LOOK[state];
}

/**
 * A session's state. The live view (a running agent's own stream) is fresher
 * than the summary, so it decides while there is one.
 */
export function sessionState(summary: SessionSummary, view?: SessionView): SessionState {
  const streaming = view?.attach === "live" && view.agentStatus !== null;
  if (view?.pendingRequest) return "needs_you";
  if (streaming) {
    switch (view.agentStatus) {
      case "prompting":
        return "working";
      case "error":
        return "error";
      case "disconnected":
        return "disconnected";
      case "connecting":
        return "working";
    }
  }
  switch (view?.sessionStatus ?? summary.status) {
    case "completed":
      return "completed";
    case "cancelled":
      return "cancelled";
    case "pending_review":
      return summary.live ? "ready" : "finished";
    case "in_progress":
      // Running per the host, not yet streamed here.
      return summary.live ? "working" : "idle";
  }
}

export interface SessionItem {
  session_id: string;
  title: string;
  agent: string | null;
  /** The workspace's (or worktree's) name. */
  workspace: string | null;
  state: SessionState;
  /** The request it waits on, in NEEDS YOU. */
  request: AgentRequest | null;
  /** While working: the latest tool call's title. */
  activity: string | null;
  /** When the session last changed on the host (ms). */
  updatedAt: number;
  /** In NEEDS YOU: since when this phone has seen the request (ms). */
  waitingSince: number | null;
}

export interface SessionSections {
  /** The host's lists have arrived. */
  loaded: boolean;
  needsYou: SessionItem[];
  working: SessionItem[];
  recent: SessionItem[];
  /** Older sessions left out of RECENT. */
  more: number;
}

/** RECENT lists this many; the rest are counted. */
export const RECENT_LIMIT = 50;

export function workspaceLabel(w: WorkspaceInfo): string {
  return w.alias ?? w.name;
}

function latestActivity(view: SessionView | undefined): string | null {
  if (!view) return null;
  const calls = Object.values(view.live.toolCalls);
  for (let i = calls.length - 1; i >= 0; i--) {
    const title = calls[i]!.title;
    if (title) return title;
  }
  return null;
}

function toItem(
  s: SessionSummary,
  view: SessionView | undefined,
  names: ReadonlyMap<string, string>,
  seenAt: (request_id: string) => number | null,
): SessionItem {
  const st = sessionState(s, view);
  const request = st === "needs_you" ? view!.pendingRequest : null;
  const updatedAt = Date.parse(s.updated_at) || 0;
  return {
    session_id: s.session_id,
    title: s.title?.trim() || "Untitled session",
    agent: s.agent,
    workspace: s.workspace_id ? (names.get(s.workspace_id) ?? null) : null,
    state: st,
    request,
    activity: st === "working" ? latestActivity(view) : null,
    updatedAt,
    waitingSince: request ? (seenAt(request.request_id) ?? updatedAt) : null,
  };
}

const workspaceNames = (workspaces: readonly WorkspaceInfo[] | null) =>
  new Map((workspaces ?? []).map((w) => [w.workspace_id, workspaceLabel(w)]));

/** One session as the list shows it; null when the host doesn't list it. */
export function sessionItem(
  state: Pick<HostState, "workspaces" | "sessionsByWorkspace" | "sessions">,
  session_id: string,
  seenAt: (request_id: string) => number | null = () => null,
): SessionItem | null {
  const summary = findSession(state, session_id);
  return summary ? toItem(summary, state.sessions[session_id], workspaceNames(state.workspaces), seenAt) : null;
}

/** Every session the host lists, once, by the section it belongs in. */
export function sessionSections(
  state: Pick<HostState, "workspaces" | "sessionsByWorkspace" | "sessions">,
  seenAt: (request_id: string) => number | null = () => null,
): SessionSections {
  if (state.workspaces === null) return { loaded: false, needsYou: [], working: [], recent: [], more: 0 };
  const names = workspaceNames(state.workspaces);
  const items = new Map<string, SessionItem>();
  for (const list of Object.values(state.sessionsByWorkspace)) {
    for (const s of list) items.set(s.session_id, toItem(s, state.sessions[s.session_id], names, seenAt));
  }
  const all = [...items.values()];
  const newest = (a: SessionItem, b: SessionItem) => b.updatedAt - a.updatedAt;
  const needsYou = all
    .filter((i) => i.state === "needs_you")
    .sort((a, b) => a.waitingSince! - b.waitingSince!);
  const working = all.filter((i) => i.state === "working").sort(newest);
  const rest = all.filter((i) => i.state !== "needs_you" && i.state !== "working").sort(newest);
  return {
    loaded: true,
    needsYou,
    working,
    recent: rest.slice(0, RECENT_LIMIT),
    more: Math.max(0, rest.length - RECENT_LIMIT),
  };
}

/** Requests pending across the host: the Sessions tab's badge. */
export function pendingCount(state: Pick<HostState, "sessions">): number {
  let n = 0;
  for (const view of Object.values(state.sessions)) if (view.pendingRequest) n++;
  return n;
}

/** Find a session the host lists. */
export function findSession(state: Pick<HostState, "sessionsByWorkspace">, session_id: string): SessionSummary | null {
  for (const list of Object.values(state.sessionsByWorkspace)) {
    const s = list.find((x) => x.session_id === session_id);
    if (s) return s;
  }
  return null;
}

// ─── requests ────────────────────────────────────────────────────────────

/** How a NEEDS YOU card resolves its request. */
export type RequestAction =
  /** A command to allow or reject right on the card. */
  | { type: "inline"; command: string; options: PermissionOption[] }
  /** One button that opens the request sheet. */
  | { type: "review"; label: string };

const isReject = (o: PermissionOption) => o.kind.startsWith("reject");

/**
 * The agent's own options in reading order: rejecting first (left, away
 * from the thumb), allowing after, allow-once last (the primary action).
 */
export function orderOptions(options: readonly PermissionOption[]): PermissionOption[] {
  const rank = (o: PermissionOption) => (isReject(o) ? 0 : o.kind === "allow_once" ? 2 : 1);
  return [...options].sort((a, b) => rank(a) - rank(b));
}

/** Whether an option is the request's primary action (allow once). */
export function isPrimaryOption(o: PermissionOption, options: readonly PermissionOption[]): boolean {
  const primary = options.find((x) => x.kind === "allow_once") ?? options.find((x) => !isReject(x));
  return primary?.option_id === o.option_id;
}

export function isRejectOption(o: PermissionOption): boolean {
  return isReject(o);
}

export function requestAction(r: AgentRequest): RequestAction {
  switch (r.kind) {
    case "question":
      return { type: "review", label: "Answer" };
    case "plan_approval":
      return { type: "review", label: "Review plan" };
    case "permission": {
      if (r.plan_markdown) return { type: "review", label: "Review plan" };
      if (r.tool?.diffs.length) return { type: "review", label: "Review changes" };
      if (r.tool?.command) return { type: "inline", command: r.tool.command, options: orderOptions(r.options) };
      return { type: "review", label: "Review" };
    }
  }
}

/** One line on what the agent asks. */
export function requestTitle(r: AgentRequest): string {
  switch (r.kind) {
    case "question":
      return r.questions.length === 1 ? r.questions[0]!.question : `${r.questions.length} questions`;
    case "plan_approval":
      return "Approve the plan";
    case "permission":
      if (r.title) return r.title;
      if (r.plan_markdown) return "Approve the plan";
      if (r.tool?.command) return "Run a command";
      if (r.tool?.diffs.length) return r.tool.diffs.length === 1 ? "Edit a file" : `Edit ${r.tool.diffs.length} files`;
      return "Permission needed";
  }
}

/** A question's answer: the labels picked per question, or declined. */
export function questionAnswer(
  questions: readonly QuestionSpec[],
  picked: Readonly<Record<string, readonly string[]>>,
  declined = false,
): RequestAnswer {
  if (declined) return { kind: "question", answer: { answers: [], declined: true } };
  return {
    kind: "question",
    answer: { answers: questions.map((q) => ({ question_id: q.question_id, labels: [...(picked[q.question_id] ?? [])] })) },
  };
}

/** Every question has a pick. */
export function questionsAnswered(
  questions: readonly QuestionSpec[],
  picked: Readonly<Record<string, readonly string[]>>,
): boolean {
  return questions.every((q) => (picked[q.question_id]?.length ?? 0) > 0);
}

/** Pick or unpick `label`: one per question, or any number with `multi_select`. */
export function togglePick(q: QuestionSpec, current: readonly string[], label: string): string[] {
  if (!q.multi_select) return [label];
  return current.includes(label) ? current.filter((l) => l !== label) : [...current, label];
}

/** What went wrong sending an answer, and what to do. */
export function respondError(e: unknown, hostName: string): string {
  if (isClientError(e, "unknown_outcome")) return "Not sure that went through. Refreshed from your Mac.";
  if (isClientError(e, "not_found") || isClientError(e, "session_not_live")) {
    return "This request is no longer waiting. It was answered or the agent stopped.";
  }
  if (isClientError(e, "network") || isClientError(e, "timeout")) return `Not connected to ${hostName}. Try again when it's back.`;
  return "Couldn't send that. Try again.";
}

// ─── diffs ───────────────────────────────────────────────────────────────

export interface DiffLine {
  sign: " " | "+" | "-";
  text: string;
}

/**
 * A proposed change as lines: the unchanged lines around one changed region,
 * then what goes and what comes. Enough to judge an edit on a phone.
 */
export function diffLines(oldText: string | null, newText: string | null, context = 3): DiffLine[] {
  const a = oldText ? oldText.split("\n") : [];
  const b = newText ? newText.split("\n") : [];
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const lines: DiffLine[] = [];
  for (let i = Math.max(0, start - context); i < start; i++) lines.push({ sign: " ", text: a[i]! });
  for (let i = start; i < endA; i++) lines.push({ sign: "-", text: a[i]! });
  for (let i = start; i < endB; i++) lines.push({ sign: "+", text: b[i]! });
  for (let i = endA; i < Math.min(a.length, endA + context); i++) lines.push({ sign: " ", text: a[i]! });
  return lines;
}

/** Lines added and removed. */
export function diffCounts(lines: readonly DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const l of lines) {
    if (l.sign === "+") added++;
    else if (l.sign === "-") removed++;
  }
  return { added, removed };
}

// ─── time ────────────────────────────────────────────────────────────────

/** A duration in mono: "now", "42s", "3m", "2h", "4d". */
export function elapsed(fromMs: number, nowMs: number): string {
  const s = Math.max(0, Math.floor((nowMs - fromMs) / 1000));
  if (s < 5) return "now";
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h`;
  return `${Math.floor(s / 86_400)}d`;
}
