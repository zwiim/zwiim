// React views of the app's services.

import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import {
  initialHostState,
  isClientError,
  type ConnectionMode,
  type HostState,
  type KeepAwakeStatus,
  type WorkspaceInfo,
} from "@zwiim/sdk";
import { accountHosts, byLastUsed, type HostEntry } from "../account/hosts";
import type { CurrentHostState } from "../host/current";
import { currentWorkspace } from "../host/workspace";
import type { TerminalScreen } from "../terminal/screen";
import { pendingCount } from "../sessions/model";
import { preferences, secureStore } from "../platform/native";
import { auth, cloud, currentHost } from "../services";
import { useAccount } from "./account";

/** The host being controlled, and its connection. */
export function useCurrentHost(): CurrentHostState {
  return useSyncExternalStore(currentHost.subscribe, () => currentHost.state);
}

const NO_HOST_STATE = initialHostState();
const noSubscription = () => () => {};

function useHostSelector<T>(select: (state: HostState) => T): T {
  const { store } = useCurrentHost();
  return useSyncExternalStore<T>(store?.subscribe ?? noSubscription, () => select(store?.getState() ?? NO_HOST_STATE));
}

/** The current host's workspaces, sessions and their live views. */
export function useHostState(): HostState {
  return useHostSelector((s) => s);
}

/** Whether the current host keeps its Mac awake, as its feed last said. */
export function useKeepAwake(): KeepAwakeStatus | null {
  return useHostSelector((s) => s.keepAwake);
}

/** Requests pending across the current host. */
export function usePendingCount(): number {
  return useHostSelector(pendingCount);
}

/** The worktree Files shows: its changes, files and terminals. */
export function useWorkspace(): WorkspaceInfo | null {
  const { workspaceId } = useCurrentHost();
  const workspaces = useHostSelector((s) => s.workspaces);
  return currentWorkspace(workspaces, workspaceId);
}

export interface Loaded<T> {
  data: T | null;
  error: unknown;
  loading: boolean;
  reload(): void;
}

/**
 * Something read from the host, read again whenever `key` changes, on
 * `reload()`, and when the connection comes back. `key` null: nothing to read.
 */
export function useHostRead<T>(key: string | null, load: () => Promise<T>): Loaded<T> {
  const { view } = useCurrentHost();
  const live = view?.status === "live";
  const [state, setState] = useState<{ key: string | null; data: T | null; error: unknown; loading: boolean }>({
    key: null,
    data: null,
    error: null,
    loading: true,
  });
  const [tick, setTick] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  useEffect(() => {
    if (key === null || !live) return;
    let current = true;
    // Another key starts empty; the same one keeps its data while it refreshes.
    setState((s) => (s.key === key ? { ...s, loading: true } : { key, data: null, error: null, loading: true }));
    loadRef.current().then(
      (data) => current && setState({ key, data, error: null, loading: false }),
      (error: unknown) => current && setState({ key, data: null, error, loading: false }),
    );
    return () => {
      current = false;
    };
  }, [key, live, tick]);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  const mine = state.key === key;
  return { data: mine ? state.data : null, error: mine ? state.error : null, loading: !mine || state.loading, reload };
}

/** A terminal's screen while `terminal_id` is set, streamed from the host. */
export function useTerminalScreen(terminal_id: string | null): TerminalScreen | null {
  const { store } = useCurrentHost();
  const [screen, setScreen] = useState<TerminalScreen | null>(null);
  useEffect(() => {
    if (!store || !terminal_id) return;
    const hold = currentHost.openTerminal(terminal_id);
    setScreen(hold?.screen ?? null);
    return () => {
      hold?.release();
      setScreen(null);
    };
  }, [store, terminal_id]);
  useSyncExternalStore(screen?.subscribe ?? noSubscription, () => screen?.version ?? 0);
  return screen;
}

/** The time in ms, ticking every second while `ticking`. */
export function useNow(ticking: boolean): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    setNow(Date.now());
    if (!ticking) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [ticking]);
  return now;
}

/** The account's hosts, last used first, with the connection mode kept for each. */
export function useAccountHosts() {
  const { signedOut } = useAccount();
  const [hosts, setHosts] = useState<HostEntry[] | null>(null);
  const [modes, setModes] = useState<Record<string, ConnectionMode>>({});
  const [loading, setLoading] = useState(false);
  const [unreachable, setUnreachable] = useState(false);

  const load = useCallback(async () => {
    if (!cloud || !auth) return;
    setLoading(true);
    try {
      const [list, last, kept] = await Promise.all([
        accountHosts(cloud, secureStore, () => auth!.accessToken(true)),
        preferences.lastHost(),
        preferences.connectionModes(),
      ]);
      setHosts(byLastUsed(list, last?.host_id ?? null));
      setModes(kept);
      setUnreachable(false);
    } catch (e) {
      if (isClientError(e, "unauthenticated")) signedOut();
      else setUnreachable(true);
    } finally {
      setLoading(false);
    }
  }, [signedOut]);

  return { hosts, modes, loading, unreachable, load };
}
