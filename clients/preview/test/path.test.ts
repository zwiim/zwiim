import { describe, expect, it } from "vitest";
import { encodePreviewPath, resolvePreviewPath } from "../src";

describe("resolvePreviewPath", () => {
  it("maps URL paths to workspace paths", () => {
    expect(resolvePreviewPath("/docs/report.html")).toBe("docs/report.html");
    expect(resolvePreviewPath("/docs/./img/../style.css?v=2#top")).toBe("docs/style.css");
    expect(resolvePreviewPath("/../../etc/passwd")).toBe("etc/passwd");
    expect(resolvePreviewPath("/a/../../b")).toBe("b");
    expect(resolvePreviewPath("/")).toBe("index.html");
    expect(resolvePreviewPath("")).toBe("index.html");
    expect(resolvePreviewPath("/docs/")).toBe("docs/index.html");
    expect(resolvePreviewPath("/docs/..")).toBe("index.html");
    expect(resolvePreviewPath("//a///b")).toBe("a/b");
  });

  it("percent-decodes each segment and refuses encoded separators", () => {
    expect(resolvePreviewPath("/img/photo%20%231.png")).toBe("img/photo #1.png");
    expect(resolvePreviewPath("/r%C3%A9sum%C3%A9.pdf")).toBe("résumé.pdf");
    expect(resolvePreviewPath("/a%2Fb")).toBeNull();
    expect(resolvePreviewPath("/a%5Cb")).toBeNull();
    expect(resolvePreviewPath("/a%00b")).toBeNull();
    expect(resolvePreviewPath("/%E0%A4%A")).toBeNull();
    expect(resolvePreviewPath("/%2E%2E/x")).toBe("x");
  });

  it("round-trips any workspace path through encodePreviewPath", () => {
    for (const p of ["docs/my report.html", "img/photo #1.png", "a/b?c.html", "résumé.pdf", "100%.css"]) {
      expect(resolvePreviewPath("/" + encodePreviewPath(p))).toBe(p);
    }
  });
});
