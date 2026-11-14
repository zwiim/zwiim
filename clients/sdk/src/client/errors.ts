import type {
  HostErrorCode,
  HostOwnedDetails,
  LargeFolderDetails,
  OptionUnavailableDetails,
  WorkspaceBusyDetails,
  WorkspaceDirtyDetails,
} from "../types/protocol";

/**
 * Client-side failure codes, in addition to the host's `HostErrorCode`s.
 *
 * - `network`: the request never reached the host (safe to retry reads).
 * - `unknown_outcome`: a mutating request may or may not have been applied;
 *   the SDK never retries it — refresh host state instead.
 * - `timeout`: a read did not complete in time.
 * - `invalid_response`: the host answered with something that is not V1.
 * - `host_mismatch`: the host at the URL is not the one we were bound to.
 *
 * Reaching a host through Zwiim Cloud adds:
 *
 * - `host_offline`: the host is not connected to the Cloud (retried).
 * - `cloud_unavailable`: the Cloud is unreachable or restarting (retried).
 * - `host_unknown`: the account has no such host (terminal).
 * - `device_refused`: the host does not trust this device, e.g. it was
 *   revoked (terminal).
 * - `handshake_failed`: the host's tunnel handshake did not authenticate
 *   against the pinned host key (terminal).
 * - `relay_quota_exceeded`: the account reached a relay limit of its plan
 *   (`RelayQuotaError` on a refused ticket). Retried with backoff: the
 *   limit lifts at the period's reset, and the local network still works.
 *
 * Reaching a host directly on the local network (`LanTransport`,
 * `ConnectionManager`) adds:
 *
 * - `lan_unavailable`: the host is not reachable directly: not advertised
 *   on this network, or not answering there (retried; `auto` mode uses the
 *   relay instead).
 * - `host_identity_mismatch`: a host on this network advertises the
 *   selected host's id with a key other than the pinned one (terminal).
 *
 * Compatibility (`evaluateCompatibility`) adds:
 *
 * - `update_required`: this client or the Host is too old for the other.
 *   The connection is `incompatible` and operations are refused until the
 *   named side is updated (re-checked on a slow timer).
 *
 * Pairing a device with a host's QR (`parsePairingInvitation`,
 * `pairThroughCloud`) adds:
 *
 * - `pairing_invalid`: not a pairing QR, or not for this account's host.
 * - `pairing_expired`: the QR's code has expired.
 * - `pairing_rejected`: the host refused the code (unknown, expired or
 *   already used).
 *
 * Browser views (`openBrowserStream`) add:
 *
 * - `browser_stream_unavailable`: this connection cannot carry a browser
 *   stream (a phone, or a computer reaching the host remotely).
 */
export type ClientErrorCode =
  | HostErrorCode
  | "network"
  | "unknown_outcome"
  | "timeout"
  | "invalid_response"
  | "host_mismatch"
  | "host_offline"
  | "host_unknown"
  | "device_refused"
  | "handshake_failed"
  | "relay_quota_exceeded"
  | "lan_unavailable"
  | "host_identity_mismatch"
  | "pairing_invalid"
  | "pairing_expired"
  | "pairing_rejected"
  | "update_required"
  | "browser_stream_unavailable"
  | (string & {});

export class ZwiimClientError extends Error {
  readonly code: ClientErrorCode;
  /** HTTP status when the host answered. */
  readonly status: number | undefined;
  /** The host's structured `details`, for codes that document them. */
  readonly details: Record<string, unknown> | undefined;

  constructor(
    code: ClientErrorCode,
    message: string,
    status?: number,
    details?: Record<string, unknown>,
  ) {
    super(message);
    this.name = "ZwiimClientError";
    this.code = code;
    this.status = status;
    this.details = details;
  }
}

/** The running agents/terminals, or the repository operation, behind a `workspace_busy` refusal. */
export function busyDetails(e: unknown): WorkspaceBusyDetails | null {
  if (!isClientError(e, "workspace_busy")) return null;
  const d = e.details ?? {};
  const ids = Array.isArray(d.session_ids) ? d.session_ids.filter((id): id is string => typeof id === "string") : [];
  return {
    live_sessions: Number(d.live_sessions ?? 0),
    terminals: Number(d.terminals ?? 0),
    session_ids: ids,
    ...(typeof d.operation === "string" ? { operation: d.operation } : {}),
  };
}

/** The uncommitted changes behind a `workspace_dirty` refusal. */
export function dirtyDetails(e: unknown): WorkspaceDirtyDetails | null {
  if (!isClientError(e, "workspace_dirty")) return null;
  return { changes: Number(e.details?.changes ?? 0) };
}

/** What a `large_folder` refusal counted, or null for any other failure. */
export function largeFolderDetails(e: unknown): LargeFolderDetails | null {
  if (!isClientError(e, "large_folder")) return null;
  return {
    files: Number(e.details?.files ?? 0),
    bytes: Number(e.details?.bytes ?? 0),
    at_least: Boolean(e.details?.at_least),
  };
}

const LARGE_FOLDER_FILES = 5_000;
const thousands = (n: number) => String(Math.round(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");

/** The first commit's size in words: "12,431 files (1.8 GB)", or the limit it passed. */
export function largeFolderText(d: LargeFolderDetails): string {
  if (d.at_least) return d.files > LARGE_FOLDER_FILES ? "more than 5,000 files" : "more than 500 MB";
  const mb = d.bytes / (1024 * 1024);
  const size = mb >= 1024 ? `${(mb / 1024).toFixed(1)} GB` : `${thousands(Math.max(1, mb))} MB`;
  return `${thousands(d.files)} ${d.files === 1 ? "file" : "files"} (${size})`;
}

/** The option and value behind an `option_unavailable` refusal. */
export function optionUnavailableDetails(e: unknown): OptionUnavailableDetails | null {
  if (!isClientError(e, "option_unavailable")) return null;
  return { option: String(e.details?.option ?? ""), value: String(e.details?.value ?? "") };
}

/** The owning account behind a `host_owned` refusal. */
export function hostOwnedDetails(e: unknown): HostOwnedDetails | null {
  if (!isClientError(e, "host_owned")) return null;
  const email = e.details?.owner_email;
  return { owner_email: typeof email === "string" ? email : null };
}

/** Whether a `publish_failed` refusal still created the GitHub repository. */
export function publishFailedDetails(e: unknown): { created_on_github: boolean } | null {
  if (!isClientError(e, "publish_failed")) return null;
  return { created_on_github: Boolean(e.details?.created_on_github) };
}

export function isClientError(e: unknown, code?: ClientErrorCode): e is ZwiimClientError {
  return e instanceof ZwiimClientError && (code === undefined || e.code === code);
}
