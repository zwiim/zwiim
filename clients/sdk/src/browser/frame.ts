// A browser stream's binary frame message (design 7.1.2): a 34-byte
// little-endian header, then the JPEG exactly as Chromium produced it.

/** Header format this SDK reads. A newer one means: close and fall back. */
export const BROWSER_FRAME_VERSION = 1;
/** Message type of a screencast frame. */
export const BROWSER_MESSAGE_FRAME = 1;
export const BROWSER_FRAME_HEADER_LEN = 34;

export interface BrowserFrame {
  /** Counts up per screencast; a restarted screencast starts again at 1. */
  number: number;
  /** When the Host took the frame, ms since the epoch. */
  host_ms: number;
  /** The page's size in CSS pixels; the JPEG is larger by the device scale. */
  css_width: number;
  css_height: number;
  /** Page zoom (pinch). */
  scale: number;
  scroll_x: number;
  scroll_y: number;
  jpeg: Uint8Array;
}

export type BrowserFrameResult =
  | { kind: "frame"; frame: BrowserFrame }
  /** A newer Host's format: close the stream. */
  | { kind: "unsupported_version"; version: number }
  /** Not a frame (a later message type), or a broken message: skip it. */
  | { kind: "ignored" };

/** Read one binary message from a browser stream socket. */
export function decodeBrowserFrame(message: ArrayBuffer | Uint8Array): BrowserFrameResult {
  const bytes = message instanceof Uint8Array ? message : new Uint8Array(message);
  const [version, type] = bytes;
  if (version === undefined || type === undefined) return { kind: "ignored" };
  if (version !== BROWSER_FRAME_VERSION) return { kind: "unsupported_version", version };
  if (type !== BROWSER_MESSAGE_FRAME || bytes.length < BROWSER_FRAME_HEADER_LEN) return { kind: "ignored" };
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const length = view.getUint32(30, true);
  if (bytes.length !== BROWSER_FRAME_HEADER_LEN + length) return { kind: "ignored" };
  return {
    kind: "frame",
    frame: {
      number: view.getUint32(2, true),
      host_ms: Number(view.getBigUint64(6, true)),
      css_width: view.getUint16(14, true),
      css_height: view.getUint16(16, true),
      scale: view.getFloat32(18, true),
      scroll_x: view.getFloat32(22, true),
      scroll_y: view.getFloat32(26, true),
      jpeg: bytes.subarray(BROWSER_FRAME_HEADER_LEN),
    },
  };
}
