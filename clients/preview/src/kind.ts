import { attachmentKind } from "@zwiim/sdk";

export type PreviewKind = "html" | "pdf" | "sheet" | "word" | "image" | "text" | "external";

const BY_EXTENSION: Record<string, PreviewKind> = {
  html: "html", htm: "html",
  pdf: "pdf",
  xlsx: "sheet", xlsm: "sheet", xls: "sheet", csv: "sheet", tsv: "sheet",
  docx: "word",
  doc: "external", ppt: "external", pptx: "external", ods: "external",
  pages: "external", numbers: "external", key: "external",
};

/** Lowercase extension without the dot; "" when there is none. */
export function extensionOf(name: string): string {
  const base = name.slice(name.lastIndexOf("/") + 1).toLowerCase();
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1) : "";
}

/** How Zwiim shows a file in-app. */
export function previewKind(name: string): PreviewKind {
  const known = BY_EXTENSION[extensionOf(name)];
  if (known) return known;
  const kind = attachmentKind(name.slice(name.lastIndexOf("/") + 1));
  if (kind === "image") return "image";
  if (kind === "document" || kind === "archive" || kind === "pdf" || kind === "spreadsheet") return "external";
  return "text";
}
