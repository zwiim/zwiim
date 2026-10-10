// Where a workspace file opens on the phone, and how.
import { formatSize } from "@zwiim/sdk";
import { extensionOf, opensInPreview, previewKind } from "@zwiim/preview";

export const LARGE_FILE_BYTES = 50 * 1024 * 1024;
export type Platform = "ios" | "android";
export type PreviewView = "html" | "native" | "renderer" | "text" | "share";

/** Office and iWork files WebKit renders itself on iOS. */
const IOS_NATIVE_EXTERNAL = new Set(["doc", "ppt", "pptx", "pages", "numbers", "key"]);

export function openTarget(name: string): "File" | "Preview" {
  return previewKind(name) === "text" ? "File" : "Preview";
}

/** Where a file the agent wrote opens: rendered kinds in Preview, the rest as a diff (repos) or the file. */
export function fileScreen(path: string, isGitRepo: boolean): "Diff" | "File" | "Preview" {
  if (opensInPreview(path)) return "Preview";
  return isGitRepo ? "Diff" : "File";
}

export function viewFor(name: string, platform: Platform): PreviewView {
  const kind = previewKind(name);
  if (kind === "html") return "html";
  if (kind === "text") return "text";
  if (kind === "external") return platform === "ios" && IOS_NATIVE_EXTERNAL.has(extensionOf(name)) ? "native" : "share";
  return platform === "ios" ? "native" : "renderer";
}

/** PDF and HTML load in pieces; anything else this big is loaded whole by the renderer. */
export function asksBeforeOpening(name: string, size: number): boolean {
  const kind = previewKind(name);
  return size > LARGE_FILE_BYTES && kind !== "pdf" && kind !== "html";
}

export function largeFileMessage(size: number): string {
  return `Large file (${formatSize(size)}). Opening it may be slow or close the app.`;
}
