import { describe, expect, it } from "vitest";
import {
  addRow,
  deleteRow,
  delimitedEditable,
  delimitedWidth,
  delimiterFor,
  EDIT_MAX_COLS,
  EDIT_MAX_ROWS,
  parseDelimited,
  serializeDelimited,
  setCell,
} from "../src/delimited";

const cells = (text: string, d: "," | "\t" = ",") => parseDelimited(text, d).rows.map((r) => r.cells);

describe("delimiterFor", () => {
  it("knows csv and tsv only", () => {
    expect(delimiterFor("a/b.CSV")).toBe(",");
    expect(delimiterFor("b.tsv")).toBe("\t");
    expect(delimiterFor("b.xlsx")).toBeNull();
    expect(delimiterFor("csv")).toBeNull();
  });
});

describe("parseDelimited", () => {
  it("reads quoted values with commas, quotes and line breaks", () => {
    expect(cells('name,note\n"Smith, J","say ""hi""\nthen go"\n')).toEqual([
      ["name", "note"],
      ["Smith, J", 'say "hi"\nthen go'],
    ]);
  });

  it("reads tsv, ragged rows and blank lines", () => {
    expect(cells("a\tb,c\n1\n\n", "\t")).toEqual([["a", "b,c"], ["1"], [""]]);
    expect(cells("")).toEqual([]);
  });

  it("treats a quote inside a value as text", () => {
    expect(cells('5" pipe,x\n')).toEqual([['5" pipe', "x"]]);
  });
});

describe("serializeDelimited", () => {
  it.each([
    ["LF", 'a,b\n"x, y",2\n'],
    ["CRLF", "a,b\r\n1,2\r\n"],
    ["no final newline", "a,b\n1,2"],
    ["mixed endings", "a\r\nb\nc"],
    ["BOM", "﻿name\nrésumé\n"],
    ["odd quoting kept", '"a",b\n"1" ,2\n'],
    ["empty", ""],
  ])("writes an untouched %s file back byte for byte", (_, text) => {
    expect(serializeDelimited(parseDelimited(text, ","))).toBe(text);
  });

  it("rewrites only the edited row, quoting where needed", () => {
    const t = parseDelimited('"a" ,b\r\n1,2\r\n3,4\r\n', ",");
    const edited = setCell(t, 1, 1, 'say "hi", then\ngo');
    expect(serializeDelimited(edited)).toBe('"a" ,b\r\n1,"say ""hi"", then\ngo"\r\n3,4\r\n');
    expect(cells(serializeDelimited(edited))[1]).toEqual(["1", 'say "hi", then\ngo']);
  });

  it("keeps the BOM when the first row is edited", () => {
    const t = parseDelimited("﻿name\nx\n", ",");
    expect(serializeDelimited(setCell(t, 0, 0, "who"))).toBe("﻿who\nx\n");
  });

  it("quotes tabs in tsv, not commas", () => {
    const t = parseDelimited("a\tb\n", "\t");
    expect(serializeDelimited(setCell(t, 0, 1, "x,y\tz"))).toBe('a\t"x,y\tz"\n');
  });

  it("pads a short row when a cell past its end is set", () => {
    const t = parseDelimited("a,b,c\n1\n", ",");
    expect(serializeDelimited(setCell(t, 1, 2, "z"))).toBe("a,b,c\n1,,z\n");
  });

  it("leaves the row untouched when a cell is set to what it was", () => {
    const t = parseDelimited('"a" ,b\n', ",");
    expect(setCell(t, 0, 1, "b")).toBe(t);
  });

  it("adds rows in the file's line ending and keeps the final newline as it was", () => {
    expect(serializeDelimited(addRow(parseDelimited("a,b\r\n1,2\r\n", ",")))).toBe("a,b\r\n1,2\r\n,\r\n");
    const added = setCell(addRow(parseDelimited("a,b\n1,2", ",")), 2, 0, "3");
    expect(serializeDelimited(added)).toBe("a,b\n1,2\n3,");
    expect(serializeDelimited(setCell(addRow(parseDelimited("", ",")), 0, 0, "x"))).toBe("x");
  });

  it("deletes rows, the last one included", () => {
    const t = parseDelimited("a\nb\nc\n", ",");
    expect(serializeDelimited(deleteRow(t, 1))).toBe("a\nc\n");
    expect(serializeDelimited(deleteRow(parseDelimited("a\nb\nc", ","), 2))).toBe("a\nb");
  });
});

describe("delimitedEditable", () => {
  it("allows at most 1,000 rows × 100 columns", () => {
    const wide = Array.from({ length: EDIT_MAX_COLS }, () => "x").join(",");
    expect(delimitedWidth(parseDelimited(`${wide}\n`, ","))).toBe(EDIT_MAX_COLS);
    expect(delimitedEditable(parseDelimited(`${wide}\n`, ","))).toBe(true);
    expect(delimitedEditable(parseDelimited(`${wide},x\n`, ","))).toBe(false);
    expect(delimitedEditable(parseDelimited("x\n".repeat(EDIT_MAX_ROWS), ","))).toBe(true);
    expect(delimitedEditable(parseDelimited("x\n".repeat(EDIT_MAX_ROWS + 1), ","))).toBe(false);
  });
});
