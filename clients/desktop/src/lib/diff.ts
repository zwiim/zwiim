// A file change read for display: from a unified diff (a transcript's edit
// result, a live edit's content) or from an edit's own old and new text.

export interface DiffLine {
  kind: "add" | "del" | "ctx" | "hunk";
  text: string;
  /** Line numbers in the old and new file, when the diff says. */
  old?: number;
  new?: number;
}

export interface FileDiff {
  path: string | null;
  lines: DiffLine[];
  added: number;
  removed: number;
}

/** Lines kept around a change when the diff does not say where it is. */
const CONTEXT = 3;

const splitLines = (text: string) => (text === "" ? [] : text.replace(/\n$/, "").split("\n"));

function counted(path: string | null, lines: DiffLine[]): FileDiff {
  return {
    path,
    lines,
    added: lines.filter((l) => l.kind === "add").length,
    removed: lines.filter((l) => l.kind === "del").length,
  };
}

/** One change without line numbers: what both sides share is context. */
function changeLines(oldLines: string[], newLines: string[]): DiffLine[] {
  let head = 0;
  while (head < oldLines.length && head < newLines.length && oldLines[head] === newLines[head]) head++;
  let tail = 0;
  while (
    tail < oldLines.length - head &&
    tail < newLines.length - head &&
    oldLines[oldLines.length - 1 - tail] === newLines[newLines.length - 1 - tail]
  )
    tail++;
  const ctx = (text: string): DiffLine => ({ kind: "ctx", text });
  return [
    ...oldLines.slice(Math.max(0, head - CONTEXT), head).map(ctx),
    ...oldLines.slice(head, oldLines.length - tail).map((text): DiffLine => ({ kind: "del", text })),
    ...newLines.slice(head, newLines.length - tail).map((text): DiffLine => ({ kind: "add", text })),
    ...oldLines.slice(oldLines.length - tail, oldLines.length - tail + CONTEXT).map(ctx),
  ];
}

/** An edit from its old and new text: one change, or several in one file. */
export function textDiff(path: string | null, changes: { old: string; new: string }[]): FileDiff {
  const lines = changes.flatMap((c, i) => [
    ...(i > 0 ? [{ kind: "hunk" as const, text: "" }] : []),
    ...changeLines(splitLines(c.old), splitLines(c.new)),
  ]);
  return counted(path, lines);
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@ ?(.*)$/;

/** The files in a unified diff; null when the text is not one. */
export function parseUnifiedDiff(text: string): FileDiff[] | null {
  const lines = text.split("\n");
  const files: FileDiff[] = [];
  let i = 0;
  while (i < lines.length) {
    if (!(lines[i]!.startsWith("--- ") && lines[i + 1]?.startsWith("+++ "))) {
      i++;
      continue;
    }
    const from = lines[i]!.slice(4).trim();
    const to = lines[i + 1]!.slice(4).trim();
    // Git-style `a/` and `b/` prefixes are not part of the path.
    const git = (from.startsWith("a/") || from === "/dev/null") && (to.startsWith("b/") || to === "/dev/null");
    const strip = (p: string) => (git && p !== "/dev/null" ? p.slice(2) : p);
    const path = to === "/dev/null" ? strip(from) : strip(to);
    i += 2;
    const out: DiffLine[] = [];
    const dels: string[] = [];
    const adds: string[] = [];
    let hunked = false;
    let oldNo = 0;
    let newNo = 0;
    for (; i < lines.length; i++) {
      const line = lines[i]!;
      if (line.startsWith("--- ") && lines[i + 1]?.startsWith("+++ ")) break;
      const hunk = HUNK.exec(line);
      if (hunk) {
        hunked = true;
        oldNo = Number(hunk[1]);
        newNo = Number(hunk[2]);
        out.push({ kind: "hunk", text: hunk[3] ?? "" });
      } else if (line.startsWith("\\")) {
        // "\ No newline at end of file"
      } else if (!hunked) {
        // Without hunks (an edit's content as the agent sends it): all old
        // lines, then all new ones.
        if (line.startsWith("-")) dels.push(line.slice(1));
        else if (line.startsWith("+")) adds.push(line.slice(1));
      } else if (line.startsWith("+")) out.push({ kind: "add", text: line.slice(1), new: newNo++ });
      else if (line.startsWith("-")) out.push({ kind: "del", text: line.slice(1), old: oldNo++ });
      else if (line.startsWith(" ") || (line === "" && i < lines.length - 1))
        out.push({ kind: "ctx", text: line.slice(1), old: oldNo++, new: newNo++ });
    }
    // The first hunk needs no separator above it.
    if (out[0]?.kind === "hunk") out.shift();
    files.push(counted(path, hunked ? out : changeLines(dels, adds)));
  }
  return files.length > 0 ? files : null;
}

const PATCH_FILE = /^\*\*\* (Add|Update|Delete) File: (.+)$/;

type PatchOp = "add" | "update" | "delete";

/**
 * The files in a Codex patch (`*** Begin Patch` … `*** End Patch`), with how
 * each is changed; null when the text is not one. Its hunks carry no line
 * numbers.
 */
export function parseCodexPatch(text: string): { diff: FileDiff; op: PatchOp }[] | null {
  const lines = text.split("\n");
  if (!lines.some((l) => l.trim() === "*** Begin Patch")) return null;
  const files: { diff: FileDiff; op: PatchOp }[] = [];
  let file: { path: string; op: PatchOp; lines: DiffLine[] } | null = null;
  const flush = () => {
    if (!file) return;
    // The first hunk needs no separator above it.
    if (file.lines[0]?.kind === "hunk") file.lines.shift();
    files.push({ diff: counted(file.path, file.lines), op: file.op });
    file = null;
  };
  for (const line of lines) {
    const header = PATCH_FILE.exec(line);
    if (header) {
      flush();
      file = { path: header[2]!.trim(), op: header[1]!.toLowerCase() as PatchOp, lines: [] };
    } else if (!file) {
      continue;
    } else if (line.startsWith("*** Move to: ")) {
      file.path = line.slice("*** Move to: ".length).trim();
    } else if (line.startsWith("*** ")) {
      // "*** End Patch", "*** End of File"
    } else if (line.startsWith("@@")) file.lines.push({ kind: "hunk", text: line.slice(2).trim() });
    else if (line.startsWith("+")) file.lines.push({ kind: "add", text: line.slice(1) });
    else if (line.startsWith("-")) file.lines.push({ kind: "del", text: line.slice(1) });
    else if (line.startsWith(" ")) file.lines.push({ kind: "ctx", text: line.slice(1) });
  }
  flush();
  return files.length > 0 ? files : null;
}

export const basename = (path: string) => path.replace(/\/+$/, "").split("/").pop() || path;
