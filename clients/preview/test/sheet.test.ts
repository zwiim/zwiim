import { describe, expect, it } from "vitest";
import * as XLSX from "xlsx";
import { readSheet, SHEET_MAX_COLS, SHEET_MAX_ROWS } from "../src/sheet";

function workbook(sheets: Record<string, unknown[][]>, bookType: "xlsx" | "biff8"): Uint8Array {
  const wb = XLSX.utils.book_new();
  for (const [name, rows] of Object.entries(sheets)) XLSX.utils.book_append_sheet(wb, XLSX.utils.aoa_to_sheet(rows), name);
  return new Uint8Array(XLSX.write(wb, { type: "array", bookType }));
}

describe("readSheet", () => {
  it("reads every sheet of xlsx and xls as text", () => {
    for (const [type, name] of [["xlsx", "b.xlsx"], ["biff8", "b.xls"]] as const) {
      const bytes = workbook({ Q1: [["item", "cost"], ["pens", 1.5]], Notes: [["<b>hi</b>"]] }, type);
      expect(readSheet(bytes, name)).toEqual({
        sheets: [
          { name: "Q1", rows: [["item", "cost"], ["pens", "1.5"]], truncated: false },
          { name: "Notes", rows: [["<b>hi</b>"]], truncated: false },
        ],
      });
    }
  });

  it("cuts a sheet at 1,000 rows × 100 columns", () => {
    const wide = Array.from({ length: SHEET_MAX_ROWS + 5 }, (_, r) => Array.from({ length: SHEET_MAX_COLS + 3 }, (_, c) => `${r}:${c}`));
    const [sheet] = readSheet(workbook({ Big: wide }, "xlsx"), "big.xlsx").sheets;
    expect(sheet!.truncated).toBe(true);
    expect(sheet!.rows.length).toBe(SHEET_MAX_ROWS);
    expect(sheet!.rows[0]!.length).toBe(SHEET_MAX_COLS);
    expect(sheet!.rows[999]![99]).toBe("999:99");
  });

  it("reads csv and tsv as written, BOM and unicode included", () => {
    const csv = new TextEncoder().encode("﻿name,amount\nrésumé,007\n");
    expect(readSheet(csv, "data.csv").sheets).toEqual([{ name: "Sheet1", rows: [["name", "amount"], ["résumé", "007"]], truncated: false }]);
    const tsv = new TextEncoder().encode("a\tb,c\n1\t2\n");
    expect(readSheet(tsv, "data.tsv").sheets[0]!.rows).toEqual([["a", "b,c"], ["1", "2"]]);
    const rows = "x\n".repeat(SHEET_MAX_ROWS + 1);
    expect(readSheet(new TextEncoder().encode(rows), "long.csv").sheets[0]!.truncated).toBe(true);
  });

  it("shows an empty csv as one empty sheet", () => {
    expect(readSheet(new Uint8Array(), "empty.csv").sheets).toEqual([{ name: "Sheet1", rows: [], truncated: false }]);
  });
});
