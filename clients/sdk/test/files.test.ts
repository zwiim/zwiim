import { describe, expect, it } from "vitest";
import { decodeBase64, hasFileRanges, RANGE_BYTES, readChunks, readFile, readRange, type FileReader } from "../src";

const b64 = (bytes: Uint8Array) => {
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
};
const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i]);

/** A Host holding one file; `ranges` says whether it offers `file_ranges`. */
function fakeHost(file: Uint8Array, ranges = true) {
  const calls: any[] = [];
  const client: FileReader = {
    state: { compat: { status: "compatible", capabilities: ranges ? ["file_ranges"] : [] } } as any,
    request: (async (op: string, params: any) => {
      calls.push({ op, params });
      if (!("offset" in params) && !("length" in params)) {
        return { name: "f.bin", mime_type: "application/octet-stream", size: file.length, data: b64(file) };
      }
      const offset = params.offset ?? 0;
      const end = Math.min(file.length, offset + Math.min(params.length ?? 8 * 1024 * 1024, 8 * 1024 * 1024));
      return { name: "f.bin", mime_type: "application/octet-stream", size: file.length, offset, data: b64(file.subarray(offset, end)) };
    }) as any,
  };
  return { client, calls };
}

const pattern = (n: number) => Uint8Array.from({ length: n }, (_, i) => i % 251);

describe("file reads", () => {
  it("decodes base64", () => {
    expect([...decodeBase64("AAEC/w==")]).toEqual([0, 1, 2, 255]);
    expect(decodeBase64("").length).toBe(0);
  });

  it("knows whether the Host reads ranges", () => {
    expect(hasFileRanges(fakeHost(new Uint8Array(), true).client)).toBe(true);
    expect(hasFileRanges(fakeHost(new Uint8Array(), false).client)).toBe(false);
    expect(hasFileRanges({ state: { compat: null } } as any)).toBe(false);
    expect(hasFileRanges({ state: { compat: { status: "incompatible" } } } as any)).toBe(false);
  });

  it("reads a file in 4 MiB ranges, with progress", async () => {
    const file = pattern(RANGE_BYTES * 2 + 7);
    const { client, calls } = fakeHost(file);
    const progress: number[] = [];
    const bytes = await readFile(client, "w1", "f.bin", { onProgress: (n) => progress.push(n) });
    expect(same(bytes, file)).toBe(true);
    expect(calls.map((c) => c.params.offset)).toEqual([0, RANGE_BYTES, RANGE_BYTES * 2]);
    expect(calls.every((c) => c.params.length === RANGE_BYTES)).toBe(true);
    expect(progress).toEqual([RANGE_BYTES, RANGE_BYTES * 2, file.length]);
  }, 30_000);

  it("yields one empty chunk for an empty file", async () => {
    const { client, calls } = fakeHost(new Uint8Array());
    const chunks = [];
    for await (const c of readChunks(client, "w1", "f.bin")) chunks.push(c);
    expect(chunks).toEqual([{ name: "f.bin", mime_type: "application/octet-stream", size: 0, offset: 0, bytes: new Uint8Array() }]);
    expect(calls.length).toBe(1);
  });

  it("reads the whole file in one call from a Host without ranges", async () => {
    const file = pattern(1000);
    const { client, calls } = fakeHost(file, false);
    expect(await readFile(client, "w1", "f.bin")).toEqual(file);
    expect(calls).toEqual([{ op: "files.download", params: { workspace_id: "w1", path: "f.bin" } }]);
  });

  it("reads one range", async () => {
    const file = pattern(100);
    const { client } = fakeHost(file);
    expect(await readRange(client, "w1", "f.bin", 10, 5)).toEqual({
      bytes: file.subarray(10, 15),
      size: 100,
      name: "f.bin",
      mime_type: "application/octet-stream",
    });
  });
});
