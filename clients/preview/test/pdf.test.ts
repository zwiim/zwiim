import { describe, expect, it } from "vitest";
import { ZwiimClientError } from "@zwiim/sdk";
import { openPdfSource, pdfDocumentParams, PDF_INITIAL_BYTES } from "../src/pdf-source";

const assets = { workerSrc: "/w.mjs", cMapUrl: "/cmaps/", standardFontDataUrl: "/fonts/", wasmUrl: "/wasm/" };

describe("pdf", () => {
  it("chooses url, whole data, or ranges", () => {
    const common = { cMapUrl: "/cmaps/", cMapPacked: true, standardFontDataUrl: "/fonts/", wasmUrl: "/wasm/", isEvalSupported: false, disableAutoFetch: true, disableStream: true, rangeChunkSize: 1024 * 1024 };
    expect(pdfDocumentParams({ url: "/file/1" }, assets)).toEqual({ kind: "url", params: { ...common, url: "/file/1" } });
    const small = new Uint8Array(10);
    expect(pdfDocumentParams({ size: 10, initial: small, read: async () => small }, assets)).toEqual({ kind: "data", params: { ...common, data: small } });
    expect(pdfDocumentParams({ size: 99, initial: small, read: async () => small }, assets)).toEqual({ kind: "range", params: common });
  });

  it("opens with the first megabyte, reading the rest on demand", async () => {
    const calls: any[] = [];
    const client = {
      state: { compat: { status: "compatible", capabilities: ["file_ranges"] } },
      request: async (_: string, p: any) => {
        calls.push(p);
        return { name: "a.pdf", mime_type: "application/pdf", size: 5 * PDF_INITIAL_BYTES, offset: p.offset, data: btoa("x".repeat(Math.min(p.length, 3))) };
      },
    } as any;
    const source = await openPdfSource(client, "w1", "a.pdf");
    expect(calls[0]).toMatchObject({ offset: 0, length: PDF_INITIAL_BYTES });
    expect("size" in source && source.size).toBe(5 * PDF_INITIAL_BYTES);
    if (!("read" in source)) throw new Error("expected ranges");
    await source.read(100, 103);
    expect(calls[1]).toMatchObject({ offset: 100, length: 3 });
  });

  it("passes Host errors through", async () => {
    const client = { state: { compat: { status: "compatible", capabilities: ["file_ranges"] } }, request: async () => { throw new ZwiimClientError("file_too_large_for_remote", "big", 413, { size: 1 }); } } as any;
    await expect(openPdfSource(client, "w1", "a.pdf")).rejects.toMatchObject({ code: "file_too_large_for_remote" });
  });
});
