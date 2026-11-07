import { describe, expect, it } from "vitest";
import { encodePreviewPath, previewMayRead, resolvePreviewPath } from "../src";

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

describe("previewMayRead", () => {
  it("keeps a page out of hidden files and folders", () => {
    for (const path of [".env", ".git/config", "a/.ssh/id_rsa", ".env.local"]) {
      expect(previewMayRead(path, "docs/report.html")).toBe(false);
    }
    expect(previewMayRead(".env", "")).toBe(false);
  });

  it("leaves everything else alone", () => {
    for (const path of ["docs/report.html", "a.b/c.css", "img/logo.png", "index.html"]) {
      expect(previewMayRead(path, "docs/report.html")).toBe(true);
    }
  });

  it("lets a page in a hidden folder load its own files, and nothing further", () => {
    const entry = ".zwiim/images/report.html";
    expect(previewMayRead(entry, entry)).toBe(true);
    expect(previewMayRead(".zwiim/images/logo.png", entry)).toBe(true);
    expect(previewMayRead(".zwiim/images/css/a.css", entry)).toBe(true);
    expect(previewMayRead(".zwiim/images/.secret", entry)).toBe(false);
    expect(previewMayRead(".zwiim/other.png", entry)).toBe(false);
    expect(previewMayRead(".env", entry)).toBe(false);
    expect(previewMayRead("docs/a.css", entry)).toBe(true);
  });

  it("lets a hidden page itself open, without opening its folder", () => {
    expect(previewMayRead(".report.html", ".report.html")).toBe(true);
    expect(previewMayRead(".env", ".report.html")).toBe(false);
  });
});
