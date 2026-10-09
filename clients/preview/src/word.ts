import DOMPurify from "dompurify";
import mammoth from "mammoth";

const FORBID_TAGS = ["script", "iframe", "object", "embed", "form", "style", "link", "meta", "base"];

/** mammoth's output with anything active removed; images stay as data: URIs. */
export function sanitizeWordHtml(html: string): string {
  return DOMPurify.sanitize(html, { FORBID_TAGS, FORBID_ATTR: ["style"] });
}

/** A .docx as sanitized HTML. Unsupported Word features are dropped. */
export async function renderWord(bytes: Uint8Array): Promise<string> {
  const arrayBuffer = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  const { value } = await mammoth.convertToHtml({ arrayBuffer });
  return sanitizeWordHtml(value);
}
