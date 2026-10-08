import type { HostClient } from "./client/host-client";
import type { UploadedAttachment } from "./types/protocol";

/** Largest file the Host stores, decoded (`attachment_too_large` past it). */
export const ATTACHMENT_MAX_BYTES = 10 * 1024 * 1024;
/** Most files one message carries. */
export const ATTACHMENT_MAX_COUNT = 10;
/** An upload is up to ~14 MB of base64; give it longer than the default. */
export const UPLOAD_TIMEOUT_MS = 180_000;

/** What to tell the user when `session.start` / `session.send` fails with `attachment_not_found`. */
export const ATTACHMENT_GONE_MESSAGE = "An attachment is no longer available. Attach it again.";

/**
 * Which of `incoming` fit next to `existing` already-pending files, and one
 * message per refusal: each file over 10 MB, then the count limit once.
 */
export function checkAttachments<T extends { name: string; size: number }>(
  existing: number,
  incoming: readonly T[],
): { accepted: T[]; errors: string[] } {
  const accepted: T[] = [];
  const errors: string[] = [];
  let overCount = false;
  for (const file of incoming) {
    if (file.size > ATTACHMENT_MAX_BYTES) {
      errors.push(`${file.name} is larger than 10 MB.`);
    } else if (existing + accepted.length >= ATTACHMENT_MAX_COUNT) {
      overCount = true;
    } else {
      accepted.push(file);
    }
  }
  if (overCount) errors.push(`Up to ${ATTACHMENT_MAX_COUNT} files per message.`);
  return { accepted, errors };
}

/** Store one file in `workspace_id`; `data` is standard base64. */
export function uploadAttachment(
  client: Pick<HostClient, "request">,
  workspace_id: string,
  name: string,
  data: string,
): Promise<UploadedAttachment> {
  return client.request(
    "attachments.upload",
    { workspace_id, name, data },
    { timeoutMs: UPLOAD_TIMEOUT_MS },
  );
}

/** `2.1 MB`, `340 KB`, `12 B`. */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  const mb = bytes / (1024 * 1024);
  return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
}

export type AttachmentKind =
  | "image"
  | "pdf"
  | "spreadsheet"
  | "document"
  | "archive"
  | "code"
  | "text"
  | "file";

const KIND_BY_EXTENSION: Record<string, AttachmentKind> = {
  pdf: "pdf",
  xls: "spreadsheet",
  xlsx: "spreadsheet",
  ods: "spreadsheet",
  csv: "spreadsheet",
  tsv: "spreadsheet",
  doc: "document",
  docx: "document",
  odt: "document",
  rtf: "document",
  pages: "document",
  ppt: "document",
  pptx: "document",
  key: "document",
  zip: "archive",
  tar: "archive",
  gz: "archive",
  tgz: "archive",
  "7z": "archive",
  rar: "archive",
  html: "code",
  htm: "code",
  json: "code",
  xml: "code",
  js: "code",
  ts: "code",
  tsx: "code",
  py: "code",
  rs: "code",
  txt: "text",
  md: "text",
  log: "text",
};

/** Which icon a file gets, from its name, then its MIME type. */
export function attachmentKind(name: string, mime_type?: string): AttachmentKind {
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
  const byExt = KIND_BY_EXTENSION[ext];
  if (byExt) return byExt;
  if (mime_type?.startsWith("image/")) return "image";
  if (/^(png|jpe?g|gif|webp|heic|heif|bmp|svg|tiff?)$/.test(ext)) return "image";
  if (mime_type?.startsWith("text/")) return "text";
  return "file";
}
