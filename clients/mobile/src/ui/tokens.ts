// The design tokens (DESIGN.md): both palettes, the type scale, spacing and
// radii. Screens use these through `useTheme()`; no hex values in screens.

import { useColorScheme, type TextStyle } from "react-native";

export interface Palette {
  bg: string;
  surface: string;
  raised: string;
  code: string;
  fg: string;
  mut: string;
  faint: string;
  line: string;
  /** A hairline that must read on `raised`: secondary buttons in sheets. */
  lineStrong: string;
  accent: string;
  accentInk: string;
  need: string;
  needInk: string;
  done: string;
  /** Text on `done`: the Merge button. */
  doneInk: string;
  err: string;
  /** The terminal's 16 ANSI colors, muted to sit on `code`. */
  ansi: readonly string[];
  /** File previews' syntax colors, muted to sit on `code`. */
  syntax: { keyword: string; string: string; number: string };
}

export const dark: Palette = {
  bg: "#121416",
  surface: "#1E2123",
  raised: "#282A2D",
  code: "#0D0E10",
  fg: "#EDEFF0",
  mut: "#A3A6A9",
  faint: "#75787C",
  line: "rgba(255,255,255,0.07)",
  lineStrong: "rgba(255,255,255,0.12)",
  accent: "#6CB4F0",
  accentInk: "#071A2B",
  // Needs you is the accent: the app has no amber.
  need: "#6CB4F0",
  needInk: "#071A2B",
  done: "#4CC38A",
  doneInk: "#0B2A1B",
  err: "#FF6467",
  ansi: [
    "#3A3A3E", "#FF6467", "#4CC38A", "#F5B544", "#7AA2F7", "#C597F0", "#5BC8D6", "#D6D6DA",
    "#75787C", "#FF8587", "#6FD8A3", "#F8C86B", "#9BB8FA", "#D5B0F5", "#7FD8E3", "#EDEFF0",
  ],
  syntax: { keyword: "#8EC5F3", string: "#9FD3B5", number: "#E3BFA0" },
};

export const light: Palette = {
  bg: "#FFFFFF",
  surface: "#EFF1F4",
  raised: "#ECEEF1",
  code: "#F3F5F6",
  fg: "#101214",
  mut: "#5F6368",
  faint: "#8C9094",
  line: "rgba(0,0,0,0.08)",
  lineStrong: "rgba(0,0,0,0.12)",
  accent: "#1A68B3",
  accentInk: "#FFFFFF",
  // Needs you is the accent: the app has no amber.
  need: "#1A68B3",
  needInk: "#FFFFFF",
  done: "#1B7A50",
  doneInk: "#FFFFFF",
  err: "#C8343A",
  ansi: [
    "#101214", "#C8343A", "#1B7A50", "#B7791F", "#2F5FD0", "#8A3FC4", "#0F7F8C", "#5F6368",
    "#8C9094", "#E0524F", "#23A06A", "#C98A24", "#4574E0", "#9C55D4", "#1596A5", "#2B2B2E",
  ],
  syntax: { keyword: "#1A68B3", string: "#1F7A52", number: "#A1621B" },
};

const CUBE = [0, 95, 135, 175, 215, 255];
const hex2 = (n: number) => n.toString(16).padStart(2, "0");

/**
 * A terminal color: one of the palette's 16, the xterm 256-color cube and
 * grays above them, or an RGB `#rrggbb` a program set itself.
 */
export function ansiColor(color: number | string, palette: Palette): string {
  if (typeof color === "string") return color;
  if (color < 16) return palette.ansi[color] ?? palette.fg;
  if (color < 232) {
    const i = color - 16;
    return `#${hex2(CUBE[Math.floor(i / 36)]!)}${hex2(CUBE[Math.floor(i / 6) % 6]!)}${hex2(CUBE[i % 6]!)}`;
  }
  const gray = 8 + (color - 232) * 10;
  return `#${hex2(gray)}${hex2(gray)}${hex2(gray)}`;
}

/** Font families, as loaded in `App.tsx`. */
export const fonts = {
  sans: "Geist_400Regular",
  sansMedium: "Geist_500Medium",
  sansSemi: "Geist_600SemiBold",
  mono: "GeistMono_400Regular",
  monoMedium: "GeistMono_500Medium",
} as const;

export const type = {
  display: { fontFamily: fonts.sansSemi, fontSize: 28, lineHeight: 34 },
  title: { fontFamily: fonts.sansSemi, fontSize: 20, lineHeight: 26 },
  headline: { fontFamily: fonts.sansSemi, fontSize: 17, lineHeight: 22 },
  body: { fontFamily: fonts.sans, fontSize: 15, lineHeight: 22 },
  callout: { fontFamily: fonts.sansMedium, fontSize: 13, lineHeight: 18 },
  caption: { fontFamily: fonts.sansSemi, fontSize: 11, lineHeight: 14, letterSpacing: 0.66, textTransform: "uppercase" },
  mono: { fontFamily: fonts.mono, fontSize: 12.5, lineHeight: 18 },
  monoSm: { fontFamily: fonts.mono, fontSize: 11.5, lineHeight: 16 },
  term: { fontFamily: fonts.mono, fontSize: 12, lineHeight: 16 },
} satisfies Record<string, TextStyle>;

/** Mono text follows the OS font scale up to 1.3×. */
export const MONO_MAX_SCALE = 1.3;

export const space = { xxs: 2, xs: 4, sm: 8, md: 12, lg: 16, xl: 24, xxl: 32, xxxl: 48 } as const;
export const radius = { sm: 6, md: 10, lg: 14, xl: 16, sheet: 20, full: 999 } as const;

/** Durations (DESIGN.md, Motion): micro for a press, short for a row change, medium for a sheet. */
export const motion = { micro: 80, short: 180, medium: 280 } as const;

/** A pressed control dims to this. */
export const PRESSED_OPACITY = 0.8;
/** A pressed button shrinks to this, as feedback under the thumb. */
export const PRESSED_SCALE = 0.98;

/** The phone layout's width on iPad (DESIGN.md, Layout). */
export const CONTENT_MAX_WIDTH = 560;

export function useTheme(): Palette {
  return useColorScheme() === "light" ? light : dark;
}
