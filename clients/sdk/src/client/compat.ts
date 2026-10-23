// Client ↔ Host compatibility: the SDK's one decision about whether this
// client and a Host can work together. Shared by Desktop and Mobile.
//
// It is never "same version". The Host reports its protocol major, the
// revision it speaks, the oldest client revision it serves, and the optional
// capabilities it offers (all in `host.hello`). This client knows its own
// revision, the oldest Host revision it needs, and the optional capabilities
// it can use. The outcome:
//
//   compatible       everything this client uses is there
//   limited          works; some optional features are missing on this Host
//   update_required  one side is too old; says which, and what it needs
//
// The SDK decides; it never updates anything. `update_required` names the
// side to update so the app can send the user to the right place.

import type { HostHello } from "../types/protocol";
import { isClientError } from "./errors";

/** `zwiim/<major>`: a different major is never compatible. */
export const PROTOCOL_MAJOR = 1;

/** Protocol revision this client speaks. Bump with the Host's revision. */
export const CLIENT_REVISION = 3;

/** Oldest Host revision this client can work with. */
export const MIN_HOST_REVISION = 1;

/**
 * Optional Host features this client uses when offered. A Host without one
 * of them is `limited`, not broken; the app hides that feature.
 */
export const CLIENT_CAPABILITIES: readonly string[] = [
  "workspace.browse",
  "workspace.worktrees",
  "workspace.continue",
  "files.write",
  "git",
  "git.pr",
  "terminal",
  "terminal.agent",
  "session.options",
  "session.effort_by_model",
  "session.usage",
  "session.commands",
  "host.lan_endpoint",
  "workspace.publish",
  "attachments",
  "file_ranges",
  "workspace.clone",
  "session.plan_hand_off",
  "session.revert",
];

/** What an app says about itself in `host.hello`. */
export interface ClientInfo {
  /** `desktop` | `mobile`. */
  app: string;
  /** The app's own marketing version, e.g. `0.2.0`. Display and diagnostics only. */
  version: string;
  build?: string;
  platform?: string;
}

/** The `client` object sent in the `host.hello` body. */
export interface HelloClient extends ClientInfo {
  revision: number;
  capabilities: readonly string[];
}

export type UpdateTarget = "client" | "host";

export type Compatibility =
  | {
      status: "compatible";
      hostRevision: number;
      capabilities: readonly string[];
    }
  | {
      status: "limited";
      hostRevision: number;
      capabilities: readonly string[];
      /** Optional features this client uses that the Host does not offer. */
      missing: readonly string[];
    }
  | {
      status: "update_required";
      /** Which side must be updated. */
      update: UpdateTarget;
      reason: "protocol_major" | "host_revision" | "client_revision";
      hostRevision: number;
      /** The client revision this Host needs, when the client must update. */
      requiredClientRevision: number | null;
      /** The Host revision this client needs, when the Host must update. */
      requiredHostRevision: number | null;
    };

export interface ClientCompat {
  major: number;
  revision: number;
  minHostRevision: number;
  capabilities: readonly string[];
}

export const THIS_CLIENT: ClientCompat = {
  major: PROTOCOL_MAJOR,
  revision: CLIENT_REVISION,
  minHostRevision: MIN_HOST_REVISION,
  capabilities: CLIENT_CAPABILITIES,
};

/** `"zwiim/1"` → 1; anything else → null. */
export function protocolMajor(protocol: unknown): number | null {
  if (typeof protocol !== "string") return null;
  const m = /^zwiim\/(\d+)$/.exec(protocol);
  return m ? Number(m[1]) : null;
}

function nonNegativeInt(value: unknown): number {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : 0;
}

/**
 * Decide compatibility from a Host's hello. Missing fields (a Host older than
 * the handshake) read as revision 0 with no capabilities. Unknown capability
 * names are ignored.
 */
export function evaluateCompatibility(hello: HostHello, client: ClientCompat = THIS_CLIENT): Compatibility {
  const hostRevision = nonNegativeInt(hello.revision);
  const minClient = nonNegativeInt(hello.min_client_revision);
  const offered = Array.isArray(hello.capabilities)
    ? hello.capabilities.filter((c): c is string => typeof c === "string")
    : [];
  const major = protocolMajor(hello.protocol);

  if (major === null || major !== client.major) {
    const hostOlder = major !== null && major < client.major;
    return {
      status: "update_required",
      update: hostOlder ? "host" : "client",
      reason: "protocol_major",
      hostRevision,
      requiredClientRevision: null,
      requiredHostRevision: null,
    };
  }
  if (client.revision < minClient) {
    return {
      status: "update_required",
      update: "client",
      reason: "client_revision",
      hostRevision,
      requiredClientRevision: minClient,
      requiredHostRevision: null,
    };
  }
  if (hostRevision < client.minHostRevision) {
    return {
      status: "update_required",
      update: "host",
      reason: "host_revision",
      hostRevision,
      requiredClientRevision: null,
      requiredHostRevision: client.minHostRevision,
    };
  }
  const have = new Set(offered);
  const missing = client.capabilities.filter((c) => !have.has(c));
  const capabilities = client.capabilities.filter((c) => have.has(c));
  return missing.length > 0
    ? { status: "limited", hostRevision, capabilities, missing }
    : { status: "compatible", hostRevision, capabilities };
}

/** Whether a Host offers an optional feature. False while not compatible. */
export function hasCapability(compat: Compatibility | null, capability: string): boolean {
  return compat !== null && compat.status !== "update_required" && compat.capabilities.includes(capability);
}

/** The hello body for this client. */
export function helloClient(info: ClientInfo, client: ClientCompat = THIS_CLIENT): HelloClient {
  return { ...info, revision: client.revision, capabilities: client.capabilities };
}

/**
 * Whether a failed request means the Host predates what this client asked
 * for: an operation it does not know, or a field it rejects by name. The
 * app should say "update the Host", not "something went wrong".
 */
export function isHostOutdated(error: unknown): boolean {
  if (!isClientError(error)) return false;
  if (error.code === "unknown_operation") return true;
  return error.code === "invalid_request" && error.details?.reason === "unknown_field";
}
