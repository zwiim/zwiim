// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { mountSheets, rendererParams } from "../renderer/dom";

describe("renderer page", () => {
  it("reads its query", () => {
    expect(rendererParams("?id=0123456789abcdef0123456789abcdef&kind=pdf&name=a%20b.pdf")).toEqual({
      id: "0123456789abcdef0123456789abcdef", kind: "pdf", name: "a b.pdf",
    });
    expect(rendererParams("?id=../x&kind=pdf&name=a")).toBeNull();
    expect(rendererParams("?kind=pdf")).toBeNull();
  });

  it("shows sheet values as text, one tab per sheet", () => {
    const box = document.createElement("div");
    mountSheets(box, [
      { name: "One", rows: [["<img src=x onerror=alert(1)>"]], truncated: false },
      { name: "Two", rows: [["2"]], truncated: true },
    ]);
    expect(box.querySelector("img")).toBeNull();
    expect(box.querySelector("td")!.textContent).toBe("<img src=x onerror=alert(1)>");
    (box.querySelectorAll("[role=tab]")[1] as HTMLElement).click();
    expect(box.textContent).toContain("Showing first 1,000 rows × 100 columns");
  });
});
