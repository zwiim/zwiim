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
  HostDownload,
  HostOwner,
  HostSnapshot,
  ItemSummary,
  KeepAwakeStatus,
  PendingApproval,
  PrEntry,
  ConflictCheck,
  RunItemView,
  RunView,
  SessionSummary,
  TerminalInfo,
  WorkspaceInfo,
  WorkspaceLayout,
} from "../types/protocol";
import { initialBrowserState, reduceBrowser, type BrowserState, type BrowserUpdate } from "./browser";
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
  /** How the workspaces are arranged; null until the feed says, and on older Hosts. */
  layout: WorkspaceLayout | null;
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
  /**
   * Workspaces whose conflicts a session's turn is resolving, to that
   * session. The entry goes when the turn ends.
   */
  resolving: Record<string, string>;
  /** GitHub clones in flight or recently finished, by `clone_id`. */
  clones: Record<string, CloneJob>;
  /** Whether the host keeps its Mac awake, as the feed last said; null until it does. */
  keepAwake: KeepAwakeStatus | null;
  /** The Host browser's tabs and engine, once a view has asked for them. */
  browser: BrowserState;
  /** What the Host is downloading, oldest first. */
  downloads: HostDownload[];
  /** Playbook runs by `run_id`. An ended run stays after the feed lets it go. */
  runs: Record<string, RunView>;
  /** Approvals waiting for the person, by `approval_id`. */
  approvals: Record<string, PendingApproval>;
  /** Items of runs a view loaded or the feed reported, by `run_id` then `item_id`. */
  runItems: Record<string, Record<string, RunItemView>>;
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
  | { type: "host_event"; event: HostFeedEvent }
  | { type: "browser"; update: BrowserUpdate }
  /** A page of a run's items (`run.items`), merged into `runItems`. */
  | { type: "run_items_loaded"; run_id: string; items: ItemSummary[] };

export function initialHostState(): HostState {
  return {
    connection: { status: "disconnected", compat: null, attempt: 0, hostId: null, error: null, retryInMs: null },
    host: null,
    agents: null,
    workspaces: null,
    layout: null,
    sessionsByWorkspace: {},
    sessions: {},
    terminals: {},
    diffRevision: {},
    prs: {},
    conflicts: {},
    resolving: {},
    clones: {},
    keepAwake: null,
    browser: initialBrowserState(),
    downloads: [],
    runs: {},
    approvals: {},
    runItems: {},
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
        layout: action.snapshot.layout ?? null,
        sessionsByWorkspace,
        terminals: listTerminals(state.terminals, terminals),
        diffRevision,
        prs: action.snapshot.prs ?? {},
        conflicts: action.snapshot.conflicts ?? {},
        resolving: action.snapshot.resolving ?? {},
        clones: Object.fromEntries((action.snapshot.clones ?? []).map((c) => [c.clone_id, c])),
        keepAwake: action.snapshot.keep_awake ?? null,
        downloads: action.snapshot.downloads ?? [],
        runs: snapshotRuns(state.runs, action.snapshot.runs ?? []),
        approvals: byKey(action.snapshot.approvals ?? [], (a) => a.approval_id),
        // Items may have moved on while the feed was away; views load them again.
        runItems: {},
      };
    }
    case "run_items_loaded": {
      const items = action.items.map((i) => itemView(action.run_id, i));
      return withRunItems(state, action.run_id, items);
    }
    case "host_event":
      return reduceFeedEvent(state, action.event);
    case "browser": {
      const browser = reduceBrowser(state.browser, action.update);
      return browser === state.browser ? state : { ...state, browser };
    }
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
    case "workspace.layout":
      return { ...state, layout: event.payload.layout };
    case "workspace.removed": {
      const id = event.payload.workspace_id;
      const { [id]: _, ...sessionsByWorkspace } = state.sessionsByWorkspace;
      const { [id]: __, ...prs } = state.prs;
      const { [id]: ___, ...conflicts } = state.conflicts;
      const { [id]: ____, ...resolving } = state.resolving;
      return {
        ...state,
        workspaces: (state.workspaces ?? []).filter((w) => w.workspace_id !== id),
        sessionsByWorkspace,
        prs,
        conflicts,
        resolving,
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
    case "workspace.resolving_changed": {
      const { workspace_id, session_id } = event.payload;
      const { [workspace_id]: _, ...resolving } = state.resolving;
      return { ...state, resolving: session_id === null ? resolving : { ...resolving, [workspace_id]: session_id } };
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
    case "run.changed":
      return { ...state, runs: { ...state.runs, [event.payload.run_id]: event.payload } };
    case "run.approval_requested":
      return { ...state, approvals: { ...state.approvals, [event.payload.approval_id]: event.payload } };
    case "run.approval_cleared": {
      const { [event.payload.approval_id]: _cleared, ...approvals } = state.approvals;
      return { ...state, approvals };
    }
    case "run.item_changed":
      return withRunItems(state, event.payload.run_id, [event.payload]);
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
    case "download.changed": {
      const d = event.payload.download;
      const i = state.downloads.findIndex((x) => x.download_id === d.download_id);
      const downloads = i < 0 ? [...state.downloads, d] : state.downloads.map((x, j) => (j === i ? d : x));
      return { ...state, downloads };
    }
    case "download.removed":
      return { ...state, downloads: state.downloads.filter((x) => x.download_id !== event.payload.download_id) };
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

const ENDED_RUN: ReadonlySet<RunView["status"]> = new Set(["done", "stopped", "failed"]);

/**
 * The snapshot lists active runs only. Ended runs the client saw end stay;
 * one it last saw active and the snapshot omits ended unseen, so it goes
 * (`run.list` has its final state).
 */
function snapshotRuns(prev: Record<string, RunView>, active: RunView[]): Record<string, RunView> {
  const ended = Object.values(prev).filter((r) => ENDED_RUN.has(r.status));
  return { ...byKey(ended, (r) => r.run_id), ...byKey(active, (r) => r.run_id) };
}

function byKey<T>(list: T[], key: (t: T) => string): Record<string, T> {
  return Object.fromEntries(list.map((t) => [key(t), t]));
}

/** A `run.items` entry in the shape `run.item_changed` carries. */
function itemView(run_id: string, i: ItemSummary): RunItemView {
  return {
    run_id,
    item_id: i.item_id,
    title: i.title,
    status: i.status,
    reason_code: i.reason_code,
    site: i.site,
    url: i.url,
    question: i.question ?? null,
    tab_id: i.tab_id ?? null,
    may_have_sent: i.may_have_sent ?? false,
  };
}

function withRunItems(state: HostState, run_id: string, items: RunItemView[]): HostState {
  const list = { ...state.runItems[run_id] };
  for (const item of items) list[item.item_id] = item;
  return { ...state, runItems: { ...state.runItems, [run_id]: list } };
}
