// The key that sends what is typed in a message box, as chosen in settings.
// The other Enter (Shift+Enter, or Enter alone when Mod+Enter sends) is left
// to the text box, which starts a new line.

import { formatShortcut, isModKey, platform, type Platform } from "@/lib/platform";
import type { SendKey } from "@/lib/prefs";

interface EnterPress {
  key: string;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
  nativeEvent: { isComposing: boolean };
}

/** This key press sends the message. Mod+Enter sends either way. */
export function isSendKey(e: EnterPress, mode: SendKey, p: Platform = platform()): boolean {
  if (e.key !== "Enter" || e.shiftKey || e.altKey || e.nativeEvent.isComposing) return false;
  return mode === "enter" || isModKey(e, p);
}

/** Mod+Shift+Enter: send the other way, steering a running turn instead of queueing or the reverse. */
export function isOtherSendKey(e: EnterPress, p: Platform = platform()): boolean {
  if (e.key !== "Enter" || !e.shiftKey || e.altKey || e.nativeEvent.isComposing) return false;
  return isModKey(e, p);
}

/** The send key as this platform writes it; plain Enter is spelled out, a lone ↵ reads as nothing. */
export function sendShortcut(mode: SendKey, p: Platform = platform()): string {
  return mode === "enter" ? "Enter" : formatShortcut("Mod+Enter", p);
}
