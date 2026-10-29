// How big the window draws, which fonts its code and terminals use, and its
// colors: this device's choices, applied to the window whenever they change.

import type { FontFamily, InstalledFonts, NativeBridge } from "@/lib/bridge";
import { codeLigatures, colorVision, fonts, onPrefsChange, softDark, zoom, ZOOM_LEVELS } from "@/lib/prefs";

/** The app's own monospace fonts, as `--font-mono` lists them (styles.css). */
export const MONO_FONTS = '"JetBrains Mono Variable", ui-monospace, "SF Mono", Menlo, monospace';

/** The font list that draws in `name` where it is installed, else in the app's own. */
export function monoFonts(name: string): string {
  return name ? `"${name}", ${MONO_FONTS}` : MONO_FONTS;
}

/** The terminal's colours, the `--terminal` token (styles.css): dark in both themes. */
export const TERMINAL_BG = "#0d0e10";
export const TERMINAL_FG = "#d5d8db";

/** The fonts a terminal draws in: its own choice, else code's. */
export function terminalFonts(): string {
  const f = fonts();
  return monoFonts(f.terminal || f.code);
}

/**
 * Whether `name` is a font this computer has, by whether text drawn in it
 * measures differently from the generic families; null where nothing can measure.
 */
export function fontInstalled(name: string): boolean | null {
  if (typeof OffscreenCanvas === "undefined") return null;
  const ctx = new OffscreenCanvas(1, 1).getContext("2d");
  if (!ctx) return null;
  const width = (font: string) => {
    ctx.font = `72px ${font}`;
    return ctx.measureText("mmmmmmmmmmlli10WwQ@").width;
  };
  return ["monospace", "serif", "sans-serif"].some((generic) => width(`"${name}", ${generic}`) !== width(generic));
}

const lists = new WeakMap<InstalledFonts, Promise<FontFamily[]>>();

/** The fonts installed here, asked for once; none where this window cannot list them. */
export function installedFonts(bridge: NativeBridge): Promise<FontFamily[]> {
  const source = bridge.fonts;
  if (!source) return Promise.resolve([]);
  let list = lists.get(source);
  if (!list) {
    list = source.list().catch(() => {
      lists.delete(source);
      return [];
    });
    lists.set(source, list);
  }
  return list;
}

/** The most fonts other than monospace ones a search shows. */
export const MAX_OTHER_FONTS = 50;

/**
 * The fonts a picker offers for `query`: the monospace ones, and the rest
 * only once searched for, as code and terminals need every character the same width.
 */
export function fontChoices(all: FontFamily[], query: string): { mono: string[]; other: string[] } {
  const q = query.trim().toLowerCase();
  const hit = (f: FontFamily) => f.name.toLowerCase().includes(q);
  return {
    mono: all.filter((f) => f.mono && hit(f)).map((f) => f.name),
    other: q
      ? all
          .filter((f) => !f.mono && hit(f))
          .slice(0, MAX_OTHER_FONTS)
          .map((f) => f.name)
      : [],
  };
}

/** The next zoom level from `current`, one step `by` (-1 smaller, +1 larger); the same at either end. */
export function stepZoom(current: number, by: -1 | 1): number {
  const i = ZOOM_LEVELS.findIndex((z) => z === current);
  const at = i < 0 ? ZOOM_LEVELS.indexOf(1) : i;
  return ZOOM_LEVELS[Math.min(ZOOM_LEVELS.length - 1, Math.max(0, at + by))];
}

/**
 * Applies the remembered zoom, code font, ligatures and colors now, and again
 * whenever they change; returns the stop. The colors are a class and an
 * attribute on the root, which styles.css reads.
 */
export function watchAppearance(bridge: NativeBridge): () => void {
  let zoomed = 1;
  let code = "";
  const apply = () => {
    const root = document.documentElement;
    root.classList.toggle("no-code-ligatures", !codeLigatures());
    root.classList.toggle("soft-dark", softDark());
    const vision = colorVision();
    if (vision === "default") root.removeAttribute("data-color-vision");
    else root.setAttribute("data-color-vision", vision);
    const z = zoom();
    if (z !== zoomed && bridge.zoom) {
      zoomed = z;
      bridge.zoom.set(z).catch(() => {});
    }
    const f = fonts().code;
    if (f !== code) {
      code = f;
      if (f) document.documentElement.style.setProperty("--font-mono", monoFonts(f));
      else document.documentElement.style.removeProperty("--font-mono");
    }
  };
  apply();
  return onPrefsChange(apply);
}
