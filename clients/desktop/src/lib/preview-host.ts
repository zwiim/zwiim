// The main window's half of an HTML preview: Rust hands each request the
// preview page makes to this resolver, which reads the file from the Host.

import type { FileReader } from "@zwiim/sdk";
import { answerPreview, encodePreviewPath, type PreviewAnswer } from "@zwiim/preview";
import { platform } from "@/lib/platform";

interface PreviewRequest {
  request: number;
  method: string;
  path: string;
}

/** Where `path` loads in preview `id`: its own origin, `http://zwiim-preview.<id>` on Windows. */
export function previewUrl(id: string, path: string, windows = platform() === "windows"): string {
  const origin = windows ? `http://zwiim-preview.${id}` : `zwiim-preview://${id}`;
  return `${origin}/${encodePreviewPath(path)}`;
}

/** Open a preview origin for `workspaceId`; its id. */
export async function openPreview(client: FileReader, workspaceId: string): Promise<string> {
  const { Channel, invoke } = await import("@tauri-apps/api/core");
  const channel = new Channel<PreviewRequest>();
  channel.onmessage = (req) => void reply(req);
  async function reply({ request, method, path }: PreviewRequest) {
    let answer: PreviewAnswer = await answerPreview(client, workspaceId, method, path);
    let body: Uint8Array = new Uint8Array();
    if (answer.status === 200) {
      try {
        const parts: Uint8Array[] = [];
        for await (const c of answer.body) parts.push(c);
        body = concat(parts);
      } catch {
        answer = { status: 502, text: "Couldn't read this file from the Host" };
      }
    }
    if ("text" in answer) body = new TextEncoder().encode(answer.text);
    await invoke("preview_reply", body, {
      headers: {
        "x-preview-request": String(request),
        "x-preview-status": String(answer.status),
        "x-preview-type": answer.status === 200 ? answer.mime_type : "text" in answer ? "text/plain; charset=utf-8" : "",
        "x-preview-location": answer.status === 301 ? answer.location : "",
      },
    });
  }
  return invoke<string>("preview_open", { onRequest: channel });
}

export async function closePreview(id: string): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("preview_close", { id });
}

function concat(parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}
