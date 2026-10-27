// Normalized client state for one host, keyed only by public ids. Pure
// reducer + a minimal observable store (compatible with React's
// `useSyncExternalStore`, but framework-free). All of it is disposable: it
// can be rebuilt from the host at any time.

import type { ConnectionState } from "../client/connection";
import type {
  AgentSummary,
  CloneJob,
  HostFeedEvent,
  HostHello,
  HostOwner,
  HostSnapshot,
  KeepAwakeStatus,
  PrEntry,
  ConflictCheck,
  SessionSummary,
  TerminalInfo,
  WorkspaceInfo,
} from "../types/protocol";
import { initialSessionView, reduceSession, type SessionFrame, type SessionView } from "./session";

export interface TerminalMeta {
  terminal_id: string;
  title: string | null;
  /** The workspace it runs in; null outside every workspace or not yet known. */
  workspace_id: string | null;
  alive: boolean;
  /** The event socket is streaming this terminal. */
  attached: boolean;
}

export interface HostState {
  connection: ConnectionState;
  host: HostHello | null;
  /** The host's agents and whether each can be used; null until loaded. */
  agents: AgentSummary[] | null;
  workspaces: WorkspaceInfo[] | null;
  sessionsByWorkspace: Record<string, SessionSummary[]>;
  sessions: Record<string, SessionView>;
  terminals: Record<string, TerminalMeta>;
  /**
   * Bumped whenever a workspace's diff may have changed; a view showing that
   * diff refetches when its number moves. Diffs themselves are not stored.
   */
  diffRevision: Record<string, number>;
  /**
   * Each workspace's pull request as the Host follows it on GitHub. A
   * workspace without an entry has no pull request (or no named branch).
   */
  prs: Record<string, PrEntry>;
  /**
   * Each workspace's merge state against its base, as the Host last checked.
   * A workspace without an entry has nothing to compare with.
   */
  conflicts: Record<string, ConflictCheck>;
  /** GitHub clones in flight or recently finished, by `clone_id`. */
  clones: Record<string, CloneJob>;
  /** Whether the host keeps its Mac awake, as the feed last said; null until it does. */
  keepAwake: KeepAwakeStatus | null;
}

export type HostAction =
  | { type: "connection"; state: ConnectionState }
  | { type: "host"; hello: HostHello }
  | { type: "host_owner"; owner: HostOwner }
  | { type: "agents"; agents: AgentSummary[] }
  | { type: "workspaces"; workspaces: WorkspaceInfo[] }
  | { type: "workspace_added"; workspace: WorkspaceInfo }
  | { type: "sessions"; workspace_id: string; sessions: SessionSummary[] }
  | { type: "session_frame"; session_id: string; frame: SessionFrame }
  | { type: "session_forget"; session_id: string }
  | { type: "terminals"; terminals: TerminalInfo[] }
  | { type: "terminal"; terminal_id: string; patch: Partial<Omit<TerminalMeta, "terminal_id">> }
  | { type: "terminal_forget"; terminal_id: string }
  | { type: "host_snapshot"; snapshot: HostSnapshot }
  | { type: "host_event"; event: HostFeedEvent };

export function initialHostState(): HostState {
  return {
    connection: { status: "disconnected", compat: null, attempt: 0, hostId: null, error: null, retryInMs: null },
    host: null,
    agents: null,
    workspaces: null,
    sessionsByWorkspace: {},
    sessions: {},
    terminals: {},
    diffRevision: {},
    prs: {},
    conflicts: {},
    clones: {},
    keepAwake: null,
  };
}

