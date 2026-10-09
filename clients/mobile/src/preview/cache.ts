// Preview bytes live in the app cache, written range by range, so a large
// file never crosses the React Native bridge or sits whole in JS memory.
import { Directory, File, Paths } from "expo-file-system";
import type { FileChunk, FileReader } from "@zwiim/sdk";
import { readChunks } from "@zwiim/sdk";
import { answerPreview } from "@zwiim/preview";
import { respondPreview } from "../../modules/zwiim-preview";
import { currentHost } from "../services";

/** The current Host as the SDK's file reader (its compatibility lives on the connection view). */
export function hostReader(): FileReader {
  return {
    request: (op, params, opts) => currentHost.request(op, params, opts),
    get state() {
      return { compat: currentHost.state.view?.compat ?? null };
    },
  };
}

export function previewDir(previewId: string): Directory {
  const dir = new Directory(Paths.cache, "zwiim-preview", previewId);
  dir.create({ intermediates: true, idempotent: true });
  return dir;
}

async function writeChunks(target: File, chunks: AsyncIterable<FileChunk> | AsyncIterable<Uint8Array>): Promise<void> {
  target.create({ overwrite: true });
  const handle = target.open();
  try {
    for await (const c of chunks) handle.writeBytes(c instanceof Uint8Array ? c : c.bytes);
  } finally {
    handle.close();
  }
}

/** The file in `dir`; `onFirst` runs after the first range (the large-file prompt waits there). */
export async function downloadTo(
  client: FileReader,
  workspaceId: string,
  path: string,
  dir: Directory,
  onFirst?: (first: FileChunk) => Promise<void>,
): Promise<{ uri: string; name: string; mime_type: string; size: number }> {
  const chunks = readChunks(client, workspaceId, path);
  const first = await chunks.next();
  if (first.done) throw new Error("empty read");
  await onFirst?.(first.value);
  const target = new File(dir, first.value.name);
  async function* all() {
    yield first.value!;
    for await (const c of chunks) yield c;
  }
  await writeChunks(target, all());
  return { uri: target.uri, name: first.value.name, mime_type: first.value.mime_type, size: first.value.size };
}

/** Answer one request an HTML preview page made. */
export async function serveRequest(
  client: FileReader,
  workspaceId: string,
  previewId: string,
  req: { requestId: string; method: string; path: string },
): Promise<void> {
  const answer = await answerPreview(client, workspaceId, req.method, req.path);
  if (answer.status === 301) return respondPreview(req.requestId, { status: 301, location: answer.location });
  if (answer.status !== 200) return respondPreview(req.requestId, { status: answer.status, text: answer.text });
  const target = new File(previewDir(previewId), `r-${req.requestId}`);
  try {
    await writeChunks(target, answer.body);
  } catch {
    target.delete();
    return respondPreview(req.requestId, { status: 502, text: "Couldn't read this file from the Host" });
  }
  return respondPreview(req.requestId, { status: 200, mime_type: answer.mime_type, file: target.uri });
}
