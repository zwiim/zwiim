import { describe, expect, it } from "vitest";
import { previewUrl } from "@/lib/preview-host";

describe("previewUrl", () => {
  it("encodes each path segment for the platform's origin", () => {
    const id = "0123456789abcdef0123456789abcdef";
    expect(previewUrl(id, "docs/my report #1.html", false)).toBe(`zwiim-preview://${id}/docs/my%20report%20%231.html`);
    expect(previewUrl(id, "résumé.html", true)).toBe(`http://zwiim-preview.${id}/r%C3%A9sum%C3%A9.html`);
  });
});
