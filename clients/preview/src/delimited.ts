import { extensionOf } from "./kind";

// CSV/TSV for editing. Unlike the sheet preview this keeps each row's source
// text, so a save rewrites only the rows the user changed: untouched rows,
// line endings, a BOM and the final newline go back byte for byte.

export type Delimiter = "," | "\t";

/** Largest table that may be edited; matches the sheet preview's cap. */
export const EDIT_MAX_ROWS = 1000;
export const EDIT_MAX_COLS = 100;

export interface DelimitedRow {
  cells: string[];
  /** The row's source text; `null` once edited, or for a new row. */
  raw: string | null;
  /** The line break after the row in the source; `""` for none. */
  end: string;
}

export interface DelimitedTable {
  delimiter: Delimiter;
  bom: boolean;
  /** Line break for new rows: the file's first one, else `\n`. */
  eol: string;
  trailingNewline: boolean;
  rows: DelimitedRow[];
}

/** `,` for `.csv`, a tab for `.tsv`, else `null`. */
export function delimiterFor(name: string): Delimiter | null {
  const ext = extensionOf(name);
  return ext === "csv" ? "," : ext === "tsv" ? "\t" : null;
}

export function parseDelimited(source: string, delimiter: Delimiter): DelimitedTable {
  const bom = source.startsWith("﻿");
  const text = bom ? source.slice(1) : source;
  const rows: DelimitedRow[] = [];
  let i = 0;
  while (i < text.length) {
    const start = i;
    const cells: string[] = [];
    let cell = "";
    let fieldStart = true;
    let quoted = false;
    let end = "";
    for (; i < text.length; i++) {
      const ch = text[i]!;
      if (quoted) {
        if (ch !== '"') cell += ch;
        else if (text[i + 1] === '"') {
          cell += '"';
          i++;
        } else quoted = false;
        continue;
      }
      if (ch === '"' && fieldStart) {
        quoted = true;
        fieldStart = false;
      } else if (ch === delimiter) {
        cells.push(cell);
        cell = "";
        fieldStart = true;
      } else if (ch === "\n" || ch === "\r") {
        end = ch === "\r" && text[i + 1] === "\n" ? "\r\n" : ch;
        break;
      } else {
        cell += ch;
        fieldStart = false;
      }
    }
    cells.push(cell);
    rows.push({ cells, raw: text.slice(start, i), end });
    i += end.length;
  }
  return {
    delimiter,
    bom,
    eol: rows.find((r) => r.end)?.end ?? "\n",
    trailingNewline: (rows.at(-1)?.end ?? "") !== "",
    rows,
  };
}

function encodeCell(value: string, delimiter: Delimiter): string {
  const quote = value.includes(delimiter) || /["\r\n]/.test(value);
  return quote ? `"${value.replaceAll('"', '""')}"` : value;
}

export function serializeDelimited(table: DelimitedTable): string {
  const last = table.rows.length - 1;
  const body = table.rows
    .map((row, i) => {
      const text = row.raw ?? row.cells.map((c) => encodeCell(c, table.delimiter)).join(table.delimiter);
      const end = i < last || table.trailingNewline ? row.end || table.eol : "";
      return text + end;
    })
    .join("");
  return (table.bom ? "﻿" : "") + body;
}

/** The widest row's cell count. */
export function delimitedWidth(table: DelimitedTable): number {
  return table.rows.reduce((w, r) => Math.max(w, r.cells.length), 0);
}

export function delimitedEditable(table: DelimitedTable): boolean {
  return table.rows.length <= EDIT_MAX_ROWS && delimitedWidth(table) <= EDIT_MAX_COLS;
}

/** Set one cell, padding a short row; the same table when nothing changes. */
export function setCell(table: DelimitedTable, row: number, col: number, value: string): DelimitedTable {
  const old = table.rows[row];
  if (!old || (old.cells[col] ?? "") === value) return table;
  const cells = [...old.cells];
  while (cells.length <= col) cells.push("");
  cells[col] = value;
  const rows = [...table.rows];
  rows[row] = { ...old, cells, raw: null };
  return { ...table, rows };
}

/** Append an empty row as wide as the table. */
export function addRow(table: DelimitedTable): DelimitedTable {
  const cells = Array.from({ length: Math.max(1, delimitedWidth(table)) }, () => "");
  return { ...table, rows: [...table.rows, { cells, raw: null, end: "" }] };
}

export function deleteRow(table: DelimitedTable, row: number): DelimitedTable {
  return { ...table, rows: table.rows.filter((_, i) => i !== row) };
}
