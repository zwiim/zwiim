/**
 * The workspace path a preview URL path names, or null when it cannot name
 * one. Query and fragment are dropped, each segment percent-decoded, `.` and
 * `..` resolved without leaving the root; a directory path means its
 * index.html.
 */
export function resolvePreviewPath(urlPath: string): string | null {
  const bare = urlPath.split(/[?#]/, 1)[0] ?? "";
  const out: string[] = [];
  const raw = bare.split("/");
  for (const segment of raw) {
    let part: string;
    try {
      part = decodeURIComponent(segment);
    } catch {
      return null;
    }
    if (/[/\\\u0000]/.test(part)) return null;
    if (part === "" || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  const last = raw[raw.length - 1];
  const dirLike = out.length === 0 || last === "" || last === "." || last === ".." ||
    (() => { try { const d = decodeURIComponent(last ?? ""); return d === "." || d === ".."; } catch { return false; } })();
  if (dirLike) out.push("index.html");
  return out.join("/");
}

/** A workspace path as a URL path (no leading slash), each segment percent-encoded. */
export function encodePreviewPath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}
