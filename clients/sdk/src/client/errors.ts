import type {
  HostErrorCode,
  HostOwnedDetails,
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
 */
export type ClientErrorCode =
  | HostErrorCode
  | "network"
  | "unknown_outcome"
  | "timeout"
  | "invalid_response"
  | "host_mismatch"
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

/** The running agents/terminals behind a `workspace_busy` refusal. */
export function busyDetails(e: unknown): WorkspaceBusyDetails | null {
  if (!isClientError(e, "workspace_busy")) return null;
  const d = e.details ?? {};
  return { live_sessions: Number(d.live_sessions ?? 0), terminals: Number(d.terminals ?? 0) };
}

/** The uncommitted changes behind a `workspace_dirty` refusal. */
export function dirtyDetails(e: unknown): WorkspaceDirtyDetails | null {
  if (!isClientError(e, "workspace_dirty")) return null;
  return { changes: Number(e.details?.changes ?? 0) };
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

export function isClientError(e: unknown, code?: ClientErrorCode): e is ZwiimClientError {
  return e instanceof ZwiimClientError && (code === undefined || e.code === code);
}
