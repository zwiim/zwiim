import type { ServerFrame } from "../types/protocol";

const FRAME_TYPES = new Set([
  "snapshot",
  "replay",
  "event",
  "detached",
  "terminal",
  "host.snapshot",
  "host.event",
  "pong",
  "error",
]);

/** Parse one event-socket text frame; `null` for anything that is not V1. */
export function decodeServerFrame(text: string): ServerFrame | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const type = (value as { type?: unknown }).type;
  if (typeof type !== "string" || !FRAME_TYPES.has(type)) return null;
  return value as ServerFrame;
}

/** Unpadded base64url of the UTF-8 bytes of `text`. */
export function base64Url(text: string): string {
  let binary = "";
  for (const byte of new TextEncoder().encode(text)) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
