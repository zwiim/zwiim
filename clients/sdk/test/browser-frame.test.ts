import { describe, expect, it } from "vitest";
import { decodeBrowserFrame } from "../src/browser/frame";

/** The Host's `encode_frame(7, 1_700_000_000_123, {800, 600, 1.5, 2, 120.5}, b"JPEG")`. */
function hostFrame(): Uint8Array {
  const out = new Uint8Array(34 + 4);
  const v = new DataView(out.buffer);
  out[0] = 1;
  out[1] = 1;
  v.setUint32(2, 7, true);
  v.setBigUint64(6, 1_700_000_000_123n, true);
  v.setUint16(14, 800, true);
  v.setUint16(16, 600, true);
  v.setFloat32(18, 1.5, true);
  v.setFloat32(22, 2, true);
  v.setFloat32(26, 120.5, true);
  v.setUint32(30, 4, true);
  out.set(new TextEncoder().encode("JPEG"), 34);
  return out;
}

describe("browser frames", () => {
  it("reads the header and the JPEG", () => {
    const r = decodeBrowserFrame(hostFrame());
    expect(r.kind).toBe("frame");
    if (r.kind !== "frame") return;
    expect(r.frame).toMatchObject({
      number: 7,
      host_ms: 1_700_000_000_123,
      css_width: 800,
      css_height: 600,
      scale: 1.5,
      scroll_x: 2,
      scroll_y: 120.5,
    });
    expect(new TextDecoder().decode(r.frame.jpeg)).toBe("JPEG");
  });

  it("reads from an ArrayBuffer at an offset too", () => {
    const frame = hostFrame();
    const padded = new Uint8Array(frame.length + 3);
    padded.set(frame, 3);
    const r = decodeBrowserFrame(padded.subarray(3));
    expect(r.kind === "frame" && r.frame.number).toBe(7);
    expect(decodeBrowserFrame(frame.slice().buffer).kind).toBe("frame");
  });

  it("stops on a newer format and skips what it does not know", () => {
    const newer = hostFrame();
    newer[0] = 2;
    expect(decodeBrowserFrame(newer)).toEqual({ kind: "unsupported_version", version: 2 });
    const cursor = hostFrame();
    cursor[1] = 2;
    expect(decodeBrowserFrame(cursor)).toEqual({ kind: "ignored" });
    expect(decodeBrowserFrame(hostFrame().subarray(0, 36))).toEqual({ kind: "ignored" });
    expect(decodeBrowserFrame(new Uint8Array(0))).toEqual({ kind: "ignored" });
  });
});
