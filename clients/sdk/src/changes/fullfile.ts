// The whole file with its changes marked, rebuilt from a one-file patch and
// the file as it is now; and how the viewer folds its unchanged stretches.

import { CONTEXT } from "./diff";

export interface FileLine {
  kind: "add" | "del" | "ctx";
  text: string;
  /** The line's number before the change; null for an added line. */
  old: number | null;
  /** The line's number now; null for a removed line. */
  new: number | null;
}

interface Hunk {
  oldStart: number;
  newStart: number;
  body: string[];
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/** A file's text as lines: CRLF or LF, a last newline or not. */
function textLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split(/\r?\n/);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function hunks(patch: string): Hunk[] {
  const out: Hunk[] = [];
  const lines = patch.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const m = HUNK.exec(lines[i]!);
    if (!m) continue;
    let oldLeft = m[2] === undefined ? 1 : Number(m[2]);
    let newLeft = m[4] === undefined ? 1 : Number(m[4]);
    // A side with no lines names the line before the hunk.
    const hunk: Hunk = {
      oldStart: oldLeft === 0 ? Number(m[1]) + 1 : Number(m[1]),
      newStart: newLeft === 0 ? Number(m[3]) + 1 : Number(m[3]),
      body: [],
    };
    while ((oldLeft > 0 || newLeft > 0) && i + 1 < lines.length) {
      const line = lines[++i]!.replace(/\r$/, "");
      const mark = line[0];
      if (mark === "\\") continue;
      if (mark === "-") oldLeft--;
      else if (mark === "+") newLeft--;
      else {
        oldLeft--;
        newLeft--;
      }
      hunk.body.push(line);
    }
    out.push(hunk);
  }
  return out;
}

/**
 * Every line of the file, each marked added, removed or unchanged. The patch
 * says what changed; the content fills the unchanged lines between and after
 * its hunks. A deleted file (no content) is its patch's lines; a file with no
 * patch is all unchanged. Content read a moment after the patch may be
 * shorter than it expects: the gaps stop where the content does.
 */
export function fullFile(patch: string | null, content: string | null): FileLine[] {
  const now = content === null ? null : textLines(content);
  const out: FileLine[] = [];
  let oldNo = 1;
  let newNo = 1;
  const fill = (until: number) => {
    if (!now) return;
    for (; newNo < until && newNo <= now.length; newNo++, oldNo++) {
      out.push({ kind: "ctx", text: now[newNo - 1]!, old: oldNo, new: newNo });
    }
  };
  for (const h of patch ? hunks(patch) : []) {
    fill(h.newStart);
    oldNo = h.oldStart;
    newNo = h.newStart;
    for (const line of h.body) {
      const text = line.slice(1);
      if (line[0] === "-") out.push({ kind: "del", text, old: oldNo++, new: null });
      else if (line[0] === "+") out.push({ kind: "add", text, old: null, new: newNo++ });
      else out.push({ kind: "ctx", text, old: oldNo++, new: newNo++ });
    }
  }
  fill(Infinity);
  return out;
}

/** Shown lines `[from, to)`, or a bar folding `count` unchanged lines from `at`. */
export type FileSegment = { kind: "lines"; from: number; to: number } | { kind: "fold"; at: number; count: number };

/** Hidden runs shorter than this read better shown than folded. */
const MIN_FOLD = 4;

/**
 * The file as the viewer draws it: each change keeps `CONTEXT` unchanged
 * lines around it, and longer unchanged stretches fold. `opened` maps a
 * fold's first line to how many of its lines have been revealed, from the
 * top. A file with no changes folds nothing.
 */
export function fileSegments(lines: readonly FileLine[], opened: Readonly<Record<number, number>>): FileSegment[] {
  const near = new Array<boolean>(lines.length).fill(false);
  let changed = false;
  lines.forEach((l, i) => {
    if (l.kind === "ctx") return;
    changed = true;
    for (let j = Math.max(0, i - CONTEXT); j <= Math.min(lines.length - 1, i + CONTEXT); j++) near[j] = true;
  });
  if (!changed) return lines.length > 0 ? [{ kind: "lines", from: 0, to: lines.length }] : [];

  const out: FileSegment[] = [];
  const show = (from: number, to: number) => {
    if (from >= to) return;
    const last = out[out.length - 1];
    if (last?.kind === "lines" && last.to === from) last.to = to;
    else out.push({ kind: "lines", from, to });
  };
  let i = 0;
  while (i < lines.length) {
    if (near[i]) {
      show(i, i + 1);
      i++;
      continue;
    }
    const at = i;
    while (i < lines.length && !near[i]) i++;
    const shown = Math.min(i - at, opened[at] ?? 0);
    const hidden = i - at - shown;
    if (hidden < MIN_FOLD) show(at, i);
    else {
      show(at, at + shown);
      out.push({ kind: "fold", at, count: hidden });
    }
  }
  return out;
}

/** How many lines one click on a fold reveals: all of a short one, else 100. */
export function revealStep(hidden: number): number {
  return hidden <= 200 ? hidden : 100;
}

export function isMarkdown(path: string): boolean {
  return /\.(md|markdown|mdx)$/i.test(path);
}
