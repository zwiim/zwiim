// @vitest-environment jsdom
import JSZip from "jszip";
import { describe, expect, it } from "vitest";
import { renderWord, sanitizeWordHtml } from "../src/word";

async function docx(bodyXml: string): Promise<Uint8Array> {
  const zip = new JSZip();
  zip.file("[Content_Types].xml", `<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>`);
  zip.file("_rels/.rels", `<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>`);
  zip.file("word/document.xml", `<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>${bodyXml}</w:body></w:document>`);
  return new Uint8Array(await zip.generateAsync({ type: "uint8array" }));
}

describe("word", () => {
  it("renders paragraphs and headings", async () => {
    const html = await renderWord(await docx(`<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Plan</w:t></w:r></w:p><w:p><w:r><w:t>Ship it &lt;now&gt;</w:t></w:r></w:p>`));
    expect(html).toContain("<h1>Plan</h1>");
    expect(html).toContain("<p>Ship it &lt;now&gt;</p>");
  });

  it("strips scripts, handlers, javascript: URLs and embedded content", () => {
    const out = sanitizeWordHtml(
      `<p onclick="x()">a</p><script>alert(1)</script><a href="javascript:alert(1)">l</a><iframe src="https://x"></iframe><object></object><embed><form><input></form><style>p{}</style><img src="data:image/png;base64,AA==">`,
    );
    expect(out).not.toMatch(/onclick|<script|javascript:|<iframe|<object|<embed|<form|<style/);
    expect(out).toContain(`<img src="data:image/png;base64,AA==">`);
  });

  it("rejects a file that is not a docx", async () => {
    await expect(renderWord(new TextEncoder().encode("not a zip"))).rejects.toThrow();
  });
});
