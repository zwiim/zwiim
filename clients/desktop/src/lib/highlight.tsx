// Syntax colors for code: transcript code blocks, the file viewer and diffs.
// Grammars and themes load on first use, so a window that never shows code
// never pays for them; until a language is ready its code draws plain.
//
// Each token carries both the light and the dark color as CSS variables
// (`--tok-light`, `--tok-dark`), and styles.css picks one by the window's
// theme, so switching light and dark never re-colors anything.

import { Fragment, useEffect, useMemo, useState } from "react";
import type { HighlighterCore, ThemedToken, ThemeRegistration } from "shiki/core";
import { bundledLanguages } from "shiki/langs";
import { bundledThemes } from "shiki/themes";
import { codeTheme, type CodeTheme } from "@/lib/prefs";
import { usePref } from "@/lib/use-pref";

export type { ThemedToken };

/** A code theme as the picker offers it, and the light and dark themes it draws with. */
export interface CodeThemeDef {
  id: CodeTheme;
  label: string;
  light: string;
  dark: string;
}

export const CODE_THEMES: readonly CodeThemeDef[] = [
  { id: "zwiim", label: "Zwiim", light: "zwiim", dark: "zwiim" },
  { id: "github", label: "GitHub", light: "github-light", dark: "github-dark" },
  { id: "one", label: "One", light: "one-light", dark: "one-dark-pro" },
  { id: "catppuccin", label: "Catppuccin", light: "catppuccin-latte", dark: "catppuccin-mocha" },
  { id: "vitesse", label: "Vitesse", light: "vitesse-light", dark: "vitesse-dark" },
  { id: "solarized", label: "Solarized", light: "solarized-light", dark: "solarized-dark" },
  { id: "rose-pine", label: "Rosé Pine", light: "rose-pine-dawn", dark: "rose-pine" },
];

/** Past these, code draws plain: highlighting it would stall the window. */
export const MAX_HIGHLIGHT_CHARS = 200_000;
export const MAX_HIGHLIGHT_LINES = 5_000;

const LANGUAGE_BY_NAME: Record<string, string> = {
  dockerfile: "dockerfile",
  makefile: "makefile",
  gnumakefile: "makefile",
  cmakelists: "cmake",
  gemfile: "ruby",
  rakefile: "ruby",
  podfile: "ruby",
  justfile: "just",
};

/** The grammar for a language name or alias, as a fenced block names it; null for none. */
export function languageId(name: string | undefined): string | null {
  const lang = name?.trim().toLowerCase();
  if (!lang || lang === "text" || lang === "plaintext" || lang === "txt") return null;
  return Object.hasOwn(bundledLanguages, lang) ? lang : null;
}

/** The grammar for a file, by its name or extension; null for none. */
export function languageFor(path: string): string | null {
  const name = (path.split("/").pop() ?? path).toLowerCase();
  const stem = name.replace(/\.txt$/, "");
  if (LANGUAGE_BY_NAME[stem]) return LANGUAGE_BY_NAME[stem]!;
  if (name.startsWith("dockerfile")) return "dockerfile";
  const dot = name.lastIndexOf(".");
  return dot > 0 || (dot === 0 && name.length > 1) ? languageId(name.slice(dot + 1)) : null;
}

/**
 * The Zwiim theme: every color is a CSS variable (`--syntax-*`) that
 * styles.css sets for light and dark from the app's own palette.
 */
async function zwiimTheme(): Promise<ThemeRegistration> {
  const { createCssVariablesTheme } = await import("shiki/core");
  return createCssVariablesTheme({ name: "zwiim", variablePrefix: "--syntax-" });
}

let highlighter: Promise<HighlighterCore> | null = null;

function core(): Promise<HighlighterCore> {
  highlighter ??= (async () => {
    const [{ createHighlighterCore }, { createJavaScriptRegexEngine }] = await Promise.all([
      import("shiki/core"),
      import("shiki/engine/javascript"),
    ]);
    return createHighlighterCore({
      engine: createJavaScriptRegexEngine({ forgiving: true }),
      themes: [zwiimTheme()],
      langs: [],
    });
  })();
  return highlighter;
}

