// Reading workspace files from the Host: in ranges when it offers
// `file_ranges`, else in one whole-file call (up to 10 MB).

import { UPLOAD_TIMEOUT_MS } from "./attachments";
import { hasCapability } from "./client/compat";
import type { ConnectionState } from "./client/connection";
import type { HostClient } from "./client/host-client";

/** Bytes asked for per range; the Host returns at most 8 MiB. */
export const RANGE_BYTES = 4 * 1024 * 1024;
/** One range is ~5.6 MB of base64; give it longer than the default. */
export const RANGE_TIMEOUT_MS = 60_000;

/** What file reads need from a Host connection: requests, and its compatibility verdict. */
export type FileReader = Pick<HostClient, "request"> & { readonly state: Pick<ConnectionState, "compat"> };

export interface FileChunk {
  name: string;
  mime_type: string;
  /** The whole file's size. */
  size: number;
  offset: number;
  bytes: Uint8Array;
}

export function hasFileRanges(client: Pick<FileReader, "state">): boolean {
  const compat = client.state.compat;
  return compat != null && "capabilities" in compat && compat.capabilities.includes("file_ranges");
}

export function decodeBase64(data: string): Uint8Array {
  const raw = atob(data);
  const out = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

/**
 * The file front to back. A file that shrinks while read ends early; one that grows ends at its new size.
 * `guardHidden` refuses a symlink to a hidden file, on a Host that offers it: an older one refuses the field.
 */
export async function* readChunks(
  client: FileReader,
  workspaceId: string,
  path: string,
  opts: { guardHidden?: boolean } = {},
): AsyncGenerator<FileChunk> {
  const guard = opts.guardHidden && hasCapability(client.state.compat, "files.guard_hidden") ? { guard_hidden: true } : {};
  if (!hasFileRanges(client)) {
    const f = await client.request("files.download", { workspace_id: workspaceId, path, ...guard }, { timeoutMs: UPLOAD_TIMEOUT_MS });
    yield { name: f.name, mime_type: f.mime_type, size: f.size, offset: 0, bytes: decodeBase64(f.data) };
    return;
  }
  let offset = 0;
  for (;;) {
    const f = await client.request(
      "files.download",
      { workspace_id: workspaceId, path, offset, length: RANGE_BYTES, ...guard },
      { timeoutMs: RANGE_TIMEOUT_MS },
    );
    const bytes = decodeBase64(f.data);
    yield { name: f.name, mime_type: f.mime_type, size: f.size, offset, bytes };
    offset += bytes.length;
    if (bytes.length === 0 || offset >= f.size) return;
  }
}

export async function readFile(
  client: FileReader,
  workspaceId: string,
  path: string,
  opts: { onProgress?: (received: number, size: number) => void } = {},
): Promise<Uint8Array> {
  const parts: Uint8Array[] = [];
  let received = 0;
  for await (const chunk of readChunks(client, workspaceId, path)) {
    parts.push(chunk.bytes);
    received += chunk.bytes.length;
    opts.onProgress?.(received, chunk.size);
  }
  if (parts.length === 1) return parts[0]!;
  const out = new Uint8Array(received);
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

/** One range; needs a Host with `file_ranges`. */
export async function readRange(
  client: Pick<HostClient, "request">,
  workspaceId: string,
  path: string,
  offset: number,
  length: number,
): Promise<{ bytes: Uint8Array; size: number; name: string; mime_type: string }> {
  const f = await client.request(
    "files.download",
    { workspace_id: workspaceId, path, offset, length },
    { timeoutMs: RANGE_TIMEOUT_MS },
  );
  return { bytes: decodeBase64(f.data), size: f.size, name: f.name, mime_type: f.mime_type };
}
