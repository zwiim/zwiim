// What the browser panel sends the Host's tab (design 7.6): pointer and key
// events in the page's CSS pixels, the shortcuts the panel keeps for itself,
// and what an address bar entry means.

import type { BrowserInput, BrowserModifiers } from "@zwiim/sdk";
import { isModKey, platform, type Platform } from "@/lib/platform";

type Modifiers = { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean };

/** CDP's modifier bits: Alt 1, Ctrl 2, Meta 4, Shift 8. */
export function modifiersOf(e: Modifiers): BrowserModifiers {
  return (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
}

const BUTTONS = ["left", "middle", "right", "back", "forward"] as const;

/** The DOM's `MouseEvent.button` as CDP names it. */
export function buttonOf(button: number): (typeof BUTTONS)[number] | "none" {
  return BUTTONS[button] ?? "none";
}

/** Where a frame of `page` (CSS pixels) is drawn in a box: as large as fits, centred. */
export interface Fit {
  x: number;
  y: number;
  width: number;
  height: number;
}

export function fitFrame(box: { width: number; height: number }, page: { width: number; height: number }): Fit {
  if (page.width <= 0 || page.height <= 0 || box.width <= 0 || box.height <= 0) {
    return { x: 0, y: 0, width: 0, height: 0 };
  }
  const k = Math.min(box.width / page.width, box.height / page.height);
  const width = page.width * k;
  const height = page.height * k;
  return { x: (box.width - width) / 2, y: (box.height - height) / 2, width, height };
}

/**
 * A point in the box (pixels from its top left) as page CSS pixels, or null
 * outside the drawn frame.
 */
export function toPage(
  point: { x: number; y: number },
  fit: Fit,
  page: { width: number; height: number },
): { x: number; y: number } | null {
  if (fit.width <= 0 || fit.height <= 0) return null;
  const x = ((point.x - fit.x) / fit.width) * page.width;
  const y = ((point.y - fit.y) / fit.height) * page.height;
  if (x < 0 || y < 0 || x > page.width || y > page.height) return null;
  return { x, y };
}

/** A point in page CSS pixels as pixels from the box's top left: the inverse of {@link toPage}. */
export function fromPage(point: { x: number; y: number }, fit: Fit, page: { width: number; height: number }) {
  if (fit.width <= 0 || page.width <= 0 || page.height <= 0) return null;
  return { x: fit.x + (point.x / page.width) * fit.width, y: fit.y + (point.y / page.height) * fit.height };
}

type KeyLike = Modifiers & { key: string; code: string; keyCode: number };

/** A key press or release for the page. Only a key that types one character carries `text`. */
export function keyInput(e: KeyLike, action: "down" | "up"): BrowserInput {
  const types = action === "down" && [...e.key].length === 1 && !e.ctrlKey && !e.metaKey;
  return {
    type: "key",
    action,
    key: e.key,
    code: e.code,
    modifiers: modifiersOf(e),
    ...(types ? { text: e.key } : {}),
    ...(e.keyCode ? { key_code: e.keyCode } : {}),
  };
}

export type BrowserShortcut = "address" | "new_tab" | "close_tab" | "reload" | "back" | "forward";

/** The shortcuts the panel keeps (design 7.6); every other key goes to the page. */
export function browserShortcut(e: KeyLike, p: Platform = platform()): BrowserShortcut | null {
  if (!isModKey(e, p) || e.altKey) return null;
  switch (e.key.toLowerCase()) {
    case "l":
      return e.shiftKey ? null : "address";
    case "t":
      return e.shiftKey ? null : "new_tab";
    case "w":
      return e.shiftKey ? null : "close_tab";
    case "r":
      return "reload";
    case "[":
      return "back";
    case "]":
      return "forward";
    default:
      return null;
  }
}

const LOCAL = /^(localhost|127(\.\d{1,3}){3}|\[::1\]|0\.0\.0\.0)(:\d+)?([/?#]|$)/i;

/**
 * What an address bar entry opens: as typed with a scheme, `http://` for this
 * computer's own servers, `https://` otherwise. Null for an empty entry.
 */
export function addressUrl(text: string): string | null {
  const t = text.trim();
  if (!t) return null;
  if (/^[a-z][a-z0-9+.-]*:/i.test(t) && !/^[^/]+:\d+([/?#]|$)/.test(t)) return t;
  return `${LOCAL.test(t) ? "http" : "https"}://${t}`;
}
