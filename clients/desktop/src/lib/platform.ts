// The OS this window runs on, for the few things that differ by name only:
// how a shortcut is written, which modifier key is the main one, what the
// computer is called. Whether a feature is offered is decided by what the
// bridge or host can do, never here. Every helper takes the platform as its
// last argument so tests can pick one.

export type Platform = "macos" | "windows" | "linux" | "unknown";

/** The platform a user agent names. */
export function detectPlatform(ua: string = typeof navigator === "undefined" ? "" : navigator.userAgent): Platform {
  if (ua.includes("Mac")) return "macos";
  if (ua.includes("Windows")) return "windows";
  if (ua.includes("Linux")) return "linux";
  return "unknown";
}

let detected: Platform | null = null;

/** This window's platform; read once. */
export function platform(): Platform {
  return (detected ??= detectPlatform());
}

/** The main modifier is held: ⌘ on a Mac, Ctrl elsewhere. */
export function isModKey(e: { metaKey: boolean; ctrlKey: boolean }, p: Platform = platform()): boolean {
  return p === "macos" ? e.metaKey : e.ctrlKey;
}

const MAC_KEYS: Record<string, string> = { Mod: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Enter: "↵" };
const OTHER_KEYS: Record<string, string> = { Mod: "Ctrl" };

/**
 * A shortcut written `Mod+Shift+F` as this platform shows it: `⌘⇧F` on a Mac,
 * `Ctrl+Shift+F` elsewhere.
 */
export function formatShortcut(combo: string, p: Platform = platform()): string {
  const keys = combo.split("+").map((k) => (k.length === 1 ? k.toUpperCase() : k));
  if (p === "macos") return keys.map((k) => MAC_KEYS[k] ?? k).join("");
  return keys.map((k) => OTHER_KEYS[k] ?? k).join("+");
}

/** What the computer this window runs on is called in a sentence. */
export function deviceNoun(p: Platform = platform()): string {
  if (p === "macos") return "this Mac";
  if (p === "windows") return "this PC";
  return "this computer";
}