export function reduceHost(state: HostState, action: HostAction): HostState {
  switch (action.type) {
    case "connection":
      return { ...state, connection: action.state };
    case "host":
      return { ...state, host: action.hello };
    case "host_owner":
      return state.host ? { ...state, host: { ...state.host, owner: action.owner } } : state;
    case "agents":
      return { ...state, agents: action.agents };
    case "workspaces": {
      // Session lists of workspaces the host no longer lists are dropped.
      const listed = new Set(action.workspaces.map((w) => w.workspace_id));
      const sessionsByWorkspace = Object.fromEntries(
        Object.entries(state.sessionsByWorkspace).filter(([id]) => listed.has(id)),
      );
      return { ...state, workspaces: action.workspaces, sessionsByWorkspace };
    }
    case "workspace_added":
      return { ...state, workspaces: upsertWorkspace(state.workspaces, action.workspace) };
    case "sessions":
      return {
        ...state,
        sessionsByWorkspace: { ...state.sessionsByWorkspace, [action.workspace_id]: action.sessions },
      };
    case "session_frame": {
      const prev = state.sessions[action.session_id] ?? initialSessionView(action.session_id);
      const next = reduceSession(prev, action.frame);
      if (next === prev) return state;
      return { ...state, sessions: { ...state.sessions, [action.session_id]: next } };
    }
    case "session_forget": {
      const { [action.session_id]: _, ...rest } = state.sessions;
      return { ...state, sessions: rest };
    }
    case "terminals":
      return { ...state, terminals: listTerminals(state.terminals, action.terminals) };
    case "terminal": {
      const prev = state.terminals[action.terminal_id] ?? {
        terminal_id: action.terminal_id,
        title: null,
        workspace_id: null,
        alive: true,
        attached: false,
      };
      return {
        ...state,
        terminals: { ...state.terminals, [action.terminal_id]: { ...prev, ...action.patch } },
      };
    }
    case "terminal_forget": {
      const { [action.terminal_id]: _, ...rest } = state.terminals;
      return { ...state, terminals: rest };
    }
    case "host_snapshot": {
      const { workspaces, sessions, terminals } = action.snapshot;
      const sessionsByWorkspace: Record<string, SessionSummary[]> = {};
      const diffRevision: Record<string, number> = {};
      for (const w of workspaces) {
        sessionsByWorkspace[w.workspace_id] = [];
        // Changes made while not following the feed were not announced.
        diffRevision[w.workspace_id] = (state.diffRevision[w.workspace_id] ?? 0) + 1;
      }
      for (const s of sessions) {
        if (s.workspace_id !== null) (sessionsByWorkspace[s.workspace_id] ??= []).push(s);
      }
      return {
        ...state,
        workspaces,
        sessionsByWorkspace,
        terminals: listTerminals(state.terminals, terminals),
        diffRevision,
        prs: action.snapshot.prs ?? {},
        conflicts: action.snapshot.conflicts ?? {},
        clones: Object.fromEntries((action.snapshot.clones ?? []).map((c) => [c.clone_id, c])),
        keepAwake: action.snapshot.keep_awake ?? null,
      };
    }
    case "host_event":
      return reduceFeedEvent(state, action.event);
  }
}

function reduceFeedEvent(state: HostState, event: HostFeedEvent): HostState {
  switch (event.type) {
    case "workspace.changed": {
      const { workspace } = event.payload;
      const id = workspace.workspace_id;
      return {
        ...state,
        workspaces: upsertWorkspace(state.workspaces, workspace),
        // The feed announces every session, so a workspace it adds has none yet.
        sessionsByWorkspace: id in state.sessionsByWorkspace
          ? state.sessionsByWorkspace
          : { ...state.sessionsByWorkspace, [id]: [] },
      };
    }
    case "workspace.removed": {
      const id = event.payload.workspace_id;
      const { [id]: _, ...sessionsByWorkspace } = state.sessionsByWorkspace;
      const { [id]: __, ...prs } = state.prs;
      const { [id]: ___, ...conflicts } = state.conflicts;
      return {
        ...state,
        workspaces: (state.workspaces ?? []).filter((w) => w.workspace_id !== id),
        sessionsByWorkspace,
        prs,
        conflicts,
      };
    }
    case "workspace.pr_changed": {
      const { workspace_id, pr, error } = event.payload;
      const { [workspace_id]: _, ...prs } = state.prs;
      if (pr === null && error === null) return { ...state, prs };
      return { ...state, prs: { ...prs, [workspace_id]: { pr, error } } };
    }
    case "workspace.conflicts_changed": {
      const { workspace_id, conflicts: check } = event.payload;
      const { [workspace_id]: _, ...conflicts } = state.conflicts;
      return { ...state, conflicts: check === null ? conflicts : { ...conflicts, [workspace_id]: check } };
    }
    case "workspace.diff_changed": {
      const id = event.payload.workspace_id;
      return { ...state, diffRevision: { ...state.diffRevision, [id]: (state.diffRevision[id] ?? 0) + 1 } };
    }
    case "session.changed": {
      const { session } = event.payload;
      const sessionsByWorkspace = withoutSession(state.sessionsByWorkspace, session.session_id, session.workspace_id);
      const ws = session.workspace_id;
      if (ws !== null) sessionsByWorkspace[ws] = placeSession(state.sessionsByWorkspace[ws] ?? [], session);
      return { ...state, sessionsByWorkspace };
    }
    case "session.removed":
      return {
        ...state,
        sessionsByWorkspace: withoutSession(state.sessionsByWorkspace, event.payload.session_id, null),
      };
    case "terminal.changed": {
      const { terminal_id, title, workspace_id } = event.payload.terminal;
      const prev = state.terminals[terminal_id];
      return {
        ...state,
        terminals: {
          ...state.terminals,
          [terminal_id]: { terminal_id, title, workspace_id, alive: true, attached: prev?.attached ?? false },
        },
      };
    }
    case "terminal.removed": {
      const id = event.payload.terminal_id;
      const prev = state.terminals[id];
      if (!prev) return state;
      // An ended terminal that is still on screen stays, marked ended.
      if (prev.attached) {
        return { ...state, terminals: { ...state.terminals, [id]: { ...prev, alive: false } } };
      }
      const { [id]: _, ...rest } = state.terminals;
      return { ...state, terminals: rest };
    }
    case "clone.changed": {
      const clone = event.payload.clone;
      return { ...state, clones: { ...state.clones, [clone.clone_id]: clone } };
    }
    case "clone.removed": {
      const { [event.payload.clone_id]: _gone, ...clones } = state.clones;
      return { ...state, clones };
    }
    case "host.keep_awake_changed":
      return { ...state, keepAwake: event.payload };
    default:
      // An event from a newer Host: ignore it, keep the state.
      return state;
  }
}

