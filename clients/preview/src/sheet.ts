import * as XLSX from "xlsx";
import { extensionOf } from "./kind";

export const SHEET_MAX_ROWS = 1000;
export const SHEET_MAX_COLS = 100;
export const SHEET_CAP_NOTE = "Showing first 1,000 rows × 100 columns";

export interface Sheet {
  name: string;
  /** Computed values as display text; never HTML. */
  rows: string[][];
  truncated: boolean;
}

/** Every sheet's computed values, at most 1,000 rows × 100 columns each. */
export function readSheet(bytes: Uint8Array, name: string): { sheets: Sheet[] } {
  const ext = extensionOf(name);
  const opts: XLSX.ParsingOptions = { sheetRows: SHEET_MAX_ROWS + 1, cellHTML: false, cellFormula: false };
  const wb =
    ext === "csv" || ext === "tsv"
      ? XLSX.read(new TextDecoder().decode(bytes), { ...opts, type: "string", raw: true, FS: ext === "tsv" ? "\t" : "," })
      : XLSX.read(bytes, { ...opts, type: "array" });
  const sheets = wb.SheetNames.map((sheetName): Sheet => {
    const ws = wb.Sheets[sheetName];
    const ref = ws?.["!ref"];
    const empty = !ws || !ref || Object.keys(ws).every((k) => k.startsWith("!"));
    if (empty) return { name: sheetName, rows: [], truncated: false };
    const full = XLSX.utils.decode_range(ws["!fullref"] ?? ref);
    const truncated = full.e.r - full.s.r + 1 > SHEET_MAX_ROWS || full.e.c - full.s.c + 1 > SHEET_MAX_COLS;
    const range = XLSX.utils.decode_range(ref);
    range.e.r = Math.min(range.e.r, range.s.r + SHEET_MAX_ROWS - 1);
    range.e.c = Math.min(range.e.c, range.s.c + SHEET_MAX_COLS - 1);
    const rows = XLSX.utils.sheet_to_json<unknown[]>(ws, { header: 1, raw: false, defval: "", blankrows: true, range });
    return { name: sheetName, rows: rows.map((r) => r.map((v) => String(v ?? ""))), truncated };
  });
  return { sheets: sheets.length ? sheets : [{ name: "Sheet1", rows: [], truncated: false }] };
}
