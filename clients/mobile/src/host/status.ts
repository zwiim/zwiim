// How the current host's connection reads on screen (DESIGN.md, Status
// semantics and Global States): the Host bar's dot and path pill, the status
// line, and the banner or full-screen state a failure calls for.

import type { ActiveConnection, ConnectionMode } from "@zwiim/sdk";
import { hostMeta, type HostEntry } from "../account/hosts";
import type { DotState } from "../ui/components";
import type { ErrorInfo, HostConnectionView } from "./connection";
import type { CurrentHostState } from "./current";

/** The connection as the Host bar and status line show it. `now` in ms. */
export function connectionText(
  view: HostConnectionView | null,
  now = Date.now(),
): { status: string; pill: string; dot: DotState } {
  switch (view?.active) {
    case "lan":
      return { status: "Connected locally", pill: "⌂ local", dot: "done" };
    case "relay":
      return { status: "Connected through Zwiim Cloud", pill: "☁ cloud", dot: "done" };
    case "connecting":
      return { status: "Connecting…", pill: "connecting…", dot: "work" };
    case "reconnecting": {
      const wait = view.retryAt ? Math.ceil((view.retryAt - now) / 1000) : 0;
      return { status: "Reconnecting…", pill: wait > 0 ? `reconnecting · ${wait}s` : "reconnecting", dot: "work" };
    }
    default:
      return { status: "Offline", pill: "offline", dot: "idle" };
  }
}

/** A failure that needs more than waiting. */
export type HostProblem =
  /** Replaces the Host shell: only pairing again fixes it. */
  | { kind: "screen"; title: string; body: string }
  /**
   * Replaces the Host shell: this app or the Host is too old for the other.
   * Nothing runs against the Host until the named side is updated.
   */
  | { kind: "update"; update: "app" | "host"; title: string; body: string }
  /** Under the Host bar, with its next step. */
  | { kind: "banner"; text: string; action: "retry" | "switch" | "sign_in" };

export function hostProblem(
  view: HostConnectionView | null,
  openError: ErrorInfo | null,
  name: string,
): HostProblem | null {
  if (openError) {
    return openError.code === "device_refused"
      ? { kind: "screen", title: `This phone isn't paired with ${name}`, body: "Pair it to connect." }
      : { kind: "banner", text: `Couldn't open ${name}. Try again.`, action: "retry" };
  }
  if (!view || view.status === "live") return null;
  if (view.status === "incompatible") {
    return view.compat?.status === "update_required" && view.compat.update === "host"
      ? {
          kind: "update",
          update: "host",
          title: `${name} needs an update`,
          body: `Update Zwiim on ${name}, then come back. This phone reconnects by itself.`,
        }
      : {
          kind: "update",
          update: "app",
          title: "Update Zwiim",
          body: `${name} needs a newer version of this app.`,
        };
  }
  switch (view.error?.code) {
    case undefined:
      return null;
    case "device_refused":
      return { kind: "screen", title: `This phone was removed from ${name}`, body: "Pair again to reconnect." };
    case "handshake_failed":
    case "host_identity_mismatch":
      return {
        kind: "screen",
        title: `Couldn't verify ${name}`,
        body: "The Mac answering isn't the one you paired with. Pair again if you reinstalled Zwiim.",
      };
    case "lan_unavailable":
      if (view.mode !== "local") return null;
      return {
        kind: "banner",
        text: view.lanKnown
          ? `${name} isn't on this network. Switch to Automatic to connect through Zwiim Cloud.`
          : `This phone doesn't know where ${name} is on the network yet. Switch to Automatic to connect through Zwiim Cloud.`,
        action: "switch",
      };
    case "host_offline":
      return { kind: "banner", text: `${name} is offline. Showing nothing until it's back.`, action: "retry" };
    case "cloud_unavailable":
      // Only claim the direct path when this phone knows where to dial and
      // hasn't just missed it: in Automatic the Cloud is tried after the LAN.
      return {
        kind: "banner",
        text:
          view.mode === "cloud" && view.lanKnown
            ? "Zwiim Cloud is unreachable. Local connections still work."
            : "Zwiim Cloud is unreachable.",
        action: "retry",
      };
    case "unauthenticated":
      return { kind: "banner", text: "Your sign-in has expired.", action: "sign_in" };
    default:
      // Retryable failures show in the pill; a stopped connection needs a hand.
      return view.status === "reconnecting"
        ? null
        : { kind: "banner", text: `Couldn't connect to ${name}. Try again.`, action: "retry" };
  }
}

/**
 * A host row's dot and meta line: this phone's own connection when the host
 * is the current one, the Cloud's presence otherwise. `now` in Unix seconds.
 */
export function hostRowState(
  host: HostEntry,
  current: CurrentHostState,
  mode: ConnectionMode | undefined,
  now: number,
): { meta: string; dot: DotState } {
  let active: ActiveConnection | null = null;
  if (host.paired && current.host?.host_id === host.host_id && !current.openError) {
    active = current.view?.active ?? "connecting";
    // Retrying a host the Cloud has seen go offline: it's offline.
    if (active === "offline" || (active === "reconnecting" && !host.online)) active = null;
  }
  const dot: DotState =
    active === "connecting" || active === "reconnecting" ? "work" : active || host.online ? "done" : "idle";
  return { meta: hostMeta(host, now, { mode, active }), dot };
}
