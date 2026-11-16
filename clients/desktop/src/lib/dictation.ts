import type { VoicePrefs } from "@/lib/prefs";

const escapePattern = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const words = (text: string) => new RegExp(`(?<![\\p{L}\\p{N}_])${escapePattern(text)}(?![\\p{L}\\p{N}_])`, "giu");

export function cleanDictation(text: string, prefs: Pick<VoicePrefs, "removeFillers" | "vocabulary">): string {
  if (prefs.removeFillers) text = text.replace(/(?<![\p{L}\p{N}_])(?:um|uh|erm|hmm)(?![\p{L}\p{N}_])[,;:]?\s*/giu, "");
  for (const { word, heardAs } of prefs.vocabulary) {
    if (word.trim()) text = text.replace(words(heardAs.trim() || word), () => word);
  }
  return text.replace(/\s+/g, " ").trim();
}

/** The new value and the caret just after the inserted words, before the suffix. */
export function insertAtCaret(
  value: string,
  selStart: number,
  selEnd: number,
  text: string,
): { value: string; caret: number } {
  const start = Math.max(0, Math.min(value.length, selStart));
  const end = Math.max(start, Math.min(value.length, selEnd));
  if (!text) return { value, caret: start };
  const before = value.slice(0, start);
  const after = value.slice(end);
  const inserted = `${before && !/\s$/.test(before) && !/^\s/.test(text) ? " " : ""}${text}`;
  const gap = after && !/^\s/.test(after) && !/\s$/.test(text) ? " " : "";
  return {
    value: before + inserted + gap + after,
    caret: start + inserted.length,
  };
}