/** What is loaded: kept apart from the highlighter so a render can ask without waiting. */
let ready: HighlighterCore | null = null;
const loading = new Map<string, Promise<void>>();

function load(key: string, run: (h: HighlighterCore) => Promise<void>): Promise<void> {
  let p = loading.get(key);
  if (!p) {
    p = core().then(async (h) => {
      await run(h);
      ready = h;
    });
    // A grammar that fails to load leaves its code plain; a later try may succeed.
    p.catch(() => loading.delete(key));
    loading.set(key, p);
  }
  return p;
}

function themesOf(theme: CodeTheme): CodeThemeDef {
  return CODE_THEMES.find((t) => t.id === theme) ?? CODE_THEMES[0]!;
}

/** Loads a language and a theme pair; resolves once code in them can be colored. */
export async function prepare(lang: string, theme: CodeTheme): Promise<void> {
  const def = themesOf(theme);
  await Promise.all([
    load(`lang:${lang}`, (h) => h.loadLanguage(bundledLanguages[lang as keyof typeof bundledLanguages])),
    ...[def.light, def.dark]
      .filter((t) => t !== "zwiim")
      .map((t) => load(`theme:${t}`, (h) => h.loadTheme(bundledThemes[t as keyof typeof bundledThemes]))),
  ]);
}

function loaded(lang: string, def: CodeThemeDef): boolean {
  if (!ready) return false;
  const themes = ready.getLoadedThemes();
  return ready.getLoadedLanguages().includes(lang) && themes.includes(def.light) && themes.includes(def.dark);
}

/** Whether code this size is colored at all. */
export function highlightable(code: string): boolean {
  if (code.length > MAX_HIGHLIGHT_CHARS) return false;
  let lines = 1;
  for (let i = code.indexOf("\n"); i !== -1; i = code.indexOf("\n", i + 1)) {
    if (++lines > MAX_HIGHLIGHT_LINES) return false;
  }
  return true;
}

/**
 * The colored lines of `code`, one token list per line, or null while its
 * language or theme is still loading, or when it is too big to color.
 */
export function tokensNow(code: string, lang: string | null, theme: CodeTheme): ThemedToken[][] | null {
  if (!lang || !highlightable(code)) return null;
  const def = themesOf(theme);
  if (!loaded(lang, def)) return null;
  try {
    return ready!.codeToTokens(code, {
      lang,
      themes: { light: def.light, dark: def.dark },
      defaultColor: false,
      cssVariablePrefix: "--tok-",
      tokenizeMaxLineLength: 1000,
      tokenizeTimeLimit: 500,
    }).tokens;
  } catch {
    return null;
  }
}

/**
 * Colored lines for each text, in the chosen code theme, or null for one not
 * colored (yet). Re-renders once a language loads or the theme changes.
 */
export function useHighlighted(texts: readonly string[], lang: string | null): (ThemedToken[][] | null)[] {
  const theme = usePref(codeTheme);
  const [version, setVersion] = useState(0);
  const wanted = lang !== null && texts.some(highlightable);
  const tokens = useMemo(
    () => texts.map((t) => tokensNow(t, lang, theme)),
    // `version` re-reads once a load lands; texts compare by content.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [texts.join("\u0000"), lang, theme, version],
  );
  const missing = wanted && tokens.every((t) => t === null);
  useEffect(() => {
    if (!missing || !lang) return;
    let live = true;
    prepare(lang, theme).then(
      () => live && setVersion((v) => v + 1),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [missing, lang, theme]);
  return tokens;
}

/** One line's tokens, each colored by the window's theme. */
export function TokenLine({ tokens }: { tokens: readonly ThemedToken[] }) {
  return tokens.map((t, i) => (
    <span key={i} className="tok" style={t.htmlStyle as React.CSSProperties}>
      {t.content}
    </span>
  ));
}

/** Code in one language, colored once its grammar is ready; its text is the code exactly. */
export function HighlightedCode({ code, lang }: { code: string; lang: string | null }) {
  const [tokens] = useHighlighted([code], lang);
  if (!tokens) return code;
  return tokens.map((line, i) => (
    <Fragment key={i}>
      {i > 0 && "\n"}
      <TokenLine tokens={line} />
    </Fragment>
  ));
}
