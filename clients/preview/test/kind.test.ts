import { describe, expect, it } from "vitest";
import { imageMime, newPreviewId, opensInPreview, previewKind } from "../src";

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

  it("opens rendered kinds in the preview, not text or external", () => {
    for (const name of ["a.html", "a.pdf", "a.csv", "a.docx", "a.png", "a.avif"]) expect(opensInPreview(name), name).toBe(true);
    for (const name of ["a.md", "a.rs", "a.pptx", "a.zip"]) expect(opensInPreview(name), name).toBe(false);
  });

  it("gives image MIME types by extension", () => {
    expect(imageMime("logo.SVG")).toBe("image/svg+xml");
    expect(imageMime(".zwiim/images/ab.jpg")).toBe("image/jpeg");
    expect(imageMime("a.tif")).toBe("image/tiff");
    expect(imageMime("a.bin")).toBe("");
  });

  it("makes 32-hex preview ids", () => {
    const a = newPreviewId();
    expect(a).toMatch(/^[0-9a-f]{32}$/);
    expect(newPreviewId()).not.toBe(a);
  });
});
