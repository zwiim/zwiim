// Where a PDF's bytes come from and how pdf.js should load them; no pdf.js
// import here, so it runs (and tests) without a DOM.
import { hasFileRanges, readFile, readRange, type FileReader } from "@zwiim/sdk";

export const PDF_INITIAL_BYTES = 1024 * 1024;
const RANGE_CHUNK = 1024 * 1024;

export type PdfSource =
  | { url: string }
  | { size: number; initial: Uint8Array; read(begin: number, end: number): Promise<Uint8Array> };

export interface PdfAssets {
  workerSrc: string;
  cMapUrl: string;
  standardFontDataUrl: string;
  wasmUrl: string;
}

export function pdfDocumentParams(source: PdfSource, assets: PdfAssets) {
  const common = {
    cMapUrl: assets.cMapUrl,
    cMapPacked: true,
    standardFontDataUrl: assets.standardFontDataUrl,
    wasmUrl: assets.wasmUrl,
    isEvalSupported: false,
    disableAutoFetch: true,
    disableStream: true,
    rangeChunkSize: RANGE_CHUNK,
  };
  if ("url" in source) return { kind: "url" as const, params: { ...common, url: source.url } };
  if (source.initial.length >= source.size) return { kind: "data" as const, params: { ...common, data: source.initial } };
  return { kind: "range" as const, params: common };
}

/** The file's first megabyte now, the rest as pdf.js asks; the whole file from a Host without ranges. */
export async function openPdfSource(client: FileReader, workspaceId: string, path: string): Promise<PdfSource> {
  if (!hasFileRanges(client)) {
    const bytes = await readFile(client, workspaceId, path);
    return { size: bytes.length, initial: bytes, read: async () => new Uint8Array() };
  }
  const first = await readRange(client, workspaceId, path, 0, PDF_INITIAL_BYTES);
  return {
    size: first.size,
    initial: first.bytes,
    read: async (begin, end) => (await readRange(client, workspaceId, path, begin, end - begin)).bytes,
  };
}
