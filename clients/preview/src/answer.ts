import { hasFileRanges, isClientError, readChunks, type FileChunk, type FileReader } from "@zwiim/sdk";
import { previewStatus } from "./errors";
import { encodePreviewPath, resolvePreviewPath } from "./path";

export type PreviewAnswer =
  | { status: 200; mime_type: string; size: number; body: AsyncIterable<Uint8Array> }
  | { status: 301; location: string }
  | { status: 404 | 405 | 413 | 502; text: string };

const TEXT = { 404: "Not found", 405: "Method not allowed", 413: "Too large to load here", 502: "Couldn't read this file from the Host" } as const;

/** Answer one request a preview page made for `rawPath` (URL path, maybe with a query). */
export async function answerPreview(
  client: FileReader,
  workspaceId: string,
  method: string,
  rawPath: string,
): Promise<PreviewAnswer> {
  if (method !== "GET" && method !== "HEAD") return { status: 405, text: TEXT[405] };
  const path = resolvePreviewPath(rawPath);
  if (path === null) return { status: 404, text: TEXT[404] };
  const chunks = readChunks(client, workspaceId, path);
  let first: IteratorResult<FileChunk>;
  try {
    first = await chunks.next();
  } catch (e) {
    if (isClientError(e, "not_found") && (await isDirectory(client, workspaceId, path))) {
      return { status: 301, location: `/${encodePreviewPath(path)}/` };
    }
    const status = previewStatus(e);
    return { status, text: TEXT[status] };
  }
  if (first.done) return { status: 502, text: TEXT[502] };
  const head = first.value;
  if (method === "HEAD") {
    await chunks.return(undefined);
    return { status: 200, mime_type: head.mime_type, size: head.size, body: (async function* () {})() };
  }
  async function* body() {
    yield head.bytes;
    for await (const c of chunks) yield c.bytes;
  }
  return { status: 200, mime_type: head.mime_type, size: head.size, body: body() };
}

/** A missing file whose `<path>/index.html` exists is a directory. */
async function isDirectory(client: FileReader, workspaceId: string, path: string): Promise<boolean> {
  if (!hasFileRanges(client) || path.endsWith("index.html")) return false;
  try {
    await client.request("files.download", { workspace_id: workspaceId, path: `${path}/index.html`, offset: 0, length: 0 });
    return true;
  } catch {
    return false;
  }
}
