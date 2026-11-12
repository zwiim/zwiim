import { setCell, type DelimitedTable } from "./delimited";

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

/** A match in a table cell: `rows[r].cells[c].slice(start, end)`. */
export interface CellMatch extends FindMatch {
  r: number;
  c: number;
}

/** Every match in the table's cells, row by row; a match never runs from one cell into the next. */
export function findInTable(table: DelimitedTable, query: string, options: FindOptions = {}): CellMatch[] {
  const out: CellMatch[] = [];
  table.rows.forEach((row, r) =>
    row.cells.forEach((cell, c) => {
      for (const m of findMatches(cell, query, options)) out.push({ r, c, ...m });
    }),
  );
  return out;
}

/** The table with each of `matches` (from `findInTable` on it) replaced; only rows it changes are rewritten. */
export function replaceInTable(table: DelimitedTable, matches: readonly CellMatch[], replacement: string): DelimitedTable {
  const cells = new Map<string, CellMatch[]>();
  for (const m of matches) {
    const key = `${m.r},${m.c}`;
    cells.set(key, [...(cells.get(key) ?? []), m]);
  }
  let next = table;
  for (const ms of cells.values()) {
    const { r, c } = ms[0]!;
    next = setCell(next, r, c, replaceMatches(table.rows[r]!.cells[c] ?? "", ms, replacement));
  }
  return next;
}
