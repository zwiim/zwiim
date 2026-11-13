// How a Host download's progress reads on every client: "23 MB / 89 MB",
// or "23 MB" when the total is not known.

import type { HostDownload } from "../types/protocol";

/** Bytes as the Mac counts them (1 MB = 1,000,000 bytes), whole above 10. */
export function formatBytes(bytes: number): string {
  if (bytes < 1_000) return `${bytes} B`;
  const [value, unit] =
    bytes < 1_000_000 ? [bytes / 1_000, "KB"] : bytes < 1_000_000_000 ? [bytes / 1_000_000, "MB"] : [bytes / 1_000_000_000, "GB"];
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${unit}`;
}

export function downloadProgress(d: Pick<HostDownload, "received" | "total">): string {
  return d.total ? `${formatBytes(d.received)} / ${formatBytes(d.total)}` : formatBytes(d.received);
}
