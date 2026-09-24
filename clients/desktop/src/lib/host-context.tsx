import { createContext, useContext, useSyncExternalStore } from "react";
import type { HostController, HostState } from "@zwiim/sdk";

const HostContext = createContext<HostController | null>(null);

export const HostProvider = HostContext.Provider;

export function useController(): HostController {
  const c = useContext(HostContext);
  if (!c) throw new Error("useController outside HostProvider");
  return c;
}

/** Subscribe to a slice of host state. Selectors must return stable references. */
export function useHost<T>(selector: (s: HostState) => T): T {
  const { store } = useController();
  return useSyncExternalStore(store.subscribe, () => selector(store.getState()));
}

/**
 * Whether the host connection is live. Mutations are offered only then: a
 * request sent while reconnecting could not be told apart from a lost one.
 */
export function useOnline(): boolean {
  return useHost((s) => s.connection.status === "live");
}

/**
 * What to tell the user about a failed request. Codes whose host message is
 * technical get plain wording; the rest keep the host's message.
 */
const FRIENDLY: Record<string, string> = {
  unknown_outcome:
    "The connection dropped before the host answered. The request may or may not have been applied — check the current state before trying again.",
  network: "Cannot reach the host. Check that it is running, then try again.",
  timeout: "The host did not answer in time. Check the current state before trying again.",
  unauthenticated: "The host no longer accepts this app's credentials. Reconnect to continue.",
  turn_in_progress: "The agent is still working on the previous message. Wait for it to finish, or cancel it.",
  session_not_live: "This session's agent is not running any more. Send a message to resume it.",
  agent_not_installed: "That agent is not installed on the host.",
  option_unavailable: "The agent does not offer that option right now.",
  host_owned: "This host already belongs to another account.",
  host_mismatch: "A different host is now answering at this address. Restart the app to connect to it.",
};

export function errorMessage(e: unknown): string {
  const code = e && typeof e === "object" && "code" in e ? String((e as { code: unknown }).code) : null;
  if (code && code in FRIENDLY) return FRIENDLY[code]!;
  const message = e instanceof Error ? e.message : String(e);
  if (code === "internal") return `Something went wrong on the host: ${message}`;
  return message;
}
