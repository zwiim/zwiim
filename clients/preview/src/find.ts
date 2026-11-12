/** How a find query matches. */
export interface FindOptions {
  matchCase?: boolean;
  /** Only where the query is not inside a longer word. */
  wholeWord?: boolean;
}

/** A match in a string: `text.slice(start, end)`. */
export interface FindMatch {
  start: number;
  end: number;
}

const WORD = /[\p{L}\p{N}_]/u;

/**
 * Every place `query` occurs in `text`, in order and not overlapping. The
 * query is plain text, never a pattern; an empty one matches nothing.
 */
export function findMatches(text: string, query: string, options: FindOptions = {}): FindMatch[] {
  if (!query) return [];
  const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(escaped, options.matchCase ? "gu" : "giu");
  const out: FindMatch[] = [];
  for (let m = re.exec(text); m; m = re.exec(text)) {
    const start = m.index;
    const end = start + m[0].length;
    if (options.wholeWord && (WORD.test(text[start - 1] ?? "") || WORD.test(text[end] ?? ""))) {
      re.lastIndex = start + 1;
      continue;
    }
    out.push({ start, end });
  }
  return out;
}

/** `text` with each of `matches` (from `findMatches` on it) replaced by `replacement`. */
export function replaceMatches(text: string, matches: readonly FindMatch[], replacement: string): string {
  let out = "";
  let at = 0;
  for (const m of matches) {
    out += text.slice(at, m.start) + replacement;
    at = m.end;
  }
  return out + text.slice(at);
}
