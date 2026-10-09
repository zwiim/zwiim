import { describe, expect, it } from "vitest";
import { newPreviewId, previewKind } from "../src";

describe("previewKind", () => {
  it("sorts names by extension, case-insensitively", () => {
    const table: Record<string, string[]> = {
      html: ["page.html", "docs/INDEX.HTM"],
      pdf: ["spec.pdf", "a/b/Spec.PDF"],
      sheet: ["budget.xlsx", "macro.xlsm", "old.xls", "data.csv", "data.tsv"],
      word: ["memo.docx"],
      image: ["shot.png", "photo.JPG", "pic.webp", "logo.svg"],
      text: ["notes.md", "main.rs", "Makefile", ".env", "README", "data.json"],
      external: ["old.doc", "deck.ppt", "deck.pptx", "sheet.ods", "bundle.zip", "letter.odt", "slides.key", "report.pages", "budget.numbers"],
    };
    for (const [kind, names] of Object.entries(table)) {
      for (const name of names) expect(previewKind(name), name).toBe(kind);
    }
  });

  it("makes 32-hex preview ids", () => {
    const a = newPreviewId();
    expect(a).toMatch(/^[0-9a-f]{32}$/);
    expect(newPreviewId()).not.toBe(a);
  });
});
