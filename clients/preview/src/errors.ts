import { formatSize, isClientError } from "@zwiim/sdk";

export function previewStatus(e: unknown): 404 | 413 | 502 {
  if (isClientError(e, "not_found") || isClientError(e, "workspace_removed")) return 404;
  if (isClientError(e, "file_too_large") || isClientError(e, "file_too_large_for_remote")) return 413;
  return 502;
}

/**
 * What a preview shows when it fails. Null for a Host or connection error the
 * app already words (offline, timed out…): show that message with Reload.
 */
export function previewErrorMessage(e: unknown, name: string): string | null {
  if (isClientError(e)) {
    if (e.code === "not_found" || e.code === "workspace_removed") return "This file no longer exists.";
    const size = typeof e.details?.size === "number" ? ` (${formatSize(e.details.size)})` : "";
    if (e.code === "file_too_large_for_remote") {
      return `Too large to preview over a remote connection${size}. It opens on the same network.`;
    }
    if (e.code === "file_too_large") return `Too large to preview${size}.`;
    return null;
  }
  const locked = (e as { name?: unknown } | null)?.name === "PasswordException";
  return `Couldn't preview ${name}. ${locked ? "It's password-protected." : "The file looks damaged or isn't supported."}`;
}