function upsertWorkspace(list: WorkspaceInfo[] | null, workspace: WorkspaceInfo): WorkspaceInfo[] {
  const all = list ?? [];
  const exists = all.some((w) => w.workspace_id === workspace.workspace_id);
  return exists
    ? all.map((w) => (w.workspace_id === workspace.workspace_id ? workspace : w))
    : [...all, workspace];
}

/** Session lists with `session_id` dropped from every workspace except `keep`. */
function withoutSession(
  byWorkspace: Record<string, SessionSummary[]>,
  session_id: string,
  keep: string | null,
): Record<string, SessionSummary[]> {
  const out = { ...byWorkspace };
  for (const [id, list] of Object.entries(byWorkspace)) {
    if (id !== keep && list.some((s) => s.session_id === session_id)) {
      out[id] = list.filter((s) => s.session_id !== session_id);
    }
  }
  return out;
}

/** Replace in place, or insert where the host lists it: newest first. */
function placeSession(list: SessionSummary[], session: SessionSummary): SessionSummary[] {
  if (list.some((s) => s.session_id === session.session_id)) {
    return list.map((s) => (s.session_id === session.session_id ? session : s));
  }
  const at = list.findIndex((s) => s.created_at < session.created_at);
  return at < 0 ? [...list, session] : [...list.slice(0, at), session, ...list.slice(at)];
}

function listTerminals(prev: Record<string, TerminalMeta>, list: TerminalInfo[]): Record<string, TerminalMeta> {
  const terminals: Record<string, TerminalMeta> = {};
  for (const t of list) {
    terminals[t.terminal_id] = {
      terminal_id: t.terminal_id,
      title: t.title,
      workspace_id: t.workspace_id,
      alive: true,
      attached: prev[t.terminal_id]?.attached ?? false,
    };
  }
  // Keep ended terminals that are still on screen.
  for (const [id, meta] of Object.entries(prev)) {
    if (!terminals[id] && meta.attached) terminals[id] = { ...meta, alive: false };
  }
  return terminals;
}

export interface Store<S, A> {
  getState(): S;
  dispatch(action: A): void;
  subscribe(listener: () => void): () => void;
}

export function createStore<S, A>(reducer: (s: S, a: A) => S, initial: S): Store<S, A> {
  let state = initial;
  const listeners = new Set<() => void>();
  return {
    getState: () => state,
    dispatch(action) {
      const next = reducer(state, action);
      if (next === state) return;
      state = next;
      for (const l of [...listeners]) l();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}

export type HostStore = Store<HostState, HostAction>;

export function createHostStore(): HostStore {
  return createStore(reduceHost, initialHostState());
}
