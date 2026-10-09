import { describe, expect, it } from "vitest";
import { asksBeforeOpening, largeFileMessage, LARGE_FILE_BYTES, openTarget, viewFor } from "../src/preview/model";

describe("mobile previews", () => {
  it("routes text to the File screen and everything else to Preview", () => {
    expect(openTarget("src/main.rs")).toBe("File");
    expect(openTarget("notes.md")).toBe("File");
    for (const n of ["a.pdf", "a.html", "a.xlsx", "a.docx", "a.png", "a.zip", "a.pptx"]) expect(openTarget(n), n).toBe("Preview");
  });

  it("chooses the renderer per platform", () => {
    expect(viewFor("a.html", "ios")).toBe("html");
    expect(viewFor("a.html", "android")).toBe("html");
    for (const n of ["a.pdf", "a.xlsx", "a.docx", "a.png", "a.pptx", "a.doc", "a.key", "a.numbers", "a.pages", "a.csv"]) {
      expect(viewFor(n, "ios"), n).toBe("native");
    }
    for (const n of ["a.pdf", "a.xlsx", "a.xls", "a.csv", "a.docx", "a.png"]) expect(viewFor(n, "android"), n).toBe("renderer");
    for (const n of ["a.pptx", "a.doc", "a.zip"]) expect(viewFor(n, "android"), n).toBe("share");
    expect(viewFor("a.zip", "ios")).toBe("share");
    expect(viewFor("a.md", "ios")).toBe("text");
  });

  it("asks before opening a large file, except PDF and HTML", () => {
    expect(asksBeforeOpening("a.xlsx", LARGE_FILE_BYTES + 1)).toBe(true);
    expect(asksBeforeOpening("a.xlsx", LARGE_FILE_BYTES)).toBe(false);
    expect(asksBeforeOpening("a.png", 412 * 1024 * 1024)).toBe(true);
    expect(asksBeforeOpening("a.pdf", 412 * 1024 * 1024)).toBe(false);
    expect(asksBeforeOpening("a.html", 412 * 1024 * 1024)).toBe(false);
    expect(largeFileMessage(412 * 1024 * 1024)).toBe("Large file (412 MB). Opening it may be slow or close the app.");
  });
});
